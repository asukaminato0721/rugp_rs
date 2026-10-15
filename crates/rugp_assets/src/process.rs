//! The process ocean's five root references (schema 2, embedded version 33).
use crate::object::{ObjectReader, ObjectRef};
use anyhow::{Result, ensure};
pub struct ProcessOcean {
    pub references: [ObjectRef; 5],
}
impl ProcessOcean {
    pub fn parse(data: &[u8], shift: u32) -> Result<Self> {
        let mut reader = ObjectReader::new(data);
        reader.input.skip(3)?;
        reader.scrambled = true;
        reader.pad(3)?;
        let version = reader.input.u32()?;
        ensure!(version == 33, "unsupported process ocean version {version}");
        let references = [
            reader.reference(shift)?,
            reader.reference(shift)?,
            reader.reference(shift)?,
            reader.reference(shift)?,
            reader.reference(shift)?,
        ];
        ensure!(reader.input.remaining() == 0, "trailing process ocean data");
        Ok(Self { references })
    }
}
