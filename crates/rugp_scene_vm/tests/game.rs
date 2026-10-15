use anyhow::{Result, ensure};
use rugp_assets::{RioArchive, native_schema::NativeSchema, script::Script};
use rugp_scene_vm::Scenario;

/// Uses the user's own game; no retail assets or dialogue are committed.
#[test]
#[ignore = "set RUGP_TEST_GAME to a KIMINOZO_LE game directory"]
fn kiminozo_le_all_project_scenes() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME")
        .ok_or_else(|| anyhow::anyhow!("RUGP_TEST_GAME missing"))?;
    let root = std::path::Path::new(&root);
    let archive = RioArchive::open(root)?;
    ensure!(archive.nodes.len() == 666, "unexpected game build");
    let native = NativeSchema::from_directory(root)?;
    ensure!(native.messages.len() >= 262, "incomplete native schema");
    let mut scenes = 0;
    let mut instructions = 0;
    for node in archive.nodes.iter().filter(|n| n.class.name == "CRsa") {
        let script = Script::load(&archive, node.id)?;
        let scene = Scenario::decode_with_schema(&script, archive.address_shift, Some(&native))?;
        ensure!(
            scene.null_records.is_empty(),
            "unexpected null records in scene {}",
            node.id
        );
        ensure!(scene.trailing_bytes == 0, "unparsed scene bytes");
        instructions += scene.instructions.len();
        scenes += 1;
    }
    ensure!(scenes == 283, "unexpected scene count {scenes}");
    println!("Validated {scenes} scenes and {instructions} instructions");
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to a KIMINOZO_LE game directory"]
fn kiminozo_le_startup_control_flow() -> Result<()> {
    use rugp_scene_vm::vm::{Cursor, Program, Step, Vm};
    let root = std::env::var_os("RUGP_TEST_GAME")
        .ok_or_else(|| anyhow::anyhow!("RUGP_TEST_GAME missing"))?;
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let program = Program::load(&archive, &native)?;
    let entry = program.startup(&archive)?;
    assert_eq!(entry, 21);
    let first_variable = program.scenes[&entry].instructions[2].arguments[1].clone();
    let second_variable = program.scenes[&entry].instructions[3].arguments[1].clone();
    let mut vm = Vm::new(std::sync::Arc::new(program), entry)?;
    // Host effects are deliberately acknowledged only in this scheduling test.
    // This does not claim audio/UI execution or game playability.
    for _ in 0..2 {
        assert!(matches!(vm.step()?, Step::Host { .. }));
        vm.acknowledge_host()?;
    }
    vm.step()?;
    assert_eq!(vm.evaluate(&first_variable)?.integer()?, 1024);
    vm.step()?;
    assert_eq!(vm.evaluate(&second_variable)?.integer()?, 600);
    vm.step()?;
    assert_eq!(
        vm.cursor(),
        Some(&Cursor {
            scene: 252,
            address: 0
        })
    );
    Ok(())
}
