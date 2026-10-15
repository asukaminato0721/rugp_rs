use anyhow::Result;
use rugp_assets::{RioArchive, native_schema::NativeSchema, oor::Oor};
use rugp_scene_vm::resources::ResourceIndex;
fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).expect("game root");
    let archive = RioArchive::open(&root)?;
    let index = ResourceIndex::scan(&archive, &NativeSchema::from_directory(root.as_ref())?)?;
    let mut count = 0;
    for r in index
        .resources
        .values()
        .filter(|r| r.class.name == "COptimizedObs")
    {
        let bytes = archive.read_at(r.offset, r.size as usize)?;
        match Oor::parse(&bytes) {
            Ok(_) => count += 1,
            Err(e) => println!(
                "{:#x}+{}: {e:#}; tail {:02x?}",
                r.offset,
                r.size,
                &bytes[bytes.len().saturating_sub(48)..]
            ),
        }
    }
    println!("{count} valid OOR streams");
    Ok(())
}
