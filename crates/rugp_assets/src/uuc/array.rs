//! Built-in scalar arrays use type metadata, rather than object references.
use super::reference_writer::ReferenceWriter;
use anyhow::{Result, ensure};

pub enum ScalarArray<'a> {
    Empty,
    Bytes(&'a [u8]),
    Integers(&'a [i32]),
    Reals(&'a [f64]),
}

impl ScalarArray<'_> {
    /// UnivUI ordinal 699. Names select built-in types. Their schema is read
    /// but ignored by the native type resolver (10024680).
    pub fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        let (name, count, width): (&[u8], usize, usize) = match self {
            Self::Empty => (b"", 0, 0),
            Self::Bytes(values) => (b"\x83\x6f\x83\x43\x83\x67", values.len(), 1),
            Self::Integers(values) => (b"\x90\xae\x90\x94", values.len(), 4),
            Self::Reals(values) => {
                ensure!(
                    values.iter().all(|value| value.is_finite()),
                    "nonfinite UUC array value"
                );
                (b"\x8e\xc0\x90\x94", values.len(), 8)
            }
        };
        ensure!(
            count <= super::MAX_BYTES / width.max(1),
            "oversized UUC scalar array"
        );
        writer.data.push(1);
        if name.is_empty() {
            writer.data.extend([0x57, 0x1e, 0, 0]); // Null type descriptor
        } else {
            writer.data.extend(0x2d6bu16.to_le_bytes());
            writer.data.extend(0u16.to_le_bytes());
            writer.data.extend((name.len() as u16).to_le_bytes());
            writer.data.extend(name);
        }
        if count < 65535 {
            writer.data.extend((count as u16).to_le_bytes());
        } else {
            writer.data.extend(u16::MAX.to_le_bytes());
            writer.data.extend((count as u32).to_le_bytes());
        }
        match self {
            Self::Empty => {}
            Self::Bytes(values) => writer.data.extend(*values),
            Self::Integers(values) => {
                for value in *values {
                    writer.data.extend(value.to_le_bytes());
                }
            }
            Self::Reals(values) => {
                for value in *values {
                    writer.data.extend(value.to_le_bytes());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonfinite_arrays_fail_before_writing_a_header() -> Result<()> {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut writer = ReferenceWriter::new(0)?;
            assert!(ScalarArray::Reals(&[value]).write(&mut writer).is_err());
            assert!(writer.data.is_empty());
        }
        Ok(())
    }

}
