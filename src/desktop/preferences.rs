//! User preferences are independent of script state and game saves.
use anyhow::{Context, Result, ensure};
use rugp_runtime::{audio_device::AcsSettings, font::TextStyle, message_window::TextSettings};
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
    /// BGM, voice and effect switches; muting keeps the chosen level.
    #[serde(default = "default_channels")]
    pub channels: [bool; 3],
    pub fullscreen: bool,
    /// Windowed size in percent, as the retail 画面サイズ menu (50–250, steps of 25).
    #[serde(default = "default_window_scale")]
    pub window_scale: u16,
    /// フォント page; None follows Setup.ini.
    #[serde(default)]
    pub font: Option<FontPreferences>,
    /// サウンド: 音声再生中にＢＧＭの音量を小さくする (bSmallBgmInPlayVoice, on).
    #[serde(default = "enabled")]
    pub duck_bgm: bool,
    #[serde(default)]
    pub acs: AcsSettings,
    /// 設定 menu マウス追尾 (rvmm bMouseTrace, on).
    #[serde(default = "enabled")]
    pub mouse_follow: bool,
    /// 設定 menu 読んだ文章は飛ばす (rvmm nSeenMsgSkip, on).
    #[serde(default = "enabled")]
    pub read_skip: bool,
    #[serde(default)]
    pub system: SystemPreferences,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FontPreferences {
    /// None keeps the retail face.
    pub family: Option<String>,
    pub size: u16,
    pub face_bold: bool,
    pub style: TextStyle,
}
impl FontPreferences {
    pub fn from_runtime(settings: &TextSettings) -> Self {
        Self {
            family: settings.family.clone(),
            size: settings.size,
            face_bold: settings.face_bold,
            style: settings.style,
        }
    }
    pub fn to_runtime(&self) -> TextSettings {
        TextSettings {
            family: self.family.clone(),
            size: self.size,
            face_bold: self.face_bold,
            style: self.style,
        }
    }
}

/// History page counts for the システム page's 小..大 trackbar.
pub(super) const HISTORY_SIZES: [u16; 5] = [64, 128, 256, 512, 1024];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(super) struct SystemPreferences {
    /// テキスト履歴バッファサイズ in pages.
    pub history_pages: u16,
    /// 拡大縮小系のバイリニアフィルタリングを行う.
    pub bilinear: bool,
    /// 他のプロセスがアクティブになった時、フルスクリーンを解除する.
    pub leave_fullscreen_inactive: bool,
    /// フルスクリーン時に、ウインドウを最前面にする.
    pub fullscreen_topmost: bool,
    /// セーブ／ロードなどのリスト選択実行を、ダブルクリックで実行する.
    pub double_click_lists: bool,
}
impl Default for SystemPreferences {
    fn default() -> Self {
        Self {
            history_pages: 256,
            bilinear: true,
            leave_fullscreen_inactive: false,
            fullscreen_topmost: false,
            double_click_lists: false,
        }
    }
}
fn default_auto_delay() -> u32 {
    1500
}
fn default_channels() -> [bool; 3] {
    [true; 3]
}
fn default_window_scale() -> u16 {
    100
}
fn enabled() -> bool {
    true
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            text_speed_ms: 0,
            auto_delay_ms: default_auto_delay(),
            // Setup.ini nCsmIni_CSoundManEx_n{Bgm,Voice,Effect}Vol of 4096:
            // 2868 (70 %), 4096 and 2048 (50 %).
            levels: [100, 70, 100, 50],
            channels: default_channels(),
            fullscreen: false,
            window_scale: default_window_scale(),
            font: None,
            duck_bgm: true,
            acs: AcsSettings::default(),
            mouse_follow: true,
            read_skip: true,
            system: SystemPreferences::default(),
        }
    }
}
impl Preferences {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.text_speed_ms <= 100
                && (250..=10000).contains(&self.auto_delay_ms)
                && self.levels.iter().all(|&v| v <= 100)
                && (50..=250).contains(&self.window_scale)
                && self.window_scale.is_multiple_of(25)
                && self.font.as_ref().is_none_or(|f| {
                    (16..=32).contains(&f.size)
                        && f.style.outline <= 2
                        && f.family
                            .as_ref()
                            .is_none_or(|n| !n.is_empty() && n.len() <= 256)
                })
                && self.acs.output <= 2
                && self.acs.model <= 3
                && HISTORY_SIZES.contains(&self.system.history_pages),
            "invalid settings value"
        );
        Ok(())
    }
    /// Mixer levels after the per-channel switches.
    pub fn output_levels(&self) -> [u8; 4] {
        let mut levels = self.levels;
        for (level, &enabled) in levels[1..].iter_mut().zip(&self.channels) {
            if !enabled {
                *level = 0;
            }
        }
        levels
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
            channels: [false, true, true],
            fullscreen: true,
            window_scale: 150,
            font: Some(FontPreferences {
                family: Some("IPAGothic".into()),
                size: 28,
                face_bold: false,
                style: TextStyle {
                    shadow: true,
                    ..TextStyle::default()
                },
            }),
            duck_bgm: false,
            acs: AcsSettings {
                output: 0,
                ..AcsSettings::default()
            },
            mouse_follow: false,
            read_skip: false,
            system: SystemPreferences {
                history_pages: 1024,
                ..SystemPreferences::default()
            },
        };
        assert_eq!(settings.output_levels(), [80, 0, 100, 45]);
        settings.save(Some(&directory))?;
        assert_eq!(Preferences::load(Some(&directory))?, settings);
        let mut invalid = settings.clone();
        invalid.levels[1] = 101;
        assert!(invalid.save(Some(&directory)).is_err());
        invalid = settings.clone();
        invalid.window_scale = 110;
        assert!(invalid.save(Some(&directory)).is_err());
        invalid = settings.clone();
        invalid.system.history_pages = 100;
        assert!(invalid.save(Some(&directory)).is_err());
        assert_eq!(Preferences::load(Some(&directory))?, settings);
        let path = directory.join("settings.json");
        fs::write(
            &path,
            br#"{"text_speed_ms":25,"levels":[100,100,100,100],"fullscreen":false}"#,
        )?;
        let older = Preferences::load(Some(&directory))?;
        assert_eq!(older.auto_delay_ms, 1500);
        assert_eq!((older.channels, older.window_scale), ([true; 3], 100));
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
