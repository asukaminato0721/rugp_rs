//! Port save files and the slot list of the retail browser (rvmm.dll
//! CUucFileList). A port save is a JSON header with the browser's columns,
//! followed by the runtime checkpoint. They live in the port's user data,
//! never in the original Vmreg directory, whose saves are listed read-only.
//! 重要度高 is the read-only file attribute, as the retail manual describes.
use super::native_store::NativeStore;
use anyhow::{Context, Result, ensure};
use rugp_assets::uuc::{NativeArchive, metadata::NativeMetadata};
use rugp_runtime::{Game, game::snapshot::GameSnapshot};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAGIC: &[u8; 8] = b"RUGPSLOT";
const MAX_HEADER: usize = 1024 * 1024;
/// Retail files run to vmr01000.uuc, growing ten at a time.
pub const MAX_SLOT: u32 = 1000;
pub const THUMBNAIL: (u16, u16) = (64, 48);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub version: u32,
    pub application: String,
    /// Seconds since the Unix epoch.
    pub saved_at: i64,
    /// OM_SetUucNavigateString 現在状況説明文.
    pub navigation: String,
    pub comment: String,
    pub thumbnail: Thumbnail,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Thumbnail {
    pub width: u16,
    pub height: u16,
    /// Top-down RGBA.
    pub rgba: Vec<u8>,
}
impl Thumbnail {
    /// Box-filter the screen down to the retail 64x48.
    pub fn from_frame(rgba: &[u8], (width, height): (u32, u32)) -> Self {
        let (tw, th) = (u32::from(THUMBNAIL.0), u32::from(THUMBNAIL.1));
        let mut out = Vec::with_capacity((tw * th * 4) as usize);
        for ty in 0..th {
            let (y0, y1) = (
                ty * height / th,
                ((ty + 1) * height / th).max(ty * height / th + 1),
            );
            for tx in 0..tw {
                let (x0, x1) = (
                    tx * width / tw,
                    ((tx + 1) * width / tw).max(tx * width / tw + 1),
                );
                let mut sum = [0u32; 3];
                for y in y0..y1.min(height) {
                    for x in x0..x1.min(width) {
                        let i = ((y * width + x) * 4) as usize;
                        for c in 0..3 {
                            sum[c] += u32::from(rgba[i + c]);
                        }
                    }
                }
                let count = ((y1.min(height) - y0) * (x1.min(width) - x0)).max(1);
                out.extend(sum.map(|v| (v / count) as u8));
                out.push(255);
            }
        }
        Self {
            width: THUMBNAIL.0,
            height: THUMBNAIL.1,
            rgba: out,
        }
    }
    fn valid(&self) -> bool {
        usize::from(self.width) * usize::from(self.height) * 4 == self.rgba.len()
            && self.width <= 256
            && self.height <= 256
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Port,
    /// A Windows vmrNNNNN.uuc, loaded by route replay.
    Original,
}
/// One occupied row of the browser.
#[derive(Clone, Debug)]
pub struct Entry {
    pub slot: u32,
    pub kind: Kind,
    pub application: String,
    pub saved_at: Option<i64>,
    pub navigation: String,
    pub comment: String,
    pub thumbnail: Option<Thumbnail>,
    pub important: bool,
    /// ヘッダ情報の読み込みに失敗しました。
    pub broken: bool,
}

pub(super) struct SaveStore {
    pub directory: Option<PathBuf>,
}
impl SaveStore {
    /// Port saves sit beside the other per-game user data.
    pub fn new(user_data: Option<&Path>) -> Self {
        Self {
            directory: user_data.map(|d| d.join("Saves")),
        }
    }
    pub fn path(&self, slot: u32) -> Option<PathBuf> {
        self.directory
            .as_ref()
            .map(|d| d.join(format!("vmr{slot:05}.rugp")))
    }
    /// Occupied slots, port saves shadowing an original of the same number.
    pub fn list(&self, native: &NativeStore) -> Result<Vec<Entry>> {
        let mut entries = std::collections::BTreeMap::new();
        for (slot, occupied) in native.slots()? {
            if occupied && slot <= MAX_SLOT {
                entries.insert(slot, original_entry(slot, &native.path(slot)));
            }
        }
        if let Some(directory) = &self.directory {
            let listing = match fs::read_dir(directory) {
                Ok(listing) => Some(listing),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e).context("read save directory"),
            };
            for file in listing.into_iter().flatten() {
                let file = file?;
                let name = file.file_name();
                let Some(slot) = name
                    .to_str()
                    .and_then(|n| n.strip_prefix("vmr")?.strip_suffix(".rugp"))
                    .filter(|n| n.len() == 5)
                    .and_then(|n| n.parse::<u32>().ok())
                    .filter(|n| (1..=MAX_SLOT).contains(n))
                else {
                    continue;
                };
                entries.insert(slot, port_entry(slot, &file.path()));
            }
        }
        Ok(entries.into_values().collect())
    }
    pub fn save(&self, slot: u32, header: &Header, snapshot: &GameSnapshot) -> Result<()> {
        let path = self
            .path(slot)
            .context("this session has no user-data folder")?;
        ensure!((1..=MAX_SLOT).contains(&slot), "invalid save slot {slot}");
        ensure!(!important(&path), "save {slot} is read-only (重要度高)");
        let mut body = Vec::new();
        snapshot.write(&mut body)?;
        write_atomic(&path, header, &body)
    }
    pub fn load(&self, slot: u32) -> Result<GameSnapshot> {
        let path = self
            .path(slot)
            .context("this session has no user-data folder")?;
        let mut file = fs::File::open(&path).with_context(|| format!("open {}", path.display()))?;
        read_header(&mut file)?;
        GameSnapshot::read(file)
    }
    pub fn set_comment(&self, slot: u32, comment: &str) -> Result<()> {
        let path = self
            .path(slot)
            .context("this session has no user-data folder")?;
        ensure!(!important(&path), "save {slot} is read-only (重要度高)");
        let mut file = fs::File::open(&path)?;
        let mut header = read_header(&mut file)?;
        let mut body = Vec::new();
        file.read_to_end(&mut body)?;
        header.comment = comment.into();
        write_atomic(&path, &header, &body)
    }
    pub fn set_important(&self, slot: u32, important: bool) -> Result<()> {
        let path = self
            .path(slot)
            .context("this session has no user-data folder")?;
        let mut permissions = fs::metadata(&path)?.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(important);
        fs::set_permissions(&path, permissions)?;
        Ok(())
    }
    pub fn delete(&self, slot: u32) -> Result<()> {
        let path = self
            .path(slot)
            .context("this session has no user-data folder")?;
        ensure!(!important(&path), "save {slot} is read-only (重要度高)");
        fs::remove_file(&path).with_context(|| format!("delete {}", path.display()))
    }
}

fn important(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.permissions().readonly())
}
fn write_atomic(path: &Path, header: &Header, body: &[u8]) -> Result<()> {
    let header_bytes = serde_json::to_vec(header)?;
    ensure!(header_bytes.len() <= MAX_HEADER, "save header too large");
    let directory = path.parent().context("save directory missing")?;
    fs::create_dir_all(directory)?;
    let temporary = path.with_extension("rugp.tmp");
    let result = (|| -> Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(MAGIC)?;
        file.write_all(&(header_bytes.len() as u32).to_le_bytes())?;
        file.write_all(&header_bytes)?;
        file.write_all(body)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error.context("write save file"));
    }
    fs::rename(&temporary, path).context("replace save file")?;
    Ok(())
}
fn read_header(input: &mut impl Read) -> Result<Header> {
    let mut magic = [0; 12];
    input.read_exact(&mut magic)?;
    ensure!(&magic[..8] == MAGIC, "not a port save file");
    let size = u32::from_le_bytes(magic[8..].try_into()?) as usize;
    ensure!(size <= MAX_HEADER, "save header too large");
    let mut bytes = vec![0; size];
    input.read_exact(&mut bytes)?;
    let header: Header = serde_json::from_slice(&bytes)?;
    ensure!(header.version == 1, "unsupported save header version");
    ensure!(header.thumbnail.valid(), "invalid save thumbnail");
    Ok(header)
}
fn port_entry(slot: u32, path: &Path) -> Entry {
    let header = fs::File::open(path)
        .map_err(anyhow::Error::from)
        .and_then(|mut file| read_header(&mut file));
    match header {
        Ok(header) => Entry {
            slot,
            kind: Kind::Port,
            application: header.application,
            saved_at: Some(header.saved_at),
            navigation: header.navigation,
            comment: header.comment,
            thumbnail: Some(header.thumbnail),
            important: important(path),
            broken: false,
        },
        Err(_) => broken(slot, Kind::Port, important(path)),
    }
}
fn original_entry(slot: u32, path: &Path) -> Entry {
    let read = || -> Result<NativeMetadata> {
        let archive = NativeArchive::read(fs::File::open(path)?)?;
        NativeMetadata::read(&archive.metadata)
    };
    let Ok(metadata) = read() else {
        return broken(slot, Kind::Original, important(path));
    };
    let thumbnail = Thumbnail {
        width: metadata.thumbnail.width,
        height: metadata.thumbnail.height,
        rgba: metadata.thumbnail.rgba,
    };
    // The native clock's origin is unknown; the file time is what Windows shows.
    let saved_at = fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    Entry {
        slot,
        kind: Kind::Original,
        application: metadata.application,
        saved_at,
        navigation: metadata.navigation,
        // Vm60 stores the slot text as `comment={...}`.
        comment: metadata
            .comment
            .strip_prefix("comment={")
            .and_then(|c| c.strip_suffix('}'))
            .unwrap_or(&metadata.comment)
            .into(),
        thumbnail: thumbnail.valid().then_some(thumbnail),
        important: important(path),
        broken: false,
    }
}
fn broken(slot: u32, kind: Kind, important: bool) -> Entry {
    Entry {
        slot,
        kind,
        application: String::new(),
        saved_at: None,
        navigation: String::new(),
        comment: String::new(),
        thumbnail: None,
        important,
        broken: true,
    }
}

/// The browser's 【%Y %m/%d %H:%M】 column in local time.
pub fn format_time(seconds: i64) -> String {
    let Ok(timestamp) = jiff::Timestamp::from_second(seconds) else {
        return String::new();
    };
    let zoned = timestamp.to_zoned(jiff::tz::TimeZone::system());
    zoned.strftime("【%Y %m/%d %H:%M】").to_string()
}

/// The game name column: the last part of the `age\\…` registry path that
/// the retail metadata stores.
pub fn game_name(application: &str) -> &str {
    application
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(application)
}

/// Capture what the retail save writes: the screen, scene text and comment.
pub fn header(game: &Game, comment: String) -> Header {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    Header {
        version: 1,
        application: game
            .archive
            .project
            .as_ref()
            .map_or_else(|| game.title().into(), |p| p.registry.clone()),
        saved_at: now,
        navigation: game.navigation.clone(),
        comment,
        thumbnail: Thumbnail::from_frame(game.frame(), game.size()),
    }
}

/// Default comment: the last read text, or at a choice the bottom choice.
pub fn default_comment(game: &Game, last_page: Option<&str>) -> String {
    let text = game
        .selection_texts()
        .last()
        .map(String::as_str)
        .or(last_page)
        .unwrap_or_default();
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(120).collect()
}

/// Smallest slot list the browser offers: occupied slots, in save mode
/// padded to the next multiple of ten with 新規ファイル rows.
pub fn save_rows(entries: &[Entry]) -> u32 {
    let highest = entries.iter().map(|e| e.slot).max().unwrap_or(0);
    ((highest / 10 + 1) * 10).min(MAX_SLOT)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn header_roundtrips_and_read_only_slots_refuse_changes() -> Result<()> {
        let directory = std::env::temp_dir().join(format!(
            "rugp-saves-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let store = SaveStore::new(Some(&directory));
        let header = Header {
            version: 1,
            application: "ゲーム".into(),
            saved_at: 0,
            navigation: "第一話".into(),
            comment: "コメント".into(),
            thumbnail: Thumbnail::from_frame(&[200; 128 * 96 * 4], (128, 96)),
        };
        let path = store.path(3).unwrap();
        write_atomic(&path, &header, b"body")?;
        let mut file = fs::File::open(&path)?;
        assert_eq!(read_header(&mut file)?, header);
        let mut body = Vec::new();
        file.read_to_end(&mut body)?;
        assert_eq!(body, b"body");
        store.set_comment(3, "新しい")?;
        let entry = port_entry(3, &path);
        assert_eq!(entry.comment, "新しい");
        assert!(!entry.important && !entry.broken);
        assert_eq!(entry.thumbnail.unwrap().rgba[..4], [200, 200, 200, 255]);
        store.set_important(3, true)?;
        assert!(port_entry(3, &path).important);
        assert!(store.set_comment(3, "x").is_err());
        assert!(store.delete(3).is_err());
        store.set_important(3, false)?;
        store.delete(3)?;
        assert!(!path.exists());
        fs::write(&path, b"RUGPSLOT\xff\xff\xff\xff")?;
        assert!(port_entry(3, &path).broken);
        fs::remove_dir_all(directory)?;
        Ok(())
    }
    #[test]
    fn save_mode_grows_in_tens() {
        let entry = |slot| broken(slot, Kind::Port, false);
        assert_eq!(save_rows(&[]), 10);
        assert_eq!(save_rows(&[entry(9)]), 10);
        assert_eq!(save_rows(&[entry(10)]), 20);
        assert_eq!(save_rows(&[entry(1000)]), 1000);
    }
}
