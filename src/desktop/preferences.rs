//! User preferences are independent of script state and game saves.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preferences {
    pub text_speed_ms: u32,
    #[serde(default = "default_auto_delay")]
    pub auto_delay_ms: u32,
    /// Master, BGM, voice, effect.
    pub levels: [u8; 4],
    pub fullscreen: bool,
}
fn default_auto_delay() -> u32 {
    1500
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            text_speed_ms: 0,
            auto_delay_ms: default_auto_delay(),
            levels: [100; 4],
            fullscreen: false,
        }
    }
}
impl Preferences {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.text_speed_ms <= 100
                && (250..=10000).contains(&self.auto_delay_ms)
                && self.levels.iter().all(|&v| v <= 100),
            "invalid settings value"
        );
        Ok(())
    }
    pub fn load(directory: Option<&Path>) -> Result<Self> {
        let Some(directory) = directory else {
            return Ok(Self::default());
        };
        let mut file = match fs::File::open(directory.join("settings.json")) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).context("open settings"),
        };
        let mut bytes = Vec::new();
        (&mut file).take(4097).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 4096, "settings file is too large");
        let settings: Self = serde_json::from_slice(&bytes)?;
        settings.validate()?;
        Ok(settings)
    }
    pub fn save(&self, directory: Option<&Path>) -> Result<()> {
        self.validate()?;
        let Some(directory) = directory else {
            return Ok(());
        };
        let temporary = directory.join(format!(
            "settings-{}-{}.pending",
            std::process::id(),
            TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let result = (|| -> Result<()> {
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, directory.join("settings.json"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_persist_and_reject_invalid_or_oversized_files() -> Result<()> {
        let directory = std::env::temp_dir().join(format!(
            "rugp-settings-{}-{}",
            std::process::id(),
            TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory)?;
        assert_eq!(Preferences::load(Some(&directory))?, Preferences::default());
        let settings = Preferences {
            text_speed_ms: 35,
            auto_delay_ms: 2000,
            levels: [80, 25, 100, 45],
            fullscreen: true,
        };
        settings.save(Some(&directory))?;
        assert_eq!(Preferences::load(Some(&directory))?, settings);
        let mut invalid = settings.clone();
        invalid.levels[1] = 101;
        assert!(invalid.save(Some(&directory)).is_err());
        assert_eq!(Preferences::load(Some(&directory))?, settings);
        let path = directory.join("settings.json");
        fs::write(
            &path,
            br#"{"text_speed_ms":25,"levels":[100,100,100,100],"fullscreen":false}"#,
        )?;
        assert_eq!(Preferences::load(Some(&directory))?.auto_delay_ms, 1500);
        fs::write(
            &path,
            br#"{"text_speed_ms":101,"levels":[100,100,100,100],"fullscreen":false}"#,
        )?;
        assert!(Preferences::load(Some(&directory)).is_err());
        fs::write(&path, vec![b' '; 4097])?;
        assert!(Preferences::load(Some(&directory)).is_err());
        fs::remove_dir_all(directory)?;
        Ok(())
    }
}
