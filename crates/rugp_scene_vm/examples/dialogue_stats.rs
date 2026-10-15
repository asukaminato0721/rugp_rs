//! Dialogue metadata coverage; never emits retail text.
use anyhow::{Context, Result};
use rugp_assets::{RioArchive, native_schema::NativeSchema};
use rugp_scene_vm::{Argument, vm::Program};
use std::collections::BTreeMap;
fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game root")?;
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let program = Program::load(&archive, &native)?;
    let mut flags = BTreeMap::new();
    let mut controls = BTreeMap::new();
    let mut targets = BTreeMap::new();
    let mut third = BTreeMap::new();
    let mut endings = BTreeMap::new();
    for scene in program.scenes.values() {
        for instruction in scene
            .instructions
            .iter()
            .filter(|i| i.class.name == "CVmMsg")
        {
            *flags
                .entry(format!("{:#x}", instruction.flags & !0xfffff))
                .or_insert(0u32) += 1;
            for (map, arg) in [
                (&mut targets, &instruction.arguments[0]),
                (&mut third, &instruction.arguments[2]),
            ] {
                let class = match arg {
                    Argument::Reference(r) => {
                        r.class.as_ref().map_or("null/cached", |c| c.name.as_str())
                    }
                    _ => "not-reference",
                };
                *map.entry(class.to_owned()).or_insert(0u32) += 1;
            }
            if let Argument::Text(body) = &instruction.arguments[1] {
                *endings
                    .entry(body.chars().last().map_or("empty".into(), |c| {
                        if c.is_control() {
                            format!("U+{:04x}", c as u32)
                        } else {
                            "glyph".into()
                        }
                    }))
                    .or_insert(0u32) += 1;
                for c in body.chars().filter(|c| c.is_control()) {
                    *controls
                        .entry(format!("U+{:04x}", c as u32))
                        .or_insert(0u32) += 1;
                }
            }
        }
    }
    println!(
        "flags {flags:?}\ntargets {targets:?}\nthird {third:?}\ncontrols {controls:?}\nendings {endings:?}"
    );
    Ok(())
}
