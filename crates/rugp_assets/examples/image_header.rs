//! Inspect native bitmap metadata without extracting pixels or retail text.
use anyhow::{Context, Result};
use rugp_assets::{RioArchive, image::R6Header, object::ObjectReader};
fn main() -> Result<()> {
    let mut args = std::env::args();
    args.next();
    let archive = RioArchive::open(args.next().context("game directory")?)?;
    for arg in args {
        let offset = if let Some(hex) = arg.strip_prefix("0x") {
            u64::from_str_radix(hex, 16)?
        } else {
            arg.parse()?
        };
        let bytes = archive.read_at(offset, 4096)?;
        let mut r = ObjectReader::new(&bytes);
        r.input.skip(3)?;
        r.scrambled = true;
        r.pad(3)?;
        let header = R6Header::read(&mut r, archive.address_shift)?;
        let tail = &bytes[r.input.position - 8..r.input.position];
        println!(
            "{offset:#x}: {}x{} crop {}x{} at {},{} flags={:#x} depth={:#x} tail={tail:02x?} floats={:?}",
            header.canvas_width,
            header.canvas_height,
            header.width,
            header.height,
            header.x,
            header.y,
            header.flags,
            header.depth,
            tail.as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect::<Vec<_>>()
        );
    }
    Ok(())
}
