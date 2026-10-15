//! Inspect world/background metadata, without extracting pixels or game text.
use anyhow::{Context, Result};
use rugp_assets::{RioArchive, background::Background};
fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let archive = RioArchive::open(args.next().context("game directory")?)?;
    let offset: u64 = args
        .next()
        .context("background offset")?
        .to_str()
        .context("offset")?
        .parse()?;
    let size: usize = args
        .next()
        .context("background size")?
        .to_str()
        .context("size")?
        .parse()?;
    let data = archive.read_at(offset, size)?;
    let bg = Background::parse(&data, 6, archive.address_shift)?;
    println!(
        "{}x{} flags {:#x}, display {:02x?}, trailing {} bytes, properties {}",
        bg.width,
        bg.height,
        bg.flags,
        bg.display_metadata,
        bg.trailing_data.len(),
        bg.properties.is_some()
    );
    for (i, plane) in bg.planes.iter().enumerate() {
        let image = plane
            .image
            .inline
            .as_ref()
            .context("external plane")?
            .decode_bitmap()?;
        println!(
            "plane {i}: {:02x?}, bitmap {}x{} scale {}",
            plane.metadata, image.canvas_width, image.canvas_height, image.display_scale
        );
    }
    Ok(())
}
