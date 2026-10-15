use anyhow::Result;
use rugp_assets::{RioArchive, native_schema::NativeSchema, script::Script};
use rugp_scene_vm::{Argument, Scenario};
fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).expect("game root");
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let target = std::env::args()
        .nth(2)
        .map(|s| s.parse::<usize>())
        .transpose()?;
    let target_offset = target
        .and_then(|id| archive.nodes.get(id))
        .and_then(|n| n.offset);
    let mut counts = std::collections::BTreeMap::new();
    for node in archive.nodes.iter().filter(|n| n.class.name == "CRsa") {
        let scene = Scenario::decode_with_schema(
            &Script::load(&archive, node.id)?,
            archive.address_shift,
            Some(&native),
        )?;
        for ins in &scene.instructions {
            if let Some(offset) = target_offset {
                let mut matched = false;
                for arg in &ins.arguments {
                    arg.visit_references(&mut |r| matched |= r.offset == Some(offset));
                }
                if matched {
                    let operation = if ins.class.name == "CVmFlagOp" {
                        ins.arguments.get(3).and_then(|a| {
                            if let Argument::Number(n) = a {
                                Some(*n)
                            } else {
                                None
                            }
                        })
                    } else {
                        None
                    };
                    println!(
                        "xref scene={} pc={:#x} class={} arguments={} operation={operation:?}",
                        node.id,
                        ins.address,
                        ins.class.name,
                        ins.arguments.len()
                    );
                }
                continue;
            }
            if ["CVmCall", "CVmJump", "CVmLabel"].contains(&ins.class.name.as_str())
                && let Argument::Reference(r) = &ins.arguments[0]
            {
                *counts
                    .entry(format!(
                        "{} {} flags {:#x}, selector {:#x}",
                        ins.class.name,
                        r.class.as_ref().map_or("cached", |c| c.name.as_str()),
                        r.flags,
                        r.selector
                    ))
                    .or_insert(0) += 1;
            }
            if ins.class.name == "CVmFlagOp" {
                *counts
                    .entry(format!(
                        "FlagOp operation {:?}, branch mode {:?}",
                        ins.arguments[3], ins.arguments[4]
                    ))
                    .or_insert(0) += 1;
            }
        }
    }
    for (key, value) in counts {
        println!("{value:8} {key}");
    }
    Ok(())
}
