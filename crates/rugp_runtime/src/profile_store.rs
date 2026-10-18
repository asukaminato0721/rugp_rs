//! Session cache and optional atomic, per-game user-data backing for CStdb.
use anyhow::{Context, Result, ensure};
use rugp_assets::profile::Profile;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub struct ProfileStore {
    directory: Option<PathBuf>,
    _lock: Option<File>,
    profiles: BTreeMap<u64, Profile>,
}
impl ProfileStore {
    const LIMIT: usize = 64 * 1024 * 1024;
    pub fn persistent(directory: &Path) -> Result<Self> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("profiles.lock"))?;
        lock.try_lock()
            .context("user data is in use by another runtime")?;
        Ok(Self {
            directory: Some(directory.to_owned()),
            _lock: Some(lock),
            profiles: BTreeMap::new(),
        })
    }
    pub fn directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }
    fn path(directory: &Path, id: u64) -> PathBuf {
        directory.join(format!("database-{id:016x}.txt"))
    }
    pub fn load(&mut self, id: u64, initial: impl FnOnce() -> Result<Profile>) -> Result<&Profile> {
        if !self.profiles.contains_key(&id) {
            ensure!(
                self.profiles.len() < 4096,
                "profile database count limit exceeded"
            );
            let saved = self.directory.as_ref().map(|dir| Self::path(dir, id));
            let profile = match saved.as_ref().map(fs::metadata) {
                Some(Ok(metadata)) => {
                    ensure!(
                        metadata.is_file() && metadata.len() <= 16 * 1024 * 1024,
                        "invalid/oversized saved profile"
                    );
                    let mut bytes = Vec::new();
                    File::open(saved.as_ref().unwrap())?
                        .take(16 * 1024 * 1024 + 1)
                        .read_to_end(&mut bytes)?;
                    ensure!(
                        bytes.len() <= 16 * 1024 * 1024,
                        "saved profile grew beyond size limit"
                    );
                    Profile::from_text(String::from_utf8(bytes)?)?
                }
                Some(Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(error.into());
                }
                _ => initial()?,
            };
            ensure!(
                self.profiles
                    .values()
                    .map(|p| p.text().len())
                    .sum::<usize>()
                    + profile.text().len()
                    <= Self::LIMIT,
                "profile cache limit exceeded"
            );
            self.profiles.insert(id, profile);
        }
        Ok(&self.profiles[&id])
    }
    /// Add `other`'s missing entries to a loaded profile in memory only; they
    /// reach disk with the next write.
    pub fn merge_missing(&mut self, id: u64, other: &Profile) -> Result<()> {
        let old = self
            .profiles
            .get(&id)
            .context("profile database not loaded")?;
        let mut next = old.clone();
        next.merge_missing(other)?;
        let total = self
            .profiles
            .values()
            .map(|p| p.text().len())
            .sum::<usize>()
            - old.text().len()
            + next.text().len();
        ensure!(total <= Self::LIMIT, "profile cache limit exceeded");
        self.profiles.insert(id, next);
        Ok(())
    }
    pub fn write_string(&mut self, id: u64, key: &str, value: &str) -> Result<()> {
        let old = self
            .profiles
            .get(&id)
            .context("profile database not loaded")?;
        let mut next = old.clone();
        next.write_string(key, value)?;
        let total = self
            .profiles
            .values()
            .map(|p| p.text().len())
            .sum::<usize>()
            - old.text().len()
            + next.text().len();
        ensure!(total <= Self::LIMIT, "profile cache limit exceeded");
        if let Some(directory) = &self.directory {
            // Never follow or truncate an old temporary file. A crash may
            // leave a unique pending file, but not a partially written save.
            let temporary = directory.join(format!(
                "database-{id:016x}-{}-{}.pending",
                std::process::id(),
                TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
            ));
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(next.text().as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, Self::path(directory, id))?;
            #[cfg(unix)]
            File::open(directory)?.sync_all()?;
        }
        self.profiles.insert(id, next);
        Ok(())
    }
}

/// Resolve a user-owned location; the caller adds a game-specific identifier.
pub fn default_data_root() -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base =
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Application Support"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")));
    Ok(base
        .context("no user-data location; specify --user-data-dir")?
        .join("rugp_rs"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_profiles_survive_reopen_and_lock_excludes_another_writer() -> Result<()> {
        let directory = std::env::temp_dir().join(format!(
            "rugp-profiles-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::create_dir(&directory)?;
        let mut store = ProfileStore::persistent(&directory)?;
        assert!(ProfileStore::persistent(&directory).is_err());
        store.load(42, || Profile::from_text("old={kept}".into()))?;
        store.write_string(42, "new", "保存;{nested}")?;
        store.write_string(42, "old", "updated")?;
        drop(store);
        let mut store = ProfileStore::persistent(&directory)?;
        let profile = store.load(42, || anyhow::bail!("saved data must replace initial data"))?;
        assert_eq!(profile.read_string("new", ""), "保存;{nested}");
        assert_eq!(profile.read_string("old", ""), "updated");
        drop(store);
        let path = ProfileStore::path(&directory, 42);
        fs::write(&path, [0xff])?;
        let mut store = ProfileStore::persistent(&directory)?;
        assert!(store.load(42, || Ok(Profile::default())).is_err());
        assert_eq!(fs::read(&path)?, [0xff]); // never reset invalid saved data
        drop(store);
        fs::remove_file(ProfileStore::path(&directory, 42))?;
        fs::remove_file(directory.join("profiles.lock"))?;
        fs::remove_dir(&directory)?;
        Ok(())
    }
}
