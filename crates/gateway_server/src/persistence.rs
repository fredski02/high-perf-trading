//! Gateway persistence for account state
//!
//! Uses the same journal + snapshot strategy as the engine, but for AccountUpdate events

use anyhow::Context;
use crc32fast::Hasher;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::account_manager::AccountUpdate;

/// Append-only journal for account updates
pub struct AccountJournal {
    file: File,
    #[allow(dead_code)]
    path: PathBuf,
    
    // Batching
    batch_buffer: Vec<AccountUpdate>,
    #[allow(dead_code)]
    batch_size: usize,
    last_sync: Instant,
    #[allow(dead_code)]
    sync_interval: Duration,
    
    // Stats
    updates_written: u64,
}

/// Configuration for journal behavior
#[derive(Debug, Clone)]
pub struct JournalConfig {
    pub batch_size: usize,
    pub sync_interval: Duration,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            sync_interval: Duration::from_millis(100),
        }
    }
}

impl AccountJournal {
    #[allow(dead_code)]
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::open_with_config(path, JournalConfig::default())
    }

    pub fn open_with_config(
        path: impl AsRef<Path>,
        config: JournalConfig,
    ) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        
        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)?;

        Ok(Self {
            file,
            path,
            batch_buffer: Vec::with_capacity(config.batch_size.max(1)),
            batch_size: config.batch_size,
            last_sync: Instant::now(),
            sync_interval: config.sync_interval,
            updates_written: 0,
        })
    }

    /// Append an account update with IMMEDIATE fsync
    /// 
    /// Since we only journal account creation (rare operation), we always fsync immediately
    /// to ensure zero data loss for account creation.
    #[inline]
    pub fn append(&mut self, update: &AccountUpdate) -> anyhow::Result<()> {
        // Write frame directly (no batching)
        self.write_frame(update)?;
        self.updates_written += 1;

        // IMMEDIATE fsync for durability (account creation is rare)
        #[cfg(unix)]
        {
            self.file.sync_data()?;
        }

        #[cfg(not(unix))]
        {
            self.file.sync_all()?;
        }

        self.last_sync = Instant::now();
        Ok(())
    }

    /// Force flush all buffered updates to disk
    pub fn flush(&mut self) -> anyhow::Result<()> {
        if self.batch_buffer.is_empty() {
            return Ok(());
        }

        let updates: Vec<AccountUpdate> = self.batch_buffer.drain(..).collect();

        for update in updates {
            self.write_frame(&update)?;
            self.updates_written += 1;
        }

        // fsync for durability
        #[cfg(unix)]
        {
            self.file.sync_data()?;
        }

        #[cfg(not(unix))]
        {
            self.file.sync_all()?;
        }

        self.last_sync = Instant::now();
        Ok(())
    }

    /// Write a single framed update with checksum
    fn write_frame(&mut self, update: &AccountUpdate) -> anyhow::Result<()> {
        let payload = postcard::to_stdvec(update).context("postcard serialize")?;

        // Calculate CRC32
        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let checksum = hasher.finalize();

        // Write: [u32 len][payload][u32 crc32]
        let len = payload.len() as u32;
        self.file.write_all(&len.to_le_bytes())?;
        self.file.write_all(&payload)?;
        self.file.write_all(&checksum.to_le_bytes())?;

        Ok(())
    }

    /// Read entire journal and return updates in order
    pub fn read_all(&mut self) -> anyhow::Result<Vec<AccountUpdate>> {
        self.file.seek(SeekFrom::Start(0))?;

        let mut out = Vec::new();
        let mut offset = 0u64;

        loop {
            let mut hdr = [0u8; 4];

            match self.file.read_exact(&mut hdr) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }

            let len = u32::from_le_bytes(hdr) as usize;
            offset += 4;

            // Sanity check
            if len > 10 * 1024 * 1024 {
                anyhow::bail!(
                    "corrupt journal: frame too large ({} bytes) at offset {}",
                    len,
                    offset
                );
            }

            let mut buf = vec![0u8; len];
            self.file.read_exact(&mut buf)?;
            offset += len as u64;

            let mut crc_bytes = [0u8; 4];
            match self.file.read_exact(&mut crc_bytes) {
                Ok(()) => {}
                Err(_) => {
                    tracing::warn!("journal: missing checksum at offset {}, skipping", offset);
                    break;
                }
            }
            offset += 4;

            let expected_crc = u32::from_le_bytes(crc_bytes);

            // Verify checksum
            let mut hasher = Hasher::new();
            hasher.update(&buf);
            let actual_crc = hasher.finalize();

            if actual_crc != expected_crc {
                tracing::warn!(
                    "journal: checksum mismatch at offset {} (expected {:08x}, got {:08x}), stopping replay",
                    offset - (len as u64 + 8),
                    expected_crc,
                    actual_crc
                );
                break;
            }

            let update: AccountUpdate =
                postcard::from_bytes(&buf).context("postcard deserialize")?;
            out.push(update);
        }

        // Ready for append again
        let _ = self.file.seek(SeekFrom::End(0));

        Ok(out)
    }

    #[allow(dead_code)]
    pub fn stats(&self) -> JournalStats {
        JournalStats {
            updates_written: self.updates_written,
            pending_batch_size: self.batch_buffer.len(),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct JournalStats {
    pub updates_written: u64,
    pub pending_batch_size: usize,
}

// ====== Snapshot Support ======

/// Snapshot of all account state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountSnapshot {
    pub sequence: u64,
    pub accounts: Vec<AccountStateSnapshot>,
}

/// Serializable account state for snapshots
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountStateSnapshot {
    pub account_id: common::AccountId,
    pub buying_power: i64,
    pub positions: Vec<(common::SymbolId, common::Position)>,
    pub risk_limits: Vec<(common::SymbolId, common::RiskLimits)>,
}

impl AccountSnapshot {
    /// Save snapshot to disk
    pub fn save(&self, snapshot_dir: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
        let dir = snapshot_dir.as_ref();
        std::fs::create_dir_all(dir)?;

        let path = dir.join(format!("account_snapshot_{:012}.bin", self.sequence));
        let mut file = File::create(&path)?;

        // Serialize entire snapshot
        let data = postcard::to_stdvec(self).context("serialize snapshot")?;

        // Write: [u64 sequence][u32 len][data][u32 crc32]
        file.write_all(&self.sequence.to_le_bytes())?;
        file.write_all(&(data.len() as u32).to_le_bytes())?;
        file.write_all(&data)?;

        // Checksum
        let mut hasher = Hasher::new();
        hasher.update(&data);
        let checksum = hasher.finalize();
        file.write_all(&checksum.to_le_bytes())?;

        file.sync_all()?;

        tracing::info!(
            "account snapshot saved: {:?} (seq={}, accounts={}, size={})",
            path,
            self.sequence,
            self.accounts.len(),
            data.len()
        );

        Ok(path)
    }

    /// Load the latest snapshot from directory
    pub fn load_latest(snapshot_dir: impl AsRef<Path>) -> anyhow::Result<Option<Self>> {
        let dir = snapshot_dir.as_ref();
        if !dir.exists() {
            return Ok(None);
        }

        let mut entries: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.starts_with("account_snapshot_") && s.ends_with(".bin"))
                    .unwrap_or(false)
            })
            .collect();

        if entries.is_empty() {
            return Ok(None);
        }

        // Sort by filename (which contains sequence number)
        entries.sort_by_key(|e| e.path());
        let latest = entries.last().unwrap();

        Self::load_from_file(latest.path())
    }

    /// Load snapshot from specific file
    pub fn load_from_file(path: impl AsRef<Path>) -> anyhow::Result<Option<Self>> {
        let path = path.as_ref();
        let mut file = File::open(path)?;

        let mut seq_bytes = [0u8; 8];
        file.read_exact(&mut seq_bytes)?;
        let sequence = u64::from_le_bytes(seq_bytes);

        let mut len_bytes = [0u8; 4];
        file.read_exact(&mut len_bytes)?;
        let len = u32::from_le_bytes(len_bytes) as usize;

        let mut data = vec![0u8; len];
        file.read_exact(&mut data)?;

        let mut crc_bytes = [0u8; 4];
        match file.read_exact(&mut crc_bytes) {
            Ok(()) => {
                let expected_crc = u32::from_le_bytes(crc_bytes);
                let mut hasher = Hasher::new();
                hasher.update(&data);
                let actual_crc = hasher.finalize();

                if actual_crc != expected_crc {
                    anyhow::bail!(
                        "snapshot checksum mismatch: {:?} (expected {:08x}, got {:08x})",
                        path,
                        expected_crc,
                        actual_crc
                    );
                }
            }
            Err(_) => {
                tracing::warn!("snapshot missing checksum: {:?}, accepting anyway", path);
            }
        }

        let snapshot: AccountSnapshot =
            postcard::from_bytes(&data).context("deserialize snapshot")?;

        tracing::info!(
            "account snapshot loaded: {:?} (seq={}, accounts={}, size={})",
            path,
            sequence,
            snapshot.accounts.len(),
            data.len()
        );

        Ok(Some(snapshot))
    }

    /// Clean up old snapshots, keeping only the latest N
    pub fn cleanup_old(snapshot_dir: impl AsRef<Path>, keep: usize) -> anyhow::Result<usize> {
        let dir = snapshot_dir.as_ref();
        if !dir.exists() {
            return Ok(0);
        }

        let mut entries: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.starts_with("account_snapshot_") && s.ends_with(".bin"))
                    .unwrap_or(false)
            })
            .collect();

        if entries.len() <= keep {
            return Ok(0);
        }

        entries.sort_by_key(|e| e.path());

        let to_remove = entries.len() - keep;
        let mut removed = 0;

        for entry in entries.iter().take(to_remove) {
            if std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
                tracing::info!("removed old snapshot: {:?}", entry.path());
            }
        }

        Ok(removed)
    }
}