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
    let extension_header = args.any(|arg| arg == "--extension-header");
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
    println!("placement {:?}", bg.placement());
    if extension_header {
        let mut r = rugp_assets::binary::Reader::new(&bg.trailing_data);
        let version = r.u8()?;
        let color = r.u32()?;
        let screen = [r.u16()? as i16, r.u16()? as i16];
        let range = [r.u32()? as i32, r.u32()? as i32];
        let scale = f32::from_bits(r.u32()?);
        println!(
            "extension version {version}, color {color:#010x}, screen {screen:?}, range {range:?}, scale {scale}"
        );
        // A native scrambled class/object record, not a RIO reference or
        // the one-byte schema encoding used by raw menu selector records.
        let mut raw = rugp_assets::object::ObjectReader::new(&bg.trailing_data[r.position..]);
        raw.scrambled = true;
        let class = raw.class()?;
        anyhow::ensure!(
            class
                .as_ref()
                .is_none_or(|c| (1..=128).contains(&c.name.len()) && c.name.is_ascii()),
            "invalid layer class header"
        );
        println!("first extended layer class {class:?}");
    }
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
