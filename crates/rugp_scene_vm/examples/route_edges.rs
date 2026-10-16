//! Static cross-scene jump/choice metadata, without emitting retail dialogue.
//! Edges are possibilities, not proof that their conditions are satisfiable.
use anyhow::{Context, Result};
use rugp_assets::{RioArchive, native_schema::NativeSchema};
use rugp_scene_vm::{Argument, vm::Program};
use std::collections::BTreeMap;

fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game root")?;
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let program = Program::load(&archive, &native)?;
    let mut scenes = BTreeMap::new();
    let mut labels = BTreeMap::new();
    for (&id, scene) in &program.scenes {
        scenes.insert(archive.nodes[id].offset.context("scene extent")?, (id, 0));
        for ins in &scene.instructions {
            if ins.class.name == "CVmLabel"
                && let Some(Argument::Reference(label)) = ins.arguments.first()
                && let Some(offset) = label.offset
            {
                labels.insert(offset, (id, ins.address));
            }
        }
    }
    for (&id, scene) in &program.scenes {
        for ins in &scene.instructions {
            if ins.class.name == "CVmJump"
                && let Some(Argument::Reference(target)) = ins.arguments.first()
                && let Some(destination) =
                    target
                        .offset
                        .and_then(|offset| match target.class.as_ref()?.name.as_str() {
                            "CRsa" => scenes.get(&offset),
                            "CLabel" => labels.get(&offset),
                            _ => None,
                        })
                && destination.0 != id
            {
                println!(
                    "jump {id} {} {} {}",
                    ins.address, destination.0, destination.1
                );
            }
            if ins.class.name == "CVmGenericMsg"
                && ins.arguments.iter().any(|arg| {
                    matches!(arg, Argument::Message { class, .. }
                        if class.name.ends_with("OM_AskUserSelectEx"))
                })
            {
                println!("choice {id} {}", ins.address);
            }
        }
    }
    Ok(())
}
