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
    let mut syncs = BTreeMap::new();
    for scene in program.scenes.values() {
        for instruction in &scene.instructions {
            let n = |index: usize| {
                if let Argument::Number(n) = instruction.arguments[index] {
                    n
                } else {
                    u32::MAX
                }
            };
            if instruction.class.name == "CVmSync" {
                if let Argument::Reference(source) = &instruction.arguments[0]
                    && let Some(class) = &source.class
                    && ["CPmBgm", "CPmEffect", "COptimizedObs"].contains(&class.name.as_str())
                {
                    *syncs
                        .entry((class.name.clone(), n(1), n(2)))
                        .or_insert(0u32) += 1;
                }
                continue;
            }
            if instruction.class.name != "CVmSound" {
                continue;
            }
            *kinds.entry((n(1), n(2))).or_insert(0u32) += 1;
            if n(1) == 4 {
                *fades.entry((n(2), [n(3), n(4), n(5)])).or_insert(0u32) += 1;
            }
        }
    }
    println!("kind/flags {kinds:?}");
    println!(
        "literal-source audio sync class/mode/time {syncs:?}; {:#x} denotes a nonliteral control",
        u32::MAX
    );
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
