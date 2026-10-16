//! Read-only Windows UUC v19 checkpoints. Runtime objects retain their native
//! payloads. This reader does not treat a thumbnail as a playable checkpoint.
use crate::{binary::Reader, object::ObjectReader};
use anyhow::{Context, Result, ensure};
use std::{collections::BTreeMap, io::Read};

const KEY: u32 = 0x0732ce9a;
const MAX_BYTES: usize = 16 * 1024 * 1024;

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
            key = !(key.wrapping_mul(2).wrapping_add(0xa3b376c9).wrapping_add((key >> 15) & 1));
        }
        ensure!(input.u16()? == checksum, "UUC checksum mismatch");
    }
    Ok(output)
}

impl NativeSave {
    pub fn read(input: impl Read, shift: u32) -> Result<Self> {
        let mut bytes = Vec::new();
        input.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= MAX_BYTES, "UUC file exceeds size limit");
        let mut encrypted = Reader::new(&bytes);
        let data = chunk(&mut encrypted)?;
        let metadata_at = encrypted.position;
        let metadata = chunk(&mut encrypted)?;
        ensure!(encrypted.remaining() == 0, "trailing data in UUC file");
        let mut reader = ObjectReader::new(&data);
        ensure!(reader.input.u32()? == 19, "unsupported UUC version (expected 19)");
        ensure!(reader.input.u32()? as usize == metadata_at, "invalid UUC metadata offset");
        ensure!(reader.input.u8()? == 1, "unsupported UUC archive mode");
        ensure!(reader.input.u32()? == 0x431, "unsupported UUC engine build");
        ensure!(reader.input.u32()? == 20, "unsupported UUC archive flags");
        reader.pad(1)?;
        let root = reader.reference_without_count(shift)?;
        ensure!(root.class.as_ref().is_some_and(|c| c.name == "CObjectOcean"), "invalid UUC root");
        ensure!(reader.input.u8()? == 0, "unsupported UUC root state");
        let title = reader.input.string()?;
        let registry = reader.input.string()?;
        let game_version = reader.input.string()?;
        reader.input.string()?; // copyright
        reader.input.string()?; // display title
        // The supported v3 host context has no active modal or extra process.
        // Reject different contexts until their runtime state can be restored.
        const CONTEXT: &[u8] = &[
            1,0,0,0, 3,0,0,0, 0,0,2,0,0,0,0,0,0,0,128,255,255,0,128,255,255,
            3,0,0,0,0,0,0,0,0,1,0,0,1,0,0,0,0,0,
        ];
        let context_at = reader.input.position;
        ensure!(reader.input.bytes(CONTEXT.len())? == CONTEXT, "unsupported UUC host context at {context_at:#x}");
        let mut objects = BTreeMap::new();
        while reader.input.remaining() > 0 {
            let named = reader.input.u8()?;
            if named == 0 {
                // Two host managers follow the archive objects. A zero byte
                // terminates this list before the navigation CString.
                let name = reader.input.string()?;
                ensure!(name == "_DevRoot/AbstThreadMediaMng", "unsupported UUC host manager {name}");
                let length = reader.input.u32()? as usize;
                reader.input.skip(length)?;
                ensure!(reader.input.u8()? == 0, "invalid UUC sound manager tag");
                ensure!(reader.input.string()? == "_DevRoot/AbstSoundMng", "missing UUC sound manager");
                let length = reader.input.u32()? as usize;
                reader.input.skip(length)?;
                ensure!(reader.input.u8()? == 0, "invalid UUC object terminator");
                break;
            }
            ensure!(named == 1 && reader.input.u16()? == 2, "unsupported UUC object identity");
            ensure!(objects.len() < 65536, "too many UUC objects");
            let offset = u64::from(reader.input.u32()?) << shift;
            let length = reader.input.u32()? as usize;
            let payload = reader.input.bytes(length)?.to_vec();
            ensure!(objects.insert(offset, payload).is_none(), "duplicate UUC object identity");
        }
        let navigation = reader.input.string()?;
        ensure!(reader.input.remaining() == 0, "trailing UUC runtime data");
        // Check both encrypted blocks, including the project identifier in the
        // thumbnail block. The remaining image is not used to restore play.
        let mut metadata = Reader::new(&metadata);
        let application = metadata.string()?;
        ensure!(application.ends_with(&registry), "UUC metadata project mismatch");
        ensure!(metadata.string()? == registry, "UUC metadata title mismatch");
        Ok(Self { title, registry, game_version, objects, navigation })
    }

    /// Decode the empty-call-stack dialogue form of CVm60Thread schema 4.
    /// Other forms need a native stack/argument decoder and are rejected.
    pub fn dialogue_cursor(&self, process: u64, shift: u32) -> Result<(u64, u32)> {
        let data = self.objects.get(&process).context("UUC process state missing")?;
        let mut reader = ObjectReader::new(data);
        reader.pad(1)?;
        let class = reader.class()?.context("UUC thread class missing")?;
        ensure!(class.name == "CVm60Thread" && class.schema == 4 && reader.input.u16()? == 0xc040, "unsupported UUC thread");
        let scene = reader.reference_without_count(shift)?;
        ensure!(scene.class.as_ref().is_some_and(|c| c.name == "CRsa"), "UUC thread scene missing");
        // Vm60 10029820 saves the instruction identity (flags & 0xfffff),
        // rather than its in-memory byte address, immediately after CRsa.
        let instruction_id = reader.input.u32()?;
        // This suffix describes one empty CVmStackFrame and an empty CStdb.
        const EMPTY_STACK: &[u8] = b"\x01\0\0\0\xe8\x03\0\0\xff\0\0\0\xff\xff\x01\0\x0d\0CVmStackFrame\0\0\0\0\0\0\0\0\x01\0\0\0\xff\xff\x01\0\x05\0CStdb\x40\xc0\0";
        ensure!(data.ends_with(EMPTY_STACK), "UUC call stack or local state is not supported");
        ensure!(data.len() - EMPTY_STACK.len() >= reader.input.position, "invalid UUC thread cursor");
        Ok((scene.offset.context("UUC scene offset missing")?, instruction_id))
    }

    pub fn dialogue_text(&self, window: u64) -> Result<String> {
        let data = self.objects.get(&window).context("UUC message window missing")?;
        let mut reader = Reader::new(data);
        ensure!(reader.u32()? == 2, "unsupported UUC box version");
        reader.skip(31)?; // CBox and CImgBox state before the retained text
        reader.string()
    }
}
