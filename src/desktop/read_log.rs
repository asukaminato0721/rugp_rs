//! Pages already read, for 読んだ文章は飛ばす. Host data beside the
//! settings; the retail UUC read flags are not interpreted.
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const FILE: &str = "read.bin";
const LIMIT: usize = 4 * 1024 * 1024;
/// Write after this many new pages, besides on exit.
const FLUSH_EVERY: usize = 64;

#[derive(Default)]
pub(super) struct ReadLog {
    pages: BTreeSet<u64>,
    path: Option<PathBuf>,
    unsaved: usize,
}
impl ReadLog {
    pub fn load(directory: Option<&Path>) -> Result<Self> {
        let Some(directory) = directory else {
            return Ok(Self::default());
        };
        let path = directory.join(FILE);
        let mut bytes = Vec::new();
        match fs::File::open(&path) {
            Ok(file) => {
                file.take(LIMIT as u64 + 1).read_to_end(&mut bytes)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("open read log"),
        }
        ensure!(
            bytes.len() <= LIMIT && bytes.len().is_multiple_of(8),
            "invalid read log"
        );
        let pages = bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|b| u64::from_le_bytes(*b))
            .collect();
        Ok(Self {
            pages,
            path: Some(path),
            unsaved: 0,
        })
    }
    pub fn contains(&self, page: u64) -> bool {
        self.pages.contains(&page)
    }
    pub fn insert(&mut self, page: u64) {
        if self.pages.len() * 8 < LIMIT && self.pages.insert(page) {
            self.unsaved += 1;
            if self.unsaved >= FLUSH_EVERY
                && let Err(error) = self.flush()
            {
                eprintln!("Read log: {error:#}");
            }
        }
    }
    pub fn flush(&mut self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if self.unsaved == 0 {
            return Ok(());
        }
        let temporary = path.with_extension("pending");
        let bytes: Vec<u8> = self.pages.iter().flat_map(|p| p.to_le_bytes()).collect();
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        self.unsaved = 0;
        Ok(())
    }
}
impl Drop for ReadLog {
    fn drop(&mut self) {
        if let Err(error) = self.flush() {
            eprintln!("Read log: {error:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pages_persist_and_damaged_logs_are_rejected() -> Result<()> {
        let directory = std::env::temp_dir().join(format!("rugp-read-{}", std::process::id()));
        fs::create_dir_all(&directory)?;
        {
            let mut log = ReadLog::load(Some(&directory))?;
            log.insert(7);
            log.insert(u64::MAX);
            assert!(log.contains(7) && !log.contains(8));
        }
        let log = ReadLog::load(Some(&directory))?;
        assert!(log.contains(7) && log.contains(u64::MAX));
        drop(log);
        fs::write(directory.join(FILE), [0; 5])?;
        assert!(ReadLog::load(Some(&directory)).is_err());
        fs::remove_dir_all(directory)?;
        Ok(())
    }
}
