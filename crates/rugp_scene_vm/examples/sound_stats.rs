//! Native sound-control metadata; never emits dialogue or sound data.
use anyhow::{Context, Result};
use rugp_assets::{RioArchive, native_schema::NativeSchema};
use rugp_scene_vm::{Argument, vm::Program};
use std::collections::BTreeMap;
fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game root")?;
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let program = Program::load(&archive, &native)?;
    let mut kinds = BTreeMap::new();
    let mut fades = BTreeMap::new();
    for scene in program.scenes.values() {
        for instruction in scene
            .instructions
            .iter()
            .filter(|i| i.class.name == "CVmSound")
        {
            let n = |index: usize| {
                if let Argument::Number(n) = instruction.arguments[index] {
                    n
                } else {
                    u32::MAX
                }
            };
            *kinds.entry((n(1), n(2))).or_insert(0u32) += 1;
            if n(1) == 4 {
                *fades.entry((n(2), [n(3), n(4), n(5)])).or_insert(0u32) += 1;
            }
        }
    }
    println!("kind/flags {kinds:?}");
    println!(
        "{} transition records, {} distinct flag/parameter tuples",
        fades.values().sum::<u32>(),
        fades.len()
    );
    if std::env::args_os().any(|s| s == "--transitions") {
        println!("transitions {fades:?}");
    }
    Ok(())
}
