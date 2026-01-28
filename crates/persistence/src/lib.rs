use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use anyhow::Context;
use common::Command;

/// Append-only command journal
///
/// Format:
///   [u32 len][postcard(Command) bytes]
///
/// Very similar to your TCP framing → nice symmetry.
pub struct Journal {
    file: File,
}

impl Journal {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        Ok(Self { file })
    }

    /// Append a command
    #[inline]
    pub fn append(&mut self, cmd: &Command) -> anyhow::Result<()> {
        let payload = postcard::to_stdvec(cmd).context("postcard serialize")?;

        let len = payload.len() as u32;

        self.file.write_all(&len.to_le_bytes())?;
        self.file.write_all(&payload)?;

        // Phase 1 durability: flush only
        self.file.flush()?;
        Ok(())
    }

    /// Read entire journal and return commands in order
    pub fn read_all(&mut self) -> anyhow::Result<Vec<Command>> {
        self.file.seek(SeekFrom::Start(0))?;

        let mut out = Vec::new();

        loop {
            let mut hdr = [0u8; 4];

            match self.file.read_exact(&mut hdr) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }

            let len = u32::from_le_bytes(hdr) as usize;

            let mut buf = vec![0u8; len];
            self.file.read_exact(&mut buf)?;

            let cmd: Command = postcard::from_bytes(&buf).context("postcard deserialize")?;
            out.push(cmd);
        }

        // ready for append again
        let _ = self.file.seek(SeekFrom::End(0));

        Ok(out)
    }
}
