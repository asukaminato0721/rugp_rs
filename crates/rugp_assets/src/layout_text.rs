//! CLayoutTextImg resource data used by the ending staff rolls. This is not
//! a Windows font-cache bitmap or a serialized live text-window snapshot.
use crate::object::{ObjectReader, ObjectRef};
use anyhow::{Result, ensure};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct LayoutText {
    pub background_style: u32,
    pub background_color: u32,
    pub text_style: u32,
    pub font_faces: [String; 2],
    pub font_sizes: [i32; 2],
    pub font_options: u32,
    pub secondary_style: u32,
    /// Native cached extent; the loader resets the first two components.
    pub rectangle: [i32; 4],
    pub layout_options: u32,
    pub text: String,
    pub images: Vec<ObjectRef>,
}
impl LayoutText {
    /// rvmm 10051000 reads this version-2 body. Its final CRefArray has a
    /// type declaration (UnivUI 10023f50), not a regular object reference.
    /// The declared CSbm element type shares the normal reference context.
    pub fn parse(data: &[u8], schema: u16, shift: u32) -> Result<Self> {
        ensure!(schema == 1, "unsupported CLayoutTextImg schema {schema}");
        ensure!(data.len() <= 16 * 1024 * 1024, "oversized layout text");
        let mut r = ObjectReader::new(data);
        r.input.skip(3)?;
        r.scrambled = true;
        ensure!(r.input.u16()? == 2, "unsupported layout-text version");
        let background_style = r.input.u32()?;
        let background_color = r.input.u32()?;
        let text_style = r.input.u32()?;
        let font_faces = [r.input.string()?, r.input.string()?];
        let font_sizes = [r.input.u32()? as i32, r.input.u32()? as i32];
        let font_options = r.input.u32()?;
        let secondary_style = r.input.u32()?;
        let rectangle = [
            r.input.u32()? as i32,
            r.input.u32()? as i32,
            r.input.u32()? as i32,
            r.input.u32()? as i32,
        ];
        let layout_options = r.input.u32()?;
        let text = r.input.string()?;
        ensure!(r.input.u8()? == 1, "unsupported layout image-array version");
        ensure!(
            r.input.u16()? == 0x1e57,
            "unsupported layout image-array type encoding"
        );
        let class = r.class()?;
        ensure!(
            class.is_some_and(|c| c.name == "CSbm" && c.schema == 1),
            "unsupported layout image-array element type"
        );
        let count = r.input.count()?;
        ensure!(
            count <= 4096 && count <= r.input.remaining() / 2,
            "invalid layout image-array count"
        );
        let mut images = Vec::with_capacity(count);
        for _ in 0..count {
            images.push(r.reference(shift)?);
        }
        ensure!(r.input.remaining() == 0, "trailing layout-text data");
        Ok(Self {
            background_style,
            background_color,
            text_style,
            font_faces,
            font_sizes,
            font_options,
            secondary_style,
            rectangle,
            layout_options,
            text,
            images,
        })
    }
}

/// Serialized defaults for CTextStaffrollSeq#2 (rvmm 10051e10).
#[derive(Debug, Clone, Serialize)]
pub struct TextRollDefaults {
    pub sources: [ObjectRef; 2],
    pub rectangle: [i16; 4],
    pub parameters: [i32; 7],
}
impl TextRollDefaults {
    pub fn parse(data: &[u8], schema: u16, shift: u32) -> Result<Self> {
        ensure!(schema == 2, "unsupported CTextStaffrollSeq schema {schema}");
        ensure!(data.len() <= 1024 * 1024, "oversized text sequence");
        let mut r = ObjectReader::new(data);
        r.input.skip(3)?;
        r.scrambled = true;
        let sources = [r.reference(shift)?, r.reference(shift)?];
        let rectangle = [
            r.input.u16()? as i16,
            r.input.u16()? as i16,
            r.input.u16()? as i16,
            r.input.u16()? as i16,
        ];
        let mut parameters = [0; 7];
        for p in &mut parameters {
            *p = r.input.u32()? as i32;
        }
        ensure!(r.input.remaining() == 0, "trailing text-sequence data");
        Ok(Self {
            sources,
            rectangle,
            parameters,
        })
    }
}
