//! CRbx image selectors and their inline CImgSel records.
use crate::{
    object::{ObjectReader, ObjectRef},
    profile::Profile,
};
use anyhow::{Context, Result, ensure};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ImageSelector {
    /// Native linked-object identifier, not the numeric selection result.
    pub header: u32,
    pub id: u32,
    /// One image-source/composition mode byte for each of the four states.
    pub flags: u32,
    /// Enabled, disabled, selected, pushed images, in native order.
    pub images: [ObjectRef; 4],
    pub properties: Profile,
    pub status: u16,
    /// X, Y, width, height in the parent image's coordinate space.
    pub rect: [i16; 4],
}

/// CRip schema 1's indexed SIA image, used as an exact hit-test map. Unlike an
/// RGB bitmap, each decoded byte is a selector ID and must not be color-mapped.
pub struct SelectorMask {
    pub width: u16,
    pub height: u16,
    pub x: u16,
    pub y: u16,
    pub pixels: Vec<u8>,
}
impl SelectorMask {
    pub fn parse(data: &[u8]) -> Result<Self> {
        use crate::binary::Reader;
        let mut r = Reader::new(data);
        r.skip(3)?;
        r.u32()?;
        let x = r.u16()?;
        let y = r.u16()?;
        let width = r.u16()?;
        let height = r.u16()?;
        r.skip(4)?; // crop dimensions (the indexed mode uses the full surface)
        let flags = r.u32()?;
        ensure!(flags & 255 == 1, "CRip is not an indexed SIA selector mask");
        let size = r.u32()? as usize;
        r.u32()?;
        let mut payload = Reader::new(r.bytes(size)?);
        ensure!(r.remaining() == 0, "trailing indexed selector mask bytes");
        let count = usize::from(width) * usize::from(height);
        ensure!(
            count > 0 && count <= 16_777_216,
            "invalid selector mask dimensions"
        );
        let mut pixels = vec![0; count];
        for row in pixels.chunks_mut(usize::from(width)) {
            let mut x = 0;
            let mut color = 0;
            while x < row.len() {
                let count = usize::from(payload.u8()?);
                ensure!(count <= row.len() - x, "selector mask run exceeds row");
                row[x..x + count].fill(color);
                x += count;
                if x < row.len() {
                    color = payload.u8()?;
                }
            }
        }
        ensure!(payload.remaining() == 0, "excess selector mask run data");
        Ok(Self {
            width,
            height,
            x,
            y,
            pixels,
        })
    }
    pub fn at(&self, x: i32, y: i32) -> Option<u8> {
        let x = x.checked_sub(i32::from(self.x))?;
        let y = y.checked_sub(i32::from(self.y))?;
        if x < 0 || y < 0 || x >= i32::from(self.width) || y >= i32::from(self.height) {
            return None;
        }
        self.pixels
            .get(y as usize * usize::from(self.width) + x as usize)
            .copied()
    }
}
impl ImageSelector {
    pub(crate) fn read(r: &mut ObjectReader<'_>, schema: u16, shift: u32) -> Result<Self> {
        ensure!(
            [1, 2].contains(&schema),
            "unsupported CImgSel schema {schema}"
        );
        let header = r.input.u32()?;
        let id = r.input.u32()?;
        let flags = r.input.u32()?;
        let images = [
            r.reference(shift)?,
            r.reference(shift)?,
            r.reference(shift)?,
            r.reference(shift)?,
        ];
        let properties = Profile::from_text(r.input.string()?)?;
        let status = r.input.u16()?;
        let mut rect = [0; 4];
        if schema >= 2 {
            for v in &mut rect {
                *v = r.input.u16()? as i16;
            }
        }
        Ok(Self {
            header,
            id,
            flags,
            images,
            properties,
            status,
            rect,
        })
    }
}
#[derive(Debug, serde::Serialize)]
pub struct ImageMenu {
    pub max_id: u32,
    pub base: ObjectRef,
    pub overlay: ObjectRef,
    pub selectors: Vec<ImageSelector>,
    pub format: u32,
    pub width: u16,
    pub height: u16,
    pub stride: u32,
    pub trailing_data: Vec<u8>,
}
impl ImageMenu {
    pub fn parse(data: &[u8], schema: u16, shift: u32) -> Result<Self> {
        ensure!(schema == 1, "unsupported CRbx schema {schema}");
        let mut r = ObjectReader::new(data);
        r.input.skip(3)?;
        r.scrambled = true;
        r.pad(3)?;
        let max_id = r.input.u32()?;
        let base = r.reference(shift)?;
        let overlay = r.reference(shift)?;
        let count = r.input.count()?;
        ensure!(
            count <= 4096 && count <= r.input.remaining() / 2,
            "invalid CRbx selector count"
        );
        let mut selectors = Vec::with_capacity(count);
        for _ in 0..count {
            // The list uses ordinary CArchive objects, not RIO ObjectRef's
            // flags/extent wrapper. Reserve their object-cache slots as well.
            let class = r.class()?.context("null CRbx selector")?;
            ensure!(class.name == "CImgSel", "non-image selector in CRbx");
            r.pad(1)?;
            selectors.push(ImageSelector::read(&mut r, class.schema, shift)?);
        }
        let format = r.input.u32()?;
        let width = r.input.u16()?;
        let height = r.input.u16()?;
        let stride = r.input.u32()?;
        ensure!(
            width > 0 && height > 0 && u64::from(width) * u64::from(height) <= 16_777_216,
            "invalid CRbx dimensions"
        );
        let trailing_data = r.input.bytes(r.input.remaining())?.to_vec();
        Ok(Self {
            max_id,
            base,
            overlay,
            selectors,
            format,
            width,
            height,
            stride,
            trailing_data,
        })
    }
    pub fn visit_references(&self, visitor: &mut impl FnMut(&ObjectRef)) {
        self.base.visit(visitor);
        self.overlay.visit(visitor);
        for selector in &self.selectors {
            for image in &selector.images {
                image.visit(visitor);
            }
        }
    }
}
