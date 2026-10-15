//! Metadata-only message audit. Optional message class selects call sites and
//! packet fields; text/sample bytes are never printed. Runtime overrides are
//! reported as encoded operands, not misrepresented as evaluated values.
use anyhow::{Context, Result};
use rugp_assets::{RioArchive, native_schema::NativeSchema};
use rugp_scene_vm::{Argument, vm::Program};
use std::collections::BTreeMap;

fn describe(argument: &Argument) -> String {
    match argument {
        Argument::Number(n) => format!("{} ({n:#x})", *n as i32),
        Argument::Text(s) => format!("text: {} bytes", s.len()),
        Argument::Bytes(s) | Argument::Literal { data: s, .. } => format!("bytes: {}", s.len()),
        Argument::Reference(r) => r
            .class
            .as_ref()
            .map_or("null".into(), |c| format!("{}#{}", c.name, c.schema)),
        Argument::Value {
            descriptor,
            arguments,
        } => {
            if descriptor & 3 == 2 {
                format!("constant {}", (*descriptor as i32) >> 2)
            } else {
                format!(
                    "value {descriptor:#x}: [{}]",
                    arguments
                        .iter()
                        .map(describe)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        Argument::Field { name, argument, .. } => format!("{name}: {}", describe(argument)),
        Argument::Parameter(arguments) => format!(
            "override [{}]",
            arguments
                .iter()
                .map(describe)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Argument::Message { class, .. } => class.name.clone(),
    }
}
fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game root")?;
    let filter = std::env::args().nth(2);
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let program = Program::load(&archive, &native)?;
    let mut counts = BTreeMap::<_, usize>::new();
    for scene in program.scenes.values() {
        for instruction in &scene.instructions {
            if instruction.class.name != "CVmGenericMsg" {
                continue;
            }
            let Some(Argument::Message { class, arguments }) = instruction.arguments.get(1) else {
                continue;
            };
            let name = class.name.trim_start_matches(['&', '-']);
            *counts.entry(name.to_owned()).or_default() += 1;
            if filter.as_deref() != Some(name) {
                continue;
            }
            println!(
                "{}:{:#x} {name}, target {}",
                scene.object_id,
                instruction.address,
                instruction
                    .arguments
                    .first()
                    .map_or("missing".into(), describe)
            );
            for field in arguments {
                println!("  {}", describe(field));
            }
            for arg in instruction
                .arguments
                .iter()
                .filter(|a| matches!(a, Argument::Parameter(_)))
            {
                println!("  {}", describe(arg));
            }
        }
    }
    if filter.is_none() {
        for (name, count) in counts {
            println!("{count:6} {name}");
        }
    }
    Ok(())
}
