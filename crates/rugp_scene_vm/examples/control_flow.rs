use anyhow::Result;
use rugp_assets::{RioArchive, native_schema::NativeSchema, script::Script};
use rugp_scene_vm::{Argument, Scenario};
fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).expect("game root");
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let mut counts = std::collections::BTreeMap::new();
    for node in archive.nodes.iter().filter(|n| n.class.name == "CRsa") {
        let scene = Scenario::decode_with_schema(
            &Script::load(&archive, node.id)?,
            archive.address_shift,
            Some(&native),
        )?;
        for ins in &scene.instructions {
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
