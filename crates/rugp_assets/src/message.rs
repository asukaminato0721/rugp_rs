//! Native message-window resource metadata (not a saved UI/history snapshot).
use crate::object::{ObjectReader, ObjectRef};
use anyhow::{Result, ensure};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct MessageBox {
    pub order: [u16; 2],
    pub flags: u32,
    pub kind: u8,
    /// Native CBox fields at offsets 0x40, 0x48, 0x50, 0x58 and 0x60.
    /// The third is the parent-relative (x, y, width, height).
    pub rectangles: [[i16; 4]; 5],
    pub initial_text: String,
    pub children: Vec<ObjectRef>,
    pub reserved: [u16; 2],
    pub surface_flags: u32,
    pub surface_parameters: [i32; 3],
    pub content_rect: [i16; 4],
    pub spacing: [i16; 2],
    pub surface: ObjectRef,
    pub label_version: u32,
    pub label_flags: u16,
    pub label_mode: u8,
    pub frame: ObjectRef,
    pub layout_version: u8,
    pub layout: u32,
}

fn rectangle(r: &mut ObjectReader<'_>) -> Result<[i16; 4]> {
    Ok([
        r.input.u16()? as i16,
        r.input.u16()? as i16,
        r.input.u16()? as i16,
        r.input.u16()? as i16,
    ])
}

impl MessageBox {
    /// Vm60 CMsgSelBox#15 inherits the CViewLabel resource serializer.
    /// Its separate save-state serializer has a different field order and is
    /// deliberately not accepted here.
    pub fn parse(data: &[u8], schema: u16, shift: u32) -> Result<Self> {
        ensure!(schema == 15, "unsupported CMsgSelBox schema {schema}");
        ensure!(data.len() <= 16 * 1024 * 1024, "oversized message window");
        let mut r = ObjectReader::new(data);
        r.input.skip(3)?;
        r.scrambled = true;
        r.pad(3)?;
        let order = [r.input.u16()?, r.input.u16()?];
        let flags = r.input.u32()?;
        let kind = r.input.u8()?;
        let rectangles = [
            rectangle(&mut r)?,
            rectangle(&mut r)?,
            rectangle(&mut r)?,
            rectangle(&mut r)?,
            rectangle(&mut r)?,
        ];
        let initial_text = r.input.string()?;
        let count = r.input.count()?;
        ensure!(
            count <= 4096 && count <= r.input.remaining() / 2,
            "invalid message-window child count"
        );
        let mut children = Vec::with_capacity(count);
        for _ in 0..count {
            children.push(r.reference(shift)?);
        }
        let reserved = [r.input.u16()?, r.input.u16()?];
        let surface_flags = r.input.u32()?;
        let surface_parameters = [
            r.input.u32()? as i32,
            r.input.u32()? as i32,
            r.input.u32()? as i32,
        ];
        let content_rect = rectangle(&mut r)?;
        let spacing = [r.input.u16()? as i16, r.input.u16()? as i16];
        let surface = r.reference(shift)?;
        let label_version = r.input.u32()?;
        ensure!(
            label_version == 0x122,
            "unsupported message-label version {label_version:#x}"
        );
        let label_flags = r.input.u16()?;
        let label_mode = r.input.u8()?;
        let frame = r.reference(shift)?;
        let layout_version = r.input.u8()?;
        ensure!(
            layout_version == 1,
            "unsupported message-layout version {layout_version}"
        );
        let layout = r.input.u32()?;
        ensure!(r.input.remaining() == 0, "trailing message-window data");
        Ok(Self {
            order,
            flags,
            kind,
            rectangles,
            initial_text,
            children,
            reserved,
            surface_flags,
            surface_parameters,
            content_rect,
            spacing,
            surface,
            label_version,
            label_flags,
            label_mode,
            frame,
            layout_version,
            layout,
        })
    }

    pub fn visit_references(&self, visitor: &mut impl FnMut(&ObjectRef)) {
        for child in &self.children {
            child.visit(visitor);
        }
        self.surface.visit(visitor);
        self.frame.visit(visitor);
    }
}

/// Eight native border pieces and their serialized positioning metadata.
#[derive(Debug, Clone, Serialize)]
pub struct FrameImage {
    pub pieces: [ObjectRef; 8],
    /// Native fields 0x50..0x5f, 0x60..0x67 and 0x68..0x6f.
    pub geometry: [i16; 16],
}
impl FrameImage {
    pub fn parse(data: &[u8], schema: u16, shift: u32) -> Result<Self> {
        ensure!(schema == 1, "unsupported CFrameImage schema {schema}");
        ensure!(data.len() <= 16 * 1024 * 1024, "oversized frame image");
        let mut r = ObjectReader::new(data);
        r.input.skip(3)?;
        r.scrambled = true;
        r.pad(3)?;
        let pieces = [
            r.reference(shift)?,
            r.reference(shift)?,
            r.reference(shift)?,
            r.reference(shift)?,
            r.reference(shift)?,
            r.reference(shift)?,
            r.reference(shift)?,
            r.reference(shift)?,
        ];
        let mut geometry = [0; 16];
        for word in &mut geometry {
            *word = r.input.u16()? as i16;
        }
        ensure!(r.input.remaining() == 0, "trailing frame-image data");
        Ok(Self { pieces, geometry })
    }
    pub fn visit_references(&self, visitor: &mut impl FnMut(&ObjectRef)) {
        for piece in &self.pieces {
            piece.visit(visitor);
        }
    }
}
