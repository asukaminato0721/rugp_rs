//! CPmBgm/CPmEffect serialized source wrappers.
use crate::object::{ObjectReader, ObjectRef};
use anyhow::{Result, ensure};
pub struct Sample {
    pub mode: u16,
    pub source: ObjectRef,
    pub name: String,
    pub flags: u16,
}
impl Sample {
    pub fn parse(data: &[u8], shift: u32) -> Result<Self> {
        let mut r = ObjectReader::new(data);
        r.input.skip(3)?;
        r.scrambled = true;
        r.pad(3)?;
        let mode = r.input.u16()?;
        let source = r.reference(shift)?;
        let name = r.input.string()?;
        let flags = r.input.u16()?;
        ensure!(r.input.remaining() == 0, "trailing CPm sound wrapper bytes");
        Ok(Self {
            mode,
            source,
            name,
            flags,
        })
    }
}
