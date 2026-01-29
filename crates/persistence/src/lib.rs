use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Context;
use common::Command;
use crc32fast::Hasher;

/// Append-only command journal with batching, fsync, checksums, and snapshots
///
/// Frame format:
///   [u32 len][postcard(Command) bytes][u32 crc32]
///
/// Improvements:
/// - CRC32 checksums for corruption detection
/// - Configurable fsync batching (every N commands or T duration)
/// - Snapshot support for faster recovery
/// - Journal rotation
pub struct Journal {
    file: File,
    path: PathBuf,
    
    // Batching config
    batch_buffer: Vec<Command>,
    batch_size: usize,
    last_sync: Instant,
    sync_interval: Duration,
    
    // Stats
    commands_written: u64,
    commands_since_rotation: u64,
    rotation_threshold: u64,
}

/// Configuration for journal behavior
#[derive(Debug, Clone)]
pub struct JournalConfig {
    /// Number of commands to buffer before fsync (0 = sync every command)
    pub batch_size: usize,
    
    /// Max duration between fsyncs
    pub sync_interval: Duration,
    
    /// Rotate journal after this many commands (0 = never rotate)
    pub rotation_threshold: u64,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            sync_interval: Duration::from_millis(100),
            rotation_threshold: 1_000_000, // 1M commands
        }
    }
}

impl Journal {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::open_with_config(path, JournalConfig::default())
    }

    pub fn open_with_config(path: impl AsRef<Path>, config: JournalConfig) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
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
            commands_written: 0,
            commands_since_rotation: 0,
            rotation_threshold: config.rotation_threshold,
        })
    }

    /// Append a command (buffered, may not be durable immediately)
    #[inline]
    pub fn append(&mut self, cmd: &Command) -> anyhow::Result<()> {
        self.batch_buffer.push(*cmd);
        
        let should_flush = self.batch_buffer.len() >= self.batch_size
            || self.last_sync.elapsed() >= self.sync_interval;
            
        if should_flush {
            self.flush()?;
        }
        
        Ok(())
    }

    /// Force flush all buffered commands to disk
    pub fn flush(&mut self) -> anyhow::Result<()> {
        if self.batch_buffer.is_empty() {
            return Ok(());
        }

        // Drain commands into a temporary vec to avoid borrow checker issues
        let commands: Vec<Command> = self.batch_buffer.drain(..).collect();
        
        for cmd in commands {
            self.write_frame(&cmd)?;
            self.commands_written += 1;
            self.commands_since_rotation += 1;
        }

        // fsync for durability
        #[cfg(unix)]
        {
            self.file.sync_data()?; // faster than sync_all (no metadata sync)
        }
        
        #[cfg(not(unix))]
        {
            self.file.sync_all()?;
        }
        
        self.last_sync = Instant::now();
        Ok(())
    }

    /// Write a single framed command with checksum
    fn write_frame(&mut self, cmd: &Command) -> anyhow::Result<()> {
        let payload = postcard::to_stdvec(cmd).context("postcard serialize")?;

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

    /// Read entire journal and return commands in order
    /// Validates checksums and skips corrupted entries
    pub fn read_all(&mut self) -> anyhow::Result<Vec<Command>> {
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
                anyhow::bail!("corrupt journal: frame too large ({} bytes) at offset {}", len, offset);
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
                // Stop at first corruption (don't skip, play it safe)
                break;
            }

            let cmd: Command = postcard::from_bytes(&buf).context("postcard deserialize")?;
            out.push(cmd);
        }

        // ready for append again
        let _ = self.file.seek(SeekFrom::End(0));

        Ok(out)
    }

    /// Check if journal should be rotated
    pub fn should_rotate(&self) -> bool {
        self.rotation_threshold > 0 && self.commands_since_rotation >= self.rotation_threshold
    }

    /// Rotate to a new journal file with timestamp
    pub fn rotate(&mut self) -> anyhow::Result<PathBuf> {
        // Flush pending writes
        self.flush()?;

        // Generate timestamped backup name
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let stem = self.path.file_stem().unwrap_or_default().to_string_lossy();
        let backup_path = parent.join(format!("{}_{}.bin", stem, ts));

        // Rename current file
        std::fs::rename(&self.path, &backup_path)?;

        // Open new file
        let new_file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)?;

        self.file = new_file;
        self.commands_since_rotation = 0;

        tracing::info!("journal rotated: {:?} -> {:?}", self.path, backup_path);

        Ok(backup_path)
    }

    /// Get current stats
    pub fn stats(&self) -> JournalStats {
        JournalStats {
            commands_written: self.commands_written,
            commands_since_rotation: self.commands_since_rotation,
            pending_batch_size: self.batch_buffer.len(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct JournalStats {
    pub commands_written: u64,
    pub commands_since_rotation: u64,
    pub pending_batch_size: usize,
}

// ====== Snapshot Support ======

/// Snapshot of order book state at a given sequence
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub sequence: u64,
    pub data: Vec<u8>,
}

impl Snapshot {
    /// Save snapshot to disk
    pub fn save(&self, snapshot_dir: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
        let dir = snapshot_dir.as_ref();
        std::fs::create_dir_all(dir)?;

        let path = dir.join(format!("snapshot_{:012}.bin", self.sequence));
        let mut file = File::create(&path)?;

        // Write: [u64 sequence][u32 len][data][u32 crc32]
        file.write_all(&self.sequence.to_le_bytes())?;
        file.write_all(&(self.data.len() as u32).to_le_bytes())?;
        file.write_all(&self.data)?;

        // Checksum the data
        let mut hasher = Hasher::new();
        hasher.update(&self.data);
        let checksum = hasher.finalize();
        file.write_all(&checksum.to_le_bytes())?;

        file.sync_all()?;

        tracing::info!("snapshot saved: {:?} (seq={}, size={})", path, self.sequence, self.data.len());

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
                    .map(|s| s.starts_with("snapshot_") && s.ends_with(".bin"))
                    .unwrap_or(false)
            })
            .collect();

        if entries.is_empty() {
            return Ok(None);
        }

        // Sort by filename (which contains sequence number)
        entries.sort_by_key(|e| e.path());
        let latest = entries.last().unwrap();

        Self::load_from_file(&latest.path())
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

        tracing::info!("snapshot loaded: {:?} (seq={}, size={})", path, sequence, data.len());

        Ok(Some(Self { sequence, data }))
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
                    .map(|s| s.starts_with("snapshot_") && s.ends_with(".bin"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use common::{NewOrder, OrderFlags, Side, TimeInForce};

    fn test_cmd() -> Command {
        Command::NewOrder(NewOrder {
            client_seq: 1,
            order_id: 100,
            account_id: 42,
            symbol_id: 1,
            side: Side::Buy,
            price: 100,
            qty: 10,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        })
    }

    #[test]
    fn test_journal_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.journal");

        let mut j = Journal::open(&path).unwrap();
        j.append(&test_cmd()).unwrap();
        j.flush().unwrap();

        let cmds = j.read_all().unwrap();
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn test_snapshot_roundtrip() {
        let dir = tempfile::tempdir().unwrap();

        let snap = Snapshot {
            sequence: 1234,
            data: vec![1, 2, 3, 4, 5],
        };

        snap.save(dir.path()).unwrap();

        let loaded = Snapshot::load_latest(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.sequence, 1234);
        assert_eq!(loaded.data, vec![1, 2, 3, 4, 5]);
    }
}