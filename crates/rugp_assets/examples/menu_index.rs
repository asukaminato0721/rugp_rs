use anyhow::{Context, Result};
use rugp_assets::{RioArchive, menu::ImageMenu};
fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game directory")?;
    let archive = RioArchive::open(root)?;
    let menu = ImageMenu::parse(&archive.read_at(248006920, 1119)?, 1, archive.address_shift)?;
    println!(
        "{}x{}, max {}, format {:#x}, stride {}, tail {:?}",
        menu.width, menu.height, menu.max_id, menu.format, menu.stride, menu.trailing_data
    );
    for (label, r) in [("base", &menu.base), ("overlay", &menu.overlay)] {
        println!("{label}: {:?} {:?}", r.class, r.offset);
    }
    for s in &menu.selectors {
        println!(
            "id {} header {} flags {:#x} status {:#x} rect {:?}",
            s.id, s.header, s.flags, s.status, s.rect
        );
        println!(
            "images: {:?}",
            s.images
                .iter()
                .map(|r| (&r.class, r.offset))
                .collect::<Vec<_>>()
        );
    }
    let reference = &menu.base;
    let mask = rugp_assets::menu::SelectorMask::parse(&archive.read_at(
        reference.offset.context("mask offset missing")?,
        reference.size.context("mask extent missing")? as usize,
    )?)?;
    for (x, y) in [(0, 0), (200, 100), (200, 160), (200, 217), (200, 275)] {
        println!("mask {x},{y}: {:?}", mask.at(x, y));
    }
    Ok(())
}
