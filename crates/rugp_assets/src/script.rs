use crate::{
    RioArchive,
    binary::Reader,
    crypto::{RIO_KEY, read_encrypted},
};
use anyhow::{Context, Result, ensure};

/// A decrypted CRsa stream. Instructions remain serialized class instances,
/// not x86 code; the original DLLs are never loaded.
#[derive(Debug)]
pub struct Script {
    pub object_id: usize,
    pub version: u16,
    pub format: u16,
    pub instruction_count: u32,
    pub memory_size: u32,
    pub data: Vec<u8>,
}

impl Script {
    pub fn load(archive: &RioArchive, id: usize) -> Result<Self> {
        let node = archive.nodes.get(id).context("script ID out of range")?;
        ensure!(
            node.class.name == "CRsa",
            "object {id} is {}, not CRsa",
            node.class.name
        );
        let stored = archive.read_node(id)?;
        let mut r = Reader::new(&stored);
        // Three bytes identify the serialized object. CRsa's envelope has
        // two u16 fields, one byte, and a u32 before its encrypted stream.
        r.skip(3)?;
        let version = r.u16()?;
        r.skip(7)?;
        let data = read_encrypted(&mut r, RIO_KEY)?;
        ensure!(r.remaining() <= 3, "trailing CRsa bytes: {}", r.remaining());
        let mut header = Reader::new(&data);
        let format = header.u16()?;
        header.u16()?;
        header.u32()?;
        header.u32()?;
        let memory_size = header.u32()?;
        let instruction_count = header.u32()?;
        header.u32()?;
        ensure!(
            instruction_count as usize <= data.len() / 2,
            "invalid script instruction count"
        );
        Ok(Self {
            object_id: id,
            version,
            format,
            instruction_count,
            memory_size,
            data,
        })
    }
}
