//! The second UUC stream: project names, an inline CS5i thumbnail and slot text.
use super::{MAX_BYTES, reference_writer::ReferenceWriter};
use crate::{binary::Reader, object::ClassId};
use anyhow::{Result, ensure};

#[derive(Debug, PartialEq, Eq)]
pub struct NativeMetadata {
    pub application: String,
    pub registry: String,
    pub thumbnail: Thumbnail,
    pub navigation: String,
    /// Preserve the native 64-bit field. Its time origin is not established.
    pub clock: u64,
    /// Native CString before the summary, empty in the inspected saves.
    pub label: String,
    pub summary: String,
    pub comment: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Thumbnail {
    pub width: u16,
    pub height: u16,
    /// Top-down RGBA, independent of the native bottom-up BGRA storage.
    pub rgba: Vec<u8>,
}

impl Thumbnail {
    fn validate(&self) -> Result<()> {
        let size = usize::from(self.width) * usize::from(self.height) * 4;
        ensure!(
            self.width > 0
                && self.height > 0
                && self.width <= 32767
                && self.height <= 32767
                && size <= MAX_BYTES
                && self.rgba.len() == size,
            "invalid UUC thumbnail dimensions or pixels"
        );
        Ok(())
    }

    fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        self.validate()?;
        writer.data.extend(0x47u32.to_le_bytes());
        writer.data.extend([0; 4]); // crop origin
        for value in [self.width, self.height, self.width, self.height] {
            writer.data.extend(value.to_le_bytes());
        }
        let stride = usize::from(self.width) * 4;
        writer.data.extend((stride as u32).to_le_bytes());
        writer.data.extend((self.rgba.len() as u32).to_le_bytes());
        for row in self.rgba.chunks_exact(stride).rev() {
            for p in row.as_chunks::<4>().0 {
                writer.data.extend([p[2], p[1], p[0], p[3]]);
            }
        }
        Ok(())
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        ensure!(reader.u32()? == 0x47, "unsupported UUC thumbnail format");
        ensure!(reader.u32()? == 0, "cropped UUC thumbnail is not supported");
        let width = reader.u16()?;
        let height = reader.u16()?;
        ensure!(
            reader.u16()? == width && reader.u16()? == height,
            "cropped UUC thumbnail canvas is not supported"
        );
        let stride = reader.u32()? as usize;
        let size = reader.u32()? as usize;
        ensure!(
            width > 0
                && height > 0
                && width <= 32767
                && height <= 32767
                && stride == usize::from(width) * 4
                && size == stride * usize::from(height)
                && size <= MAX_BYTES,
            "invalid UUC thumbnail size"
        );
        let pixels = reader.bytes(size)?;
        let mut rgba = Vec::with_capacity(size);
        for row in pixels.chunks_exact(stride).rev() {
            for p in row.as_chunks::<4>().0 {
                rgba.extend([p[2], p[1], p[0], p[3]]);
            }
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}

impl NativeMetadata {
    pub fn read(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= MAX_BYTES, "UUC metadata exceeds size limit");
        let mut reader = Reader::new(bytes);
        let application = reader.string()?;
        let registry = reader.string()?;
        ensure!(
            reader.bytes(4)? == [1, 5, 0, 0],
            "unsupported UUC thumbnail envelope"
        );
        ensure!(
            reader.bytes(12)? == b"\xff\xff\x02\0\x04\0CS5i\x40\xc0",
            "unsupported UUC thumbnail object"
        );
        let thumbnail = Thumbnail::read(&mut reader)?;
        let navigation = reader.string()?;
        let clock = u64::from_le_bytes(reader.bytes(8)?.try_into()?);
        let label = reader.string()?;
        let summary = reader.string()?;
        let comment = reader.string()?;
        ensure!(reader.remaining() == 0, "trailing UUC metadata");
        Ok(Self {
            application,
            registry,
            thumbnail,
            navigation,
            clock,
            label,
            summary,
            comment,
        })
    }

    pub fn write(&self) -> Result<Vec<u8>> {
        self.thumbnail.validate()?;
        let mut writer = ReferenceWriter::new(0)?;
        writer.string(&self.application)?;
        writer.string(&self.registry)?;
        writer.data.extend([1, 5, 0, 0]);
        writer.inline(
            &ClassId {
                name: "CS5i".into(),
                schema: 2,
            },
            |writer| self.thumbnail.write(writer),
        )?;
        writer.string(&self.navigation)?;
        writer.data.extend(self.clock.to_le_bytes());
        writer.string(&self.label)?;
        writer.string(&self.summary)?;
        writer.string(&self.comment)?;
        ensure!(
            writer.data.len() <= MAX_BYTES,
            "UUC metadata exceeds size limit"
        );
        Ok(writer.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_metadata_encodes_bottom_up_bgra_and_rejects_incomplete_pixels() -> Result<()> {
        let mut metadata = NativeMetadata {
            application: "age\\ゲーム".into(),
            registry: "ゲーム".into(),
            thumbnail: Thumbnail {
                width: 1,
                height: 2,
                rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
            },
            navigation: "Chapter".into(),
            clock: 0x123456789abcdef,
            label: "Slot".into(),
            summary: "Text".into(),
            comment: "comment={Text}".into(),
        };
        let bytes = metadata.write()?;
        let pixel_at = bytes
            .windows(8)
            .position(|b| b == [7, 6, 5, 8, 3, 2, 1, 4])
            .unwrap();
        assert_eq!(NativeMetadata::read(&bytes)?, metadata);
        for size in [0, pixel_at, pixel_at + 7, bytes.len() - 1] {
            assert!(NativeMetadata::read(&bytes[..size]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(NativeMetadata::read(&trailing).is_err());
        metadata.thumbnail.rgba.pop();
        assert!(metadata.write().is_err());
        Ok(())
    }

    #[test]
    #[ignore = "set RUGP_TEST_UUC_DIR to the original Vmreg directory"]
    fn retail_thumbnail_and_slot_metadata_reencode_without_changes() -> Result<()> {
        let root = std::path::PathBuf::from(
            std::env::var_os("RUGP_TEST_UUC_DIR")
                .ok_or_else(|| anyhow::anyhow!("UUC directory missing"))?,
        );
        for name in ["vmr00001.uuc", "vmr00002.uuc"] {
            let original = std::fs::read(root.join(name))?;
            let archive = super::super::NativeArchive::read(original.as_slice())?;
            let metadata = NativeMetadata::read(&archive.metadata)?;
            assert_eq!(
                (metadata.thumbnail.width, metadata.thumbnail.height),
                (64, 48)
            );
            assert_eq!(metadata.write()?, archive.metadata);
            assert_eq!(std::fs::read(root.join(name))?, original);
        }
        Ok(())
    }
}
