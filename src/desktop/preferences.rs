//! User preferences are independent of script state and game saves.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preferences {
    pub text_speed_ms: u32,
    /// Master, BGM, voice, effect.
    pub levels: [u8; 4],
    pub fullscreen: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            text_speed_ms: 0,
            levels: [100; 4],
            fullscreen: false,
        }
    }
}
impl Preferences {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.text_speed_ms <= 100 && self.levels.iter().all(|&v| v <= 100),
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
        let temporary = directory.join(format!("settings-{}.pending", std::process::id()));
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
