//! Portable slots in the already locked per-game user-data directory. Original
//! Windows saves are neither read nor overwritten. Failed writes leave the old
//! slot intact; bounded, versioned gzip JSON is checked before runtime mutation.
use super::{snapshot::GameSnapshot, *};
use crate::{audio::AudioResources, audio_state::snapshot::PlaybackResume};
use flate2::{Compression, bufread::GzDecoder, write::GzEncoder};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Cursor, Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

const MAGIC: &[u8; 8] = b"RUGPRS\0\x01";
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_JSON: u64 = 256 * 1024 * 1024;
static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);
pub const SLOT_COUNT: u8 = 10;

#[derive(Debug)]
pub struct SlotInfo {
    pub slot: u8,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
}
fn path(directory: &Path, slot: u8) -> Result<PathBuf> {
    ensure!(slot < SLOT_COUNT, "save slot must be between 0 and 9");
    Ok(directory.join(format!("slot-{slot}.rugpsave")))
}
impl Game {
    pub fn save_slots(&self) -> Result<Vec<SlotInfo>> {
        let directory = self
            .user_data_dir()
            .context("persistent user data is not enabled")?;
        let mut slots = Vec::new();
        for slot in 0..SLOT_COUNT {
            match fs::metadata(path(directory, slot)?) {
                Ok(info) => {
                    ensure!(info.is_file(), "save slot is not a regular file");
                    slots.push(SlotInfo {
                        slot,
                        bytes: info.len(),
                        modified: info.modified().ok(),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(slots)
    }
    pub fn save_slot(&self, slot: u8) -> Result<()> {
        let directory = self
            .user_data_dir()
            .context("persistent user data is not enabled")?;
        let destination = path(directory, slot)?;
        let snapshot = self.snapshot()?;
        let temporary = directory.join(format!(
            "slot-{slot}-{}-{}.pending",
            std::process::id(),
            TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        let result = (|| -> Result<()> {
            snapshot.write(&mut file)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &destination)?;
            #[cfg(unix)]
            File::open(directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            // Only this invocation's freshly-created temporary is removed.
            // A failed rename never removes/truncates the existing slot.
            let _ = fs::remove_file(&temporary);
        }
        result
    }
    pub fn load_slot(
        &mut self,
        slot: u8,
        resources: &mut AudioResources,
    ) -> Result<Vec<PlaybackResume>> {
        let directory = self
            .user_data_dir()
            .context("persistent user data is not enabled")?;
        let file = File::open(path(directory, slot)?)?;
        ensure!(
            file.metadata()?.is_file(),
            "save slot is not a regular file"
        );
        let snapshot = GameSnapshot::read(file)?;
        self.restore_snapshot(&snapshot, resources)
    }
}
impl GameSnapshot {
    pub fn write(&self, output: impl Write) -> Result<()> {
        write_wire(self, output, MAX_JSON)
    }
    pub fn read(input: impl Read) -> Result<Self> {
        read_wire(input, MAX_JSON)
    }
}
struct LimitedWriter<W> {
    inner: W,
    remaining: u64,
}
impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(std::io::Error::other("save payload exceeds size limit"));
        }
        let count = self.inner.write(bytes)?;
        self.remaining -= count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
fn write_wire(value: &impl Serialize, output: impl Write, json_limit: u64) -> Result<()> {
    let mut output = LimitedWriter {
        inner: output,
        remaining: MAX_FILE,
    };
    output.write_all(MAGIC)?;
    let encoder = GzEncoder::new(output, Compression::fast());
    let mut payload = LimitedWriter {
        // JSON emits many one-byte writes (pixel/reference arrays). Batch
        // those before deflation, otherwise saving stalls on codec overhead.
        inner: BufWriter::with_capacity(64 * 1024, encoder),
        remaining: json_limit,
    };
    serde_json::to_writer(&mut payload, value)?;
    payload.inner.flush()?;
    payload
        .inner
        .into_inner()
        .map_err(|e| e.into_error())?
        .finish()?
        .flush()?;
    Ok(())
}
fn read_wire<T: DeserializeOwned>(input: impl Read, json_limit: u64) -> Result<T> {
    let mut bytes = Vec::new();
    input.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE,
        "save file exceeds size limit"
    );
    ensure!(
        bytes.starts_with(MAGIC),
        "invalid/unsupported portable save format"
    );
    let body = &bytes[MAGIC.len()..];
    let mut decoder = GzDecoder::new(Cursor::new(body));
    let mut json = Vec::new();
    (&mut decoder).take(json_limit + 1).read_to_end(&mut json)?;
    ensure!(
        json.len() as u64 <= json_limit,
        "expanded save exceeds size limit"
    );
    ensure!(
        decoder.into_inner().position() == body.len() as u64,
        "unexpected trailing data in save"
    );
    Ok(serde_json::from_slice(&json)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_format_roundtrip_rejects_truncation_trailing_bytes_crc_and_expansion() -> Result<()>
    {
        let value = vec!["synthetic".repeat(500)];
        let mut bytes = Vec::new();
        write_wire(&value, &mut bytes, MAX_JSON)?;
        let restored: Vec<String> = read_wire(bytes.as_slice(), MAX_JSON)?;
        assert_eq!(restored, value);
        for end in 0..bytes.len() {
            assert!(read_wire::<Vec<String>>(&bytes[..end], MAX_JSON).is_err());
        }
        let mut invalid = bytes.clone();
        invalid.push(0);
        assert!(read_wire::<Vec<String>>(invalid.as_slice(), MAX_JSON).is_err());
        let mut invalid = bytes.clone();
        let at = invalid.len() - 8;
        invalid[at] ^= 1;
        assert!(read_wire::<Vec<String>>(invalid.as_slice(), MAX_JSON).is_err());
        assert!(read_wire::<Vec<String>>(bytes.as_slice(), 32).is_err());
        assert!(write_wire(&value, Vec::new(), 32).is_err());
        let mut invalid = bytes.clone();
        invalid[7] += 1;
        assert!(read_wire::<Vec<String>>(invalid.as_slice(), MAX_JSON).is_err());
        assert!(path(Path::new("."), SLOT_COUNT).is_err());
        Ok(())
    }
}
