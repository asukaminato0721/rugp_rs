//! Report effect classes and numeric operands without scenario text or images.
use anyhow::{Context, Result};
use rugp_assets::{RioArchive, fade::Fade, native_schema::NativeSchema};
use rugp_scene_vm::{Argument, vm::Program};
use std::collections::BTreeMap;

fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game root")?;
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let program = Program::load(&archive, &native)?;
    let mut counts = BTreeMap::<String, usize>::new();
    for scene in program.scenes.values() {
        for instruction in &scene.instructions {
            if instruction.class.name != "CVmBlt" {
                continue;
            }
            let Some(Argument::Message { class, arguments }) = instruction.arguments.first() else {
                continue;
            };
            let count = counts.entry(class.name.clone()).or_default();
            *count += 1;
            if *count > 2 && !class.name.contains("Stretch") && class.name != "CFadeCarten" {
                continue;
            }
            let Some(Argument::Bytes(data)) = arguments.first() else {
                continue;
            };
            let fade = Fade::from_inline(&class.name, data)?;
            println!(
                "{}:{:#x} {} mode={:#x} rectangles={:?} alpha={:?} stretch={:?} curtain={:?} surfaces={:?}",
                scene.object_id,
                instruction.address,
                class.name,
                fade.mode,
                fade.rectangles,
                [0, 64, 128, 192, 256].map(|t| fade.curve.alpha(t, 256)),
                fade.stretch,
                fade.curtain,
                arguments[1..]
                    .iter()
                    .map(|arg| match arg {
                        Argument::Reference(r) =>
                            (r.class.as_ref().map(|c| c.name.as_str()), r.offset),
                        _ => (None, None),
                    })
                    .collect::<Vec<_>>(),
            );
        }
    }
    for (name, count) in counts {
        println!("{name}: {count}");
    }
    Ok(())
}
