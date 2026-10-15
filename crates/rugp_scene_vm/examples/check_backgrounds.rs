use anyhow::{Context, Result};
use rugp_assets::{RioArchive, background::Background, native_schema::NativeSchema};
use rugp_scene_vm::resources::ResourceIndex;
fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game root")?;
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let index = ResourceIndex::scan(&archive, &native)?;
    let mut count = 0;
    let mut failures = std::collections::BTreeMap::new();
    for resource in index.resources.values().filter(|r| r.class.name == "CBg2d") {
        let result = (|| -> Result<()> {
            let bytes = archive.read_at(resource.offset, resource.size as usize)?;
            let bg = Background::parse(&bytes, resource.class.schema, archive.address_shift)?;
            for plane in &bg.planes {
                if let Some(inline) = &plane.image.inline {
                    inline.decode_bitmap()?;
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => count += 1,
            Err(error) => {
                *failures.entry(format!("{error:#}")).or_insert(0) += 1;
                eprintln!("{:#x}: {error:#}", resource.offset);
            }
        }
    }
    println!("{count} backgrounds decoded");
    for (error, count) in &failures {
        println!("{count}: {error}");
    }
    anyhow::ensure!(failures.is_empty(), "background failures");
    Ok(())
}
