//! Locate original Windows saves. Nothing here writes to them.
use anyhow::{Context, Result};
use rugp_runtime::Game;
use std::{fs, path::PathBuf};

pub(super) struct NativeStore {
    pub directory: PathBuf,
    pub choices: Vec<u32>,
}
impl NativeStore {
    pub fn detect(game: &Game, supplied: Option<PathBuf>, choices: Vec<u32>) -> Result<Self> {
        let project = game
            .archive
            .project
            .as_ref()
            .context("game project missing")?;
        let root = game.archive.volumes[0]
            .path
            .parent()
            .context("game root missing")?;
        let mut candidates = Vec::new();
        if let Some(path) = supplied {
            let directory = if path.join("Vmreg").is_dir() {
                path.join("Vmreg")
            } else {
                path
            };
            return Ok(Self { directory, choices });
        }
        // The registry path is relative to Documents, e.g. age\\君が望む永遠LE.
        let mut relative = PathBuf::new();
        for component in project.registry.split(['\\', '/']) {
            anyhow::ensure!(
                !component.is_empty() && component != "." && component != "..",
                "invalid project save path"
            );
            relative.push(component);
        }
        relative.push("Vmreg");
        for name in ["USERPROFILE", "HOME"] {
            if let Some(home) = std::env::var_os(name) {
                candidates.push(PathBuf::from(home).join("Documents").join(&relative));
            }
        }
        for ancestor in root.ancestors() {
            if ancestor.file_name().is_some_and(|n| n == "drive_c") {
                if let Ok(users) = fs::read_dir(ancestor.join("users")) {
                    for user in users {
                        candidates.push(user?.path().join("Documents").join(&relative));
                    }
                }
                break;
            }
        }
        candidates.push(root.join("Vmreg"));
        let directory = candidates
            .iter()
            .find(|p| p.is_dir())
            .cloned()
            .unwrap_or_else(|| candidates[0].clone());
        Ok(Self { directory, choices })
    }
    pub fn path(&self, slot: u32) -> PathBuf {
        self.directory.join(format!("vmr{slot:05}.uuc"))
    }
    pub fn slots(&self) -> Result<Vec<(u32, bool)>> {
        let mut slots: std::collections::BTreeMap<u32, bool> =
            (1..=10).map(|n| (n, false)).collect();
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(slots.into_iter().collect());
            }
            Err(e) => return Err(e).context("read original save directory"),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(number) = name
                .strip_prefix("vmr")
                .and_then(|s| s.strip_suffix(".uuc"))
            else {
                continue;
            };
            if number.len() == 5
                && number.bytes().all(|c| c.is_ascii_digit())
                && let Ok(slot) = number.parse::<u32>()
                && slot > 0
                && entry.file_type()?.is_file()
            {
                slots.insert(slot, true);
            }
        }
        Ok(slots.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lists_only_original_slots_including_later_pages() -> Result<()> {
        let directory = std::env::temp_dir().join(format!(
            "rugp-native-list-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::create_dir(&directory)?;
        for name in [
            "vmr00001.uuc",
            "vmr00002.uuc",
            "vmr00123.uuc",
            "vmr00000.uuc",
            "vmr1.uuc",
            "vmrABCDE.uuc",
            "slot-0.rugpsave",
        ] {
            fs::write(directory.join(name), b"fixture")?;
        }
        fs::create_dir(directory.join("vmr00003.uuc"))?;
        let store = NativeStore {
            directory: directory.clone(),
            choices: vec![],
        };
        let slots = store.slots()?;
        assert_eq!(slots.len(), 11);
        assert_eq!(slots[0], (1, true));
        assert_eq!(slots[1], (2, true));
        assert_eq!(slots[2], (3, false));
        assert_eq!(slots[10], (123, true));
        assert_eq!(store.path(123).file_name().unwrap(), "vmr00123.uuc");
        fs::remove_dir_all(directory)?;
        assert_eq!(store.slots()?.len(), 10);
        Ok(())
    }
}
