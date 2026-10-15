//! Native CS5i#2 bottom-up BGRA surfaces used by window borders.
use crate::{binary::Reader, image::Image};
use anyhow::{Result, ensure};

pub fn decode_s5i(data: &[u8], schema: u16) -> Result<Image> {
    ensure!(schema == 2, "unsupported CS5i schema {schema}");
    let mut r = Reader::new(data);
    r.skip(3)?;
    let format = r.u32()?;
    ensure!(format == 0x47, "unsupported CS5i format {format:#x}");
    let x = r.u16()?;
    let y = r.u16()?;
    let canvas_width = u32::from(r.u16()?);
    let canvas_height = u32::from(r.u16()?);
    let width = u32::from(r.u16()?);
    let height = u32::from(r.u16()?);
    let stride = r.u32()? as usize;
    let size = r.u32()? as usize;
    ensure!(
        width > 0
            && height > 0
            && canvas_width <= 32767
            && canvas_height <= 32767
            && u32::from(x) + width <= canvas_width
            && u32::from(y) + height <= canvas_height
            && u64::from(canvas_width) * u64::from(canvas_height) <= 16_777_216,
        "invalid CS5i canvas/crop dimensions"
    );
    ensure!(
        stride >= width as usize * 4 && stride <= 64 * 1024 * 1024,
        "invalid CS5i row stride"
    );
    ensure!(
        size as u64 == stride as u64 * u64::from(height) && size <= 64 * 1024 * 1024,
        "invalid CS5i payload length"
    );
    let pixels = r.bytes(size)?;
    ensure!(r.remaining() == 0, "trailing CS5i data");
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for row in (0..height as usize).rev() {
        for p in pixels[row * stride..row * stride + width as usize * 4]
            .as_chunks::<4>()
            .0
        {
            rgba.extend([p[2], p[1], p[0], p[3]]);
        }
    }
    Ok(Image {
        canvas_width,
        canvas_height,
        width,
        height,
        x,
        y,
        display_scale: 1.0,
        rgba,
    })
}
