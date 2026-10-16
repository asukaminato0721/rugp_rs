//! Read-only Windows UUC v19 checkpoints. Runtime objects retain their native
//! payloads. This reader does not treat a thumbnail as a playable checkpoint.
use crate::{binary::Reader, object::ObjectReader};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
};

const KEY: u32 = 0x0732ce9a;
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// The two native encrypted streams. Object payloads share one MFC reference
/// cache inside `runtime`; their order must remain stable when rewriting them.
pub struct NativeArchive {
    pub runtime: Vec<u8>,
    pub metadata: Vec<u8>,
}

impl NativeArchive {
    pub fn read(input: impl Read) -> Result<Self> {
        let mut bytes = Vec::new();
        input.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= MAX_BYTES, "UUC file exceeds size limit");
        let mut encrypted = Reader::new(&bytes);
        let runtime = chunk(&mut encrypted)?;
        let metadata_at = encrypted.position;
        let metadata = chunk(&mut encrypted)?;
        ensure!(encrypted.remaining() == 0, "trailing data in UUC file");
        let mut header = Reader::new(&runtime);
        ensure!(header.u32()? == 19, "unsupported UUC version (expected 19)");
        ensure!(
            header.u32()? as usize == metadata_at,
            "invalid UUC metadata offset"
        );
        Ok(Self { runtime, metadata })
    }

    /// Encode the native UUC container, including its final short-block
    /// checksums. Recompute the metadata offset after runtime records change.
    pub fn write(&self, mut output: impl Write) -> Result<()> {
        ensure!(self.runtime.len() >= 8, "truncated UUC runtime header");
        ensure!(
            self.runtime[..4] == 19u32.to_le_bytes(),
            "unsupported UUC version (expected 19)"
        );
        let runtime_size = encrypted_size(self.runtime.len())?;
        let metadata_size = encrypted_size(self.metadata.len())?;
        ensure!(
            runtime_size + metadata_size <= MAX_BYTES,
            "UUC file exceeds size limit"
        );
        let mut runtime = self.runtime.clone();
        runtime[4..8].copy_from_slice(&(runtime_size as u32).to_le_bytes());
        let mut encoded = Vec::with_capacity(runtime_size + metadata_size);
        encrypt_into(&runtime, &mut encoded);
        encrypt_into(&self.metadata, &mut encoded);
        output.write_all(&encoded)?;
        Ok(())
    }
}

fn encrypted_size(size: usize) -> Result<usize> {
    ensure!(size <= MAX_BYTES, "UUC chunk exceeds size limit");
    Ok(8 + size + size.div_ceil(32) * 2)
}

fn encrypt_into(bytes: &[u8], output: &mut Vec<u8>) {
    output.extend((!(bytes.len() as u32) ^ 0xc92e568b).to_le_bytes());
    // Native engine build 0x431 sets bit 2 in the redundant size word.
    output.extend((((bytes.len() as u32 * 8) | 4) ^ 0xc92e568f).to_le_bytes());
    let mut key = KEY;
    for block in bytes.chunks(32) {
        let checksum = block.iter().enumerate().fold(0u16, |sum, (index, &byte)| {
            sum.wrapping_add(u16::from(byte) * (block.len() - index) as u16)
        });
        for &byte in block {
            output.push(byte ^ key as u8);
            key = !(key
                .wrapping_mul(2)
                .wrapping_add(0xa3b376c9)
                .wrapping_add((key >> 15) & 1));
        }
        output.extend(checksum.to_le_bytes());
    }
}

pub struct NativeSave {
    pub title: String,
    pub registry: String,
    pub game_version: String,
    pub objects: BTreeMap<u64, Vec<u8>>,
    pub navigation: String,
}

// UUC includes a checksum after a short final block. RIO omits that checksum.
fn chunk(input: &mut Reader<'_>) -> Result<Vec<u8>> {
    let size = !(input.u32()? ^ 0xc92e568b) as usize;
    ensure!(
        (input.u32()? ^ 0xc92e568f) >> 3 == size as u32 && size <= MAX_BYTES,
        "invalid UUC chunk length"
    );
    ensure!(
        size + size.div_ceil(32) * 2 <= input.remaining(),
        "truncated UUC chunk"
    );
    let mut key = KEY;
    let mut output = Vec::with_capacity(size);
    while output.len() < size {
        let count = (size - output.len()).min(32);
        let mut checksum = 0u16;
        for index in 0..count {
            let value = input.u8()? ^ key as u8;
            output.push(value);
            checksum = checksum.wrapping_add(u16::from(value) * (count - index) as u16);
            key = !(key
                .wrapping_mul(2)
                .wrapping_add(0xa3b376c9)
                .wrapping_add((key >> 15) & 1));
        }
        ensure!(input.u16()? == checksum, "UUC checksum mismatch");
    }
    Ok(output)
}

impl NativeSave {
    pub fn read(input: impl Read, shift: u32) -> Result<Self> {
        let archive = NativeArchive::read(input)?;
        let data = archive.runtime;
        let metadata = archive.metadata;
        let mut reader = ObjectReader::new(&data);
        ensure!(
            reader.input.u32()? == 19,
            "unsupported UUC version (expected 19)"
        );
        reader.input.u32()?; // NativeArchive checked the encrypted-stream offset.
        ensure!(reader.input.u8()? == 1, "unsupported UUC archive mode");
        ensure!(reader.input.u32()? == 0x431, "unsupported UUC engine build");
        ensure!(reader.input.u32()? == 20, "unsupported UUC archive flags");
        reader.pad(1)?;
        let root = reader.reference_without_count(shift)?;
        ensure!(
            root.class
                .as_ref()
                .is_some_and(|c| c.name == "CObjectOcean"),
            "invalid UUC root"
        );
        ensure!(reader.input.u8()? == 0, "unsupported UUC root state");
        let title = reader.input.string()?;
        let registry = reader.input.string()?;
        let game_version = reader.input.string()?;
        reader.input.string()?; // copyright
        reader.input.string()?; // display title
        // The supported v3 host context has no active modal or extra process.
        // Reject different contexts until their runtime state can be restored.
        const CONTEXT: &[u8] = &[
            1, 0, 0, 0, 3, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 128, 255, 255, 0, 128, 255, 255,
            3, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0,
        ];
        let context_at = reader.input.position;
        ensure!(
            reader.input.bytes(CONTEXT.len())? == CONTEXT,
            "unsupported UUC host context at {context_at:#x}"
        );
        let mut objects = BTreeMap::new();
        while reader.input.remaining() > 0 {
            let named = reader.input.u8()?;
            if named == 0 {
                // Two host managers follow the archive objects. A zero byte
                // terminates this list before the navigation CString.
                let name = reader.input.string()?;
                ensure!(
                    name == "_DevRoot/AbstThreadMediaMng",
                    "unsupported UUC host manager {name}"
                );
                let length = reader.input.u32()? as usize;
                reader.input.skip(length)?;
                ensure!(reader.input.u8()? == 0, "invalid UUC sound manager tag");
                ensure!(
                    reader.input.string()? == "_DevRoot/AbstSoundMng",
                    "missing UUC sound manager"
                );
                let length = reader.input.u32()? as usize;
                reader.input.skip(length)?;
                ensure!(reader.input.u8()? == 0, "invalid UUC object terminator");
                break;
            }
            ensure!(
                named == 1 && reader.input.u16()? == 2,
                "unsupported UUC object identity"
            );
            ensure!(objects.len() < 65536, "too many UUC objects");
            let offset = u64::from(reader.input.u32()?) << shift;
            let length = reader.input.u32()? as usize;
            let payload = reader.input.bytes(length)?.to_vec();
            ensure!(
                objects.insert(offset, payload).is_none(),
                "duplicate UUC object identity"
            );
        }
        let navigation = reader.input.string()?;
        ensure!(reader.input.remaining() == 0, "trailing UUC runtime data");
        // Check both encrypted blocks, including the project identifier in the
        // thumbnail block. The remaining image is not used to restore play.
        let mut metadata = Reader::new(&metadata);
        let application = metadata.string()?;
        ensure!(
            application.ends_with(&registry),
            "UUC metadata project mismatch"
        );
        ensure!(
            metadata.string()? == registry,
            "UUC metadata title mismatch"
        );
        Ok(Self {
            title,
            registry,
            game_version,
            objects,
            navigation,
        })
    }

    /// Decode the empty-call-stack dialogue form of CVm60Thread schema 4.
    /// Other forms need a native stack/argument decoder and are rejected.
    pub fn dialogue_cursor(&self, process: u64, shift: u32) -> Result<(u64, u32)> {
        let data = self
            .objects
            .get(&process)
            .context("UUC process state missing")?;
        let mut reader = ObjectReader::new(data);
        reader.pad(1)?;
        let class = reader.class()?.context("UUC thread class missing")?;
        ensure!(
            class.name == "CVm60Thread" && class.schema == 4 && reader.input.u16()? == 0xc040,
            "unsupported UUC thread"
        );
        let scene = reader.reference_without_count(shift)?;
        ensure!(
            scene.class.as_ref().is_some_and(|c| c.name == "CRsa"),
            "UUC thread scene missing"
        );
        // Vm60 10029820 saves the instruction identity (flags & 0xfffff),
        // rather than its in-memory byte address, immediately after CRsa.
        let instruction_id = reader.input.u32()?;
        // This suffix describes one empty CVmStackFrame and an empty CStdb.
        const EMPTY_STACK: &[u8] = b"\x01\0\0\0\xe8\x03\0\0\xff\0\0\0\xff\xff\x01\0\x0d\0CVmStackFrame\0\0\0\0\0\0\0\0\x01\0\0\0\xff\xff\x01\0\x05\0CStdb\x40\xc0\0";
        ensure!(
            data.ends_with(EMPTY_STACK),
            "UUC call stack or local state is not supported"
        );
        ensure!(
            data.len() - EMPTY_STACK.len() >= reader.input.position,
            "invalid UUC thread cursor"
        );
        Ok((
            scene.offset.context("UUC scene offset missing")?,
            instruction_id,
        ))
    }

    pub fn dialogue_text(&self, window: u64) -> Result<String> {
        let data = self
            .objects
            .get(&window)
            .context("UUC message window missing")?;
        let mut reader = Reader::new(data);
        ensure!(reader.u32()? == 2, "unsupported UUC box version");
        reader.skip(31)?; // CBox and CImgBox state before the retained text
        reader.string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encrypt(bytes: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend((!(bytes.len() as u32) ^ 0xc92e568b).to_le_bytes());
        output.extend((((bytes.len() as u32 * 8) | 4) ^ 0xc92e568f).to_le_bytes());
        let mut key = KEY;
        for block in bytes.chunks(32) {
            let checksum = block.iter().enumerate().fold(0u16, |sum, (i, &b)| {
                sum.wrapping_add(u16::from(b) * (block.len() - i) as u16)
            });
            for &byte in block {
                output.push(byte ^ key as u8);
                key = !(key
                    .wrapping_mul(2)
                    .wrapping_add(0xa3b376c9)
                    .wrapping_add((key >> 15) & 1));
            }
            output.extend(checksum.to_le_bytes());
        }
        output
    }

    #[test]
    fn native_chunks_require_checksums_including_short_final_blocks() -> Result<()> {
        for size in [0, 1, 31, 32, 33, 63, 64, 65, 1000] {
            let bytes: Vec<_> = (0..size).map(|i| (i * 17) as u8).collect();
            let encrypted = encrypt(&bytes);
            let mut reader = Reader::new(&encrypted);
            assert_eq!(chunk(&mut reader)?, bytes);
            assert_eq!(reader.remaining(), 0);
            for end in 0..encrypted.len() {
                assert!(chunk(&mut Reader::new(&encrypted[..end])).is_err());
            }
            if size > 0 {
                let mut corrupt = encrypted.clone();
                let last = corrupt.len() - 1;
                corrupt[last] ^= 1;
                assert!(chunk(&mut Reader::new(&corrupt)).is_err());
            }
            let mut invalid = encrypted.clone();
            invalid[4] ^= 8;
            assert!(chunk(&mut Reader::new(&invalid)).is_err());
        }
        Ok(())
    }

    #[test]
    fn native_writer_updates_offsets_and_matches_the_native_block_encoding() -> Result<()> {
        let (runtime, metadata) = fixture();
        let expected = [encrypt(&runtime), encrypt(&metadata)].concat();
        let mut archive = NativeArchive { runtime, metadata };
        let mut output = Vec::new();
        archive.write(&mut output)?;
        assert_eq!(output, expected);
        for length in [8, 31, 32, 33, 63, 64, 65, 127] {
            archive.runtime.resize(length, 0x5a);
            archive.runtime[4..8].fill(0);
            let mut output = Vec::new();
            archive.write(&mut output)?;
            let decoded = NativeArchive::read(output.as_slice())?;
            assert_eq!(&decoded.runtime[8..], &archive.runtime[8..]);
            assert_eq!(decoded.metadata, archive.metadata);
            assert_eq!(
                u32::from_le_bytes(decoded.runtime[4..8].try_into()?) as usize,
                encrypt(&decoded.runtime).len()
            );
        }
        let mut untouched = Vec::new();
        archive.runtime[0] = 0;
        assert!(archive.write(&mut untouched).is_err());
        assert!(untouched.is_empty());
        archive.runtime[..4].copy_from_slice(&19u32.to_le_bytes());
        archive.metadata.resize(MAX_BYTES, 0);
        assert!(archive.write(&mut untouched).is_err());
        assert!(untouched.is_empty());
        Ok(())
    }

    #[test]
    #[ignore = "set RUGP_TEST_UUC_DIR to the original Vmreg directory"]
    fn retail_native_writer_reproduces_original_files_without_changing_them() -> Result<()> {
        let root = std::path::PathBuf::from(
            std::env::var_os("RUGP_TEST_UUC_DIR").context("UUC directory")?,
        );
        for name in ["vmr00001.uuc", "vmr00002.uuc"] {
            let original = std::fs::read(root.join(name))?;
            let archive = NativeArchive::read(original.as_slice())?;
            let mut rewritten = Vec::new();
            archive.write(&mut rewritten)?;
            assert!(rewritten == original, "native UUC encoding differs for {name}");
            let before = NativeSave::read(original.as_slice(), 0)?;
            let after = NativeSave::read(rewritten.as_slice(), 0)?;
            assert_eq!(after.objects, before.objects);
            assert_eq!(after.navigation, before.navigation);
            assert_eq!(std::fs::read(root.join(name))?, original);
        }
        Ok(())
    }

    fn string(output: &mut Vec<u8>, value: &str) {
        assert!(value.len() < 255 && value.is_ascii());
        output.push(value.len() as u8);
        output.extend(value.as_bytes());
    }

    fn fixture() -> (Vec<u8>, Vec<u8>) {
        let mut data = Vec::new();
        data.extend(19u32.to_le_bytes());
        data.extend(0u32.to_le_bytes()); // second encrypted block offset
        data.push(1);
        data.extend(0x431u32.to_le_bytes());
        data.extend(20u32.to_le_bytes());
        data.extend([0xff, 0xff, 1, 0, 12, 0]);
        data.extend(b"CObjectOcean");
        data.extend([0; 14]); // stored reference, null owner, selector
        data.push(0); // root state
        for value in ["Test Game", "Example", "Script 1", "", "Test"] {
            string(&mut data, value);
        }
        data.extend([
            1, 0, 0, 0, 3, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 128, 255, 255, 0, 128, 255, 255,
            3, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0,
        ]);
        data.extend([1, 2, 0]); // archive object identity
        data.extend(0x1234u32.to_le_bytes());
        data.extend(4u32.to_le_bytes());
        data.extend([1, 2, 3, 4]);
        for name in ["_DevRoot/AbstThreadMediaMng", "_DevRoot/AbstSoundMng"] {
            data.push(0);
            string(&mut data, name);
            data.extend(0u32.to_le_bytes());
        }
        data.push(0);
        string(&mut data, "Chapter");
        let offset = encrypt(&data).len() as u32;
        data[4..8].copy_from_slice(&offset.to_le_bytes());
        let mut metadata = Vec::new();
        string(&mut metadata, "C:/Test/Example");
        string(&mut metadata, "Example");
        (data, metadata)
    }

    #[test]
    fn native_files_reject_invalid_version_offset_truncation_and_trailing_data() -> Result<()> {
        let (data, metadata) = fixture();
        let mut bytes = encrypt(&data);
        bytes.extend(encrypt(&metadata));
        let saved = NativeSave::read(bytes.as_slice(), 0)?;
        assert_eq!(saved.title, "Test Game");
        assert_eq!(saved.registry, "Example");
        assert_eq!(saved.game_version, "Script 1");
        assert_eq!(saved.navigation, "Chapter");
        assert_eq!(saved.objects[&0x1234], [1, 2, 3, 4]);
        assert_eq!(
            NativeSave::read(bytes.as_slice(), 2)?.objects[&0x48d0],
            [1, 2, 3, 4]
        );
        for size in 0..bytes.len() {
            assert!(NativeSave::read(&bytes[..size], 0).is_err());
        }
        bytes.push(0);
        assert!(NativeSave::read(bytes.as_slice(), 0).is_err());
        for (offset, value) in [(0, 20u32), (4, 0), (9, 0), (13, 0)] {
            let mut invalid = data.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            let mut bytes = encrypt(&invalid);
            bytes.extend(encrypt(&metadata));
            assert!(NativeSave::read(bytes.as_slice(), 0).is_err());
        }
        let mut invalid_metadata = metadata;
        *invalid_metadata.last_mut().unwrap() = b'?';
        let mut bytes = encrypt(&data);
        bytes.extend(encrypt(&invalid_metadata));
        assert!(NativeSave::read(bytes.as_slice(), 0).is_err());
        Ok(())
    }
}
