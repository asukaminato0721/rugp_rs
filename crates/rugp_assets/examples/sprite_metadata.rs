//! Inspect sprite dimensions and native anchor metadata without decoding pixels.
use anyhow::{Context, Result};
use rugp_assets::{RioArchive, crmt::Crmt};
fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let archive = RioArchive::open(args.next().context("game directory")?)?;
    let offset: u64 = args
        .next()
        .context("offset")?
        .to_str()
        .context("offset")?
        .parse()?;
    let size: usize = args
        .next()
        .context("size")?
        .to_str()
        .context("size")?
        .parse()?;
    let sprite = Crmt::parse(&archive.read_at(offset, size)?, archive.address_shift)?;
    println!(
        "size={}x{} scale={} base={:?} properties={:?}",
        sprite.width, sprite.height, sprite.scale, sprite.base_offset, sprite.image_properties
    );
    if let (Some(offset), Some(size)) =
        (sprite.image_properties.offset, sprite.image_properties.size)
    {
        let bytes = archive.read_at(offset, size as usize)?;
        println!(
            "anchor_q16={:?}",
            rugp_assets::image_properties::anchor(
                &bytes,
                [sprite.base_offset.0, sprite.base_offset.1]
            )?
        );
    }
    Ok(())
}
