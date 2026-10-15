//! Metadata-only diagnostic: never prints stored dialogue or game prose.
use anyhow::{Context, Result};
use rugp_assets::{
    RioArchive,
    message::{FrameImage, MessageBox},
};
fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game directory")?;
    let archive = RioArchive::open(root)?;
    let node = archive
        .nodes
        .iter()
        .find(|n| n.class.name == "CMsgSelBox")
        .context("message box")?;
    let data = archive.read_node(node.id)?;
    let message = MessageBox::parse(&data, node.class.schema, archive.address_shift)?;
    println!(
        "rectangles {:?}, content {:?}, view position {:?}, layout {:#x}",
        message.rectangles, message.content_rect, message.view_position, message.layout
    );
    message
        .visit_references(&mut |r| println!("reference {:?} {:?} {:?}", r.class, r.offset, r.size));
    let bytes = archive.read_at(
        message.frame.offset.context("frame offset")?,
        message.frame.size.context("frame size")? as usize,
    )?;
    let frame = FrameImage::parse(
        &bytes,
        message.frame.class.as_ref().context("frame class")?.schema,
        archive.address_shift,
    )?;
    println!("frame geometry {:?}", frame.geometry);
    for (i, r) in frame.pieces.iter().enumerate() {
        println!("piece {i}: {:?} {:?} {:?}", r.class, r.offset, r.size);
    }
    Ok(())
}
