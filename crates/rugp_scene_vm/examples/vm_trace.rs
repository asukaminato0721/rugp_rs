//! Scheduling diagnostic only: acknowledges host requests without executing effects.
use anyhow::{Context, Result};
use rugp_assets::{RioArchive, native_schema::NativeSchema};
use rugp_scene_vm::vm::{Program, Step, Value, Vm};
fn describe(v: &Value) -> String {
    match v {
        Value::Integer(v) => v.to_string(),
        Value::Real(v) => v.to_string(),
        Value::Null => "null".into(),
        Value::Text(s) => format!("text[{}]", s.len()),
        Value::Bytes(b) => format!("bytes[{}]", b.len()),
        Value::Reference(r) => format!(
            "{}@{:x}",
            r.class.as_ref().map_or("cached", |c| c.name.as_str()),
            r.offset.unwrap_or(0)
        ),
    }
}
fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game root")?;
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let program = Program::load(&archive, &native)?;
    let entry = program.startup(&archive)?;
    let mut vm = Vm::new(std::sync::Arc::new(program), entry)?;
    let steps = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(200);
    eprintln!("Dry scheduling trace: host requests are acknowledged, NOT executed.");
    for _ in 0..steps {
        match vm.step().with_context(|| format!("PC {:?}", vm.cursor()))? {
            Step::Halted => {
                println!("halted");
                break;
            }
            Step::Advanced => {}
            Step::Host {
                cursor,
                instruction,
            } => {
                if instruction.class.name == "CVmGenericMsg" {
                    let msg = vm.message(&instruction)?;
                    println!(
                        "{}:{:04x} {} {} {:?}",
                        cursor.scene,
                        cursor.address,
                        msg.class,
                        describe(&msg.target),
                        msg.fields
                            .iter()
                            .map(|(k, v)| format!("{k}={}", describe(v)))
                            .collect::<Vec<_>>()
                    );
                } else {
                    println!(
                        "{}:{:04x} {}",
                        cursor.scene, cursor.address, instruction.class.name
                    );
                }
                vm.acknowledge_host()?;
            }
        }
    }
    Ok(())
}
