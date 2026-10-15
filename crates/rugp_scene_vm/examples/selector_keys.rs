//! Static gallery-unlock packet audit; prints only classes/counts/cursors.
//! Never executes the Windows engine or dumps retail dialogue/assets.
use anyhow::{Context, Result};
use rugp_assets::{RioArchive, native_schema::NativeSchema};
use rugp_scene_vm::{
    Argument,
    vm::{Program, Value, Vm},
};
use std::{collections::BTreeMap, sync::Arc};
fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bytes(bytes) => format!("bytes ({} bytes)", bytes.len()),
        Value::Real(n) => format!("real {n}"),
        Value::Reference(r) => format!(
            "{} flags={:#x} stored={} inline={} owner={} selector={}",
            r.class.as_ref().map_or("null", |c| c.name.as_str()),
            r.flags,
            r.offset.is_some(),
            r.inline.is_some(),
            r.owner.is_some(),
            r.selector
        ),
        Value::Text(s) => format!("text ({} bytes)", s.len()),
        Value::Integer(n) => format!("integer {n}"),
    }
}
fn main() -> Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .context("game directory required")?;
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let program = Arc::new(Program::load(&archive, &native)?);
    for (name, r) in &archive
        .project
        .as_ref()
        .context("project missing")?
        .references
    {
        println!(
            "project {name}: {} at {:?}",
            describe(&Value::Reference(r.clone())),
            r.offset
        );
    }
    let mut groups = BTreeMap::new();
    for (scene, code) in &program.scenes {
        let vm = Vm::new(Arc::clone(&program), *scene)?;
        for instruction in &code.instructions {
            if !instruction.arguments.iter().any(|a| {
                matches!(a,
                Argument::Message{class,..} if class.name.ends_with("OM_SetSelecterByDatabase"))
            }) {
                continue;
            }
            let message = vm.message(instruction)?;
            if *scene == 239 && instruction.address == 0x88 {
                for (name, value) in &message.fields {
                    let mut value = if let Value::Reference(r) = value {
                        Some(r)
                    } else {
                        None
                    };
                    while let Some(r) = value {
                        println!(
                            "sample {name}: {} at {:?}",
                            describe(&Value::Reference(r.clone())),
                            r.offset
                        );
                        value = r.owner.as_deref();
                    }
                }
            }
            let signature = (
                describe(&message.target),
                message
                    .fields
                    .iter()
                    .map(|(name, value)| (name.clone(), describe(value)))
                    .collect::<Vec<_>>(),
            );
            let group = groups
                .entry(signature)
                .or_insert((0, *scene, instruction.address));
            group.0 += 1;
        }
    }
    for ((target, fields), (count, scene, pc)) in groups {
        println!("count={count} first={scene}:{pc:#x} target={target} fields={fields:?}");
    }
    Ok(())
}
