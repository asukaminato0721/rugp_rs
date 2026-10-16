//! CRimp#3 image properties. Points use signed Q16 coordinates.
use crate::binary::{Reader, decode_text};
use anyhow::{Result, bail, ensure};

/// rvmm 10059a20 reads 原点 before 基準立ち位置Y. The latter is a
/// packed pair of signed shorts. The default anchor is the bottom center.
pub fn anchor(data: &[u8], base: [i32; 2]) -> Result<Option<[i32; 2]>> {
    let mut reader = Reader::new(data);
    reader.skip(3)?;
    let count = reader.count()?;
    ensure!(count <= 4096, "too many image properties");
    let mut fallback = None;
    for _ in 0..count {
        let name = reader.string()?;
        let tag = reader.u32()?;
        if tag == 4 {
            let kind = reader.u16()?;
            ensure!(
                matches!(kind, 0x2f1a | 0x2d6b),
                "unsupported image property type tag"
            );
            reader.u16()?;
            let length = usize::from(reader.u16()?);
            ensure!(length <= 64, "image property type name exceeds limit");
            let type_name = decode_text(reader.bytes(length)?);
            let size = match type_name.as_str() {
                "フラグ" | "バイト" => 1,
                "短整数" | "短正整数" => 2,
                "整数" | "正整数" | "低精度実数" | "色" => 4,
                "点" | "実数" | "高精度実数" => 8,
                "ロングベクトル" => 12,
                _ => bail!("unsupported image property type {type_name}"),
            };
            if name == "原点" && type_name == "点" {
                return Ok(Some([
                    (reader.u32()? as i32).wrapping_sub(base[0]),
                    (reader.u32()? as i32).wrapping_sub(base[1]),
                ]));
            }
            reader.skip(size)?;
        } else {
            match tag & 3 {
                2 => {
                    if name == "基準立ち位置Y" {
                        let point = (tag as i32) >> 2;
                        fallback = Some([
                            (i32::from(point as i16) << 16).wrapping_add(base[0]),
                            (i32::from((point >> 16) as i16) << 16).wrapping_add(base[1]),
                        ]);
                    }
                }
                1 => {
                    reader.string()?;
                }
                _ => bail!("unsupported image property value {tag:#x}"),
            }
        }
    }
    ensure!(reader.remaining() == 0, "trailing image property bytes");
    Ok(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(data: &mut Vec<u8>, value: &str) {
        let (bytes, _, errors) = encoding_rs::SHIFT_JIS.encode(value);
        assert!(!errors);
        data.push(bytes.len() as u8);
        data.extend_from_slice(&bytes);
    }
    fn typed(data: &mut Vec<u8>, name: &str, kind: &str, payload: &[u8]) {
        text(data, name);
        data.extend_from_slice(&4u32.to_le_bytes());
        data.extend_from_slice(&0x2f1au16.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(kind);
        data.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
        data.extend_from_slice(&bytes);
        data.extend_from_slice(payload);
    }
    #[test]
    fn origin_uses_q16_and_subtracts_base_after_vector_property() -> Result<()> {
        let mut data = vec![0, 0, 0, 3, 0];
        text(&mut data, "基準立ち位置Y");
        data.extend_from_slice(&(((200u32 << 16 | 30) << 2) | 2).to_le_bytes());
        typed(&mut data, "向き", "ロングベクトル", &[0; 12]);
        let point = [-5 * 65536 + 123, 100 * 65536 + 456];
        let payload: Vec<u8> = point.into_iter().flat_map(i32::to_le_bytes).collect();
        typed(&mut data, "原点", "点", &payload);
        assert_eq!(
            anchor(&data, [65536, -65536])?,
            Some([point[0] - 65536, point[1] + 65536])
        );
        for length in 0..data.len() {
            assert!(anchor(&data[..length], [0; 2]).is_err());
        }
        Ok(())
    }
    #[test]
    fn standing_point_adds_base_and_empty_map_uses_default() -> Result<()> {
        let mut data = vec![0, 0, 0, 1, 0];
        text(&mut data, "基準立ち位置Y");
        data.extend_from_slice(&(((200u32 << 16 | 30) << 2) | 2).to_le_bytes());
        assert_eq!(
            anchor(&data, [17, -19])?,
            Some([30 * 65536 + 17, 200 * 65536 - 19])
        );
        assert_eq!(anchor(&[0; 5], [0; 2])?, None);
        Ok(())
    }
}
