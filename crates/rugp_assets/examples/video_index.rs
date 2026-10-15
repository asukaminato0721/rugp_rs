//! Checks all scene-referenced video extents without loading the movies into RAM.
use anyhow::{Context, Result};
use rugp_assets::{RioArchive, crsi::Crsi};
fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game directory")?;
    let archive = RioArchive::open(root)?;
    for (offset, size) in [(0xd0ba5f, 234130539), (0x7c67b686, 202725217)] {
        let stream = Crsi::read(&archive, offset, size)?;
        let keys = stream.frames.iter().filter(|f| f.is_keyframe()).count();
        println!(
            "{offset:#x}: {}x{}, rate {}, {} frames ({keys} keyframes), {} indexed bytes, {} trailer bytes",
            stream.width,
            stream.height,
            stream.rate,
            stream.frames.len(),
            stream.payload_size,
            stream.trailing_data.len()
        );
        for i in 0..stream.frames.len() {
            let data = stream.frame_data(&archive, offset, i)?;
            anyhow::ensure!(
                data.len() == stream.frames[i].size as usize,
                "short frame read"
            );
            if i < 4 && stream.frames[i].is_keyframe() {
                let image = rugp_assets::crsi::codec::decode_keyframe(
                    stream.width,
                    stream.height,
                    &stream.frames[i],
                    &data,
                )
                .with_context(|| format!("decode frame {i}"))?;
                let rgb: u64 = image
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]))
                    .sum();
                println!("frame {i} decoded, RGBsum={rgb}");
            }
        }
    }
    Ok(())
}
