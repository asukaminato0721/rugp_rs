//! Native tagged values, used by CVmVarObj and stack argument vectors.
use super::reference_writer::ReferenceWriter;
use crate::{binary::Reader, object::ObjectRef};
use anyhow::{Result, ensure};

pub enum NativeValue<'a> {
    Integer(i32),
    Real(f64),
    Text(&'a str),
    Object(Option<&'a ObjectRef>),
}

#[derive(Debug, PartialEq)]
pub enum ScalarValue {
    Null,
    Integer(i32),
    Real(f64),
    Text(String),
}

/// Read values that do not need the enclosing stream's object cache. Return
/// None for object identities and runtime type bindings. Their payloads must
/// be read in the original reference context, not as independent records.
pub fn read_scalar(data: &[u8]) -> Result<Option<ScalarValue>> {
    let mut reader = Reader::new(data);
    let tag = reader.u32()?;
    let value = if tag == 4 {
        let kind = reader.u16()?;
        if !matches!(kind, 0x2f1a | 0x2d6b) {
            return Ok(None);
        }
        reader.u16()?; // The built-in type resolver ignores the schema.
        let length = reader.u16()? as usize;
        ensure!(length < 64, "oversized UUC scalar type name");
        let name = crate::binary::decode_text(reader.bytes(length)?);
        match name.as_str() {
            "フラグ" | "バイト" => ScalarValue::Integer(i32::from(reader.u8()?)),
            "短整数" => ScalarValue::Integer(i32::from(reader.u16()? as i16)),
            "短正整数" => ScalarValue::Integer(i32::from(reader.u16()?)),
            "整数" | "正整数" | "色" => ScalarValue::Integer(reader.u32()? as i32),
            "低精度実数" => ScalarValue::Real(f64::from(f32::from_bits(reader.u32()?))),
            "実数" | "高精度実数" => {
                ScalarValue::Real(f64::from_le_bytes(reader.bytes(8)?.try_into()?))
            }
            _ => return Ok(None),
        }
    } else {
        match tag & 3 {
            0 if reader.remaining() == 2 && reader.u16()? == 0 => ScalarValue::Null,
            1 => ScalarValue::Text(reader.string()?),
            2 => ScalarValue::Integer((tag as i32) >> 2),
            _ => return Ok(None),
        }
    };
    ensure!(reader.remaining() == 0, "trailing UUC scalar bytes");
    if let ScalarValue::Real(value) = value {
        ensure!(value.is_finite(), "nonfinite UUC real value");
    }
    Ok(Some(value))
}

impl NativeValue<'_> {
    /// UnivUI ordinal 297. The low two bits select the storage form. Pointer
    /// bits in native object/text tags carry no persistent object identity.
    pub fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        match self {
            Self::Integer(value) => {
                if (-536_870_912..=536_870_911).contains(value) {
                    writer.data.extend(((*value as u32) << 2 | 2).to_le_bytes());
                } else {
                    typed(writer, b"\x90\xae\x90\x94", &value.to_le_bytes());
                }
            }
            Self::Real(value) => {
                ensure!(value.is_finite(), "nonfinite UUC real value");
                typed(writer, b"\x8e\xc0\x90\x94", &value.to_le_bytes());
            }
            Self::Text(value) => {
                writer.data.extend(1u32.to_le_bytes());
                writer.string(value)?;
            }
            Self::Object(reference) => {
                writer.data.extend(0u32.to_le_bytes());
                match reference {
                    Some(reference) => writer.reference(reference)?,
                    None => writer.cached(0)?,
                }
            }
        }
        Ok(())
    }
}

/// The native built-in type reader (UnivUI 10024680) uses the Shift-JIS name
/// to resolve its descriptor. It reads but does not use the 16-bit schema.
fn typed(writer: &mut ReferenceWriter, name: &[u8], data: &[u8]) {
    writer.data.extend(4u32.to_le_bytes());
    writer.data.extend(0x2f1au16.to_le_bytes());
    writer.data.extend(0u16.to_le_bytes());
    writer.data.extend((name.len() as u16).to_le_bytes());
    writer.data.extend(name);
    writer.data.extend(data);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Result<Vec<(&'static str, Vec<u8>)>> {
        let mut output = Vec::new();
        for (name, value) in [
            ("value-positive.bin", NativeValue::Integer(37)),
            ("value-negative.bin", NativeValue::Integer(-500)),
            ("value-null.bin", NativeValue::Object(None)),
            ("value-text.bin", NativeValue::Text("")),
            ("value-integer-min.bin", NativeValue::Integer(i32::MIN)),
            ("value-integer-max.bin", NativeValue::Integer(i32::MAX)),
            ("value-real.bin", NativeValue::Real(1.5)),
        ] {
            let mut writer = ReferenceWriter::new(0)?;
            value.write(&mut writer)?;
            output.push((name, writer.data));
        }
        Ok(output)
    }

    #[test]
    fn integers_preserve_sign_and_use_typed_storage_outside_tagged_range() -> Result<()> {
        for value in [-536_870_912, -500, 0, 37, 536_870_911] {
            let mut writer = ReferenceWriter::new(0)?;
            NativeValue::Integer(value).write(&mut writer)?;
            let word = u32::from_le_bytes(writer.data.try_into().unwrap());
            assert_eq!((word as i32) >> 2, value);
            assert_eq!(word & 3, 2);
        }
        for value in [i32::MIN, -536_870_913, 536_870_912, i32::MAX] {
            let mut writer = ReferenceWriter::new(0)?;
            NativeValue::Integer(value).write(&mut writer)?;
            assert_eq!(&writer.data[..4], &4u32.to_le_bytes());
            assert_eq!(&writer.data[writer.data.len() - 4..], &value.to_le_bytes());
        }
        for value in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let mut writer = ReferenceWriter::new(0)?;
            assert!(NativeValue::Real(value).write(&mut writer).is_err());
            assert!(writer.data.is_empty());
        }
        Ok(())
    }

    #[test]
    fn scalar_reader_handles_native_pointer_tags_and_rejects_truncation() -> Result<()> {
        for (native, expected) in [
            (
                NativeValue::Integer(i32::MIN),
                ScalarValue::Integer(i32::MIN),
            ),
            (
                NativeValue::Integer(i32::MAX),
                ScalarValue::Integer(i32::MAX),
            ),
            (NativeValue::Integer(-500), ScalarValue::Integer(-500)),
            (NativeValue::Real(-1.5), ScalarValue::Real(-1.5)),
            (NativeValue::Text("設定"), ScalarValue::Text("設定".into())),
            (NativeValue::Object(None), ScalarValue::Null),
        ] {
            let mut writer = ReferenceWriter::new(0)?;
            native.write(&mut writer)?;
            assert_eq!(read_scalar(&writer.data)?, Some(expected));
            if matches!(native, NativeValue::Text(_) | NativeValue::Object(_)) {
                let tag = u32::from_le_bytes(writer.data[..4].try_into()?);
                writer.data[..4].copy_from_slice(&(0x12345678 | tag).to_le_bytes());
                assert!(read_scalar(&writer.data)?.is_some());
            }
            for length in 0..writer.data.len() {
                // Incomplete object references belong to the shared-cache
                // reader. Other scalar payloads must fail on truncation.
                if !matches!(native, NativeValue::Object(_)) {
                    assert!(read_scalar(&writer.data[..length]).is_err());
                }
            }
            writer.data.push(0);
            if !matches!(native, NativeValue::Object(_)) {
                assert!(read_scalar(&writer.data).is_err());
            }
        }
        assert_eq!(read_scalar(&[0, 0, 0, 0, 7, 0])?, None);
        Ok(())
    }

    #[test]
    #[ignore = "set RUGP_NATIVE_ORACLE_DIR to files generated by dev/native_uuc/probe.c"]
    fn retail_variant_writers_match_original_dll_serializer() -> Result<()> {
        let directory = std::path::PathBuf::from(
            std::env::var_os("RUGP_NATIVE_ORACLE_DIR")
                .ok_or_else(|| anyhow::anyhow!("native oracle directory missing"))?,
        );
        for (name, bytes) in samples()? {
            let mut expected = std::fs::read(directory.join(name))?;
            let tag = u32::from_le_bytes(expected[..4].try_into()?);
            // Native object/text tags contain process addresses. The loader
            // uses only their low two bits and then reads the payload.
            if tag != 4 && tag & 3 <= 1 {
                expected[..4].copy_from_slice(&(tag & 3).to_le_bytes());
            }
            assert_eq!(bytes, expected, "{name}");
            assert!(read_scalar(&expected)?.is_some(), "{name}");
        }
        Ok(())
    }
}
