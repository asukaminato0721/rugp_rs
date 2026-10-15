use anyhow::{Result, ensure};
use rugp_assets::{RioArchive, native_schema::NativeSchema, script::Script};
use rugp_scene_vm::Scenario;

#[test]
#[ignore = "set RUGP_TEST_GAME to a KIMINOZO_LE game directory"]
fn message_border_expansion_is_explicit_and_idempotent() -> Result<()> {
    use rugp_scene_vm::resources::ResourceIndex;
    let root = std::env::var_os("RUGP_TEST_GAME")
        .ok_or_else(|| anyhow::anyhow!("RUGP_TEST_GAME missing"))?;
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let mut index = ResourceIndex::scan(&archive, &native)?;
    assert_eq!(index.resources.len(), 34993);
    assert!(!index.resources.contains_key(&13493870));
    index.expand_message_sources(&archive)?;
    assert_eq!(index.resources[&13493708].class.name, "CFrameImage");
    for offset in [
        13493870, 13494041, 13494264, 13494435, 13494606, 13495017, 13495428, 13495839,
    ] {
        let resource = &index.resources[&offset];
        assert_eq!(resource.class.name, "CS5i");
        rugp_assets::s5i::decode_s5i(&index.read(&archive, offset)?, resource.class.schema)?;
    }
    let count = index.resources.len();
    index.expand_message_sources(&archive)?;
    assert_eq!(index.resources.len(), count);
    Ok(())
}

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
    let mut postures = 0;
    for node in archive.nodes.iter().filter(|n| n.class.name == "CRsa") {
        let script = Script::load(&archive, node.id)?;
        let scene = Scenario::decode_with_schema(&script, archive.address_shift, Some(&native))?;
        ensure!(
            scene.null_records.is_empty(),
            "unexpected null records in scene {}",
            node.id
        );
        ensure!(scene.trailing_bytes == 0, "unparsed scene bytes");
        let mut posture_failure = None;
        scene.visit_references(&mut |reference| {
            if let Some(class) = &reference.class
                && class.name == "CPostureMoment"
                && let Some(inline) = &reference.inline
            {
                match rugp_assets::posture::Posture::from_inline(inline, class) {
                    Ok(posture) => {
                        postures += 1;
                        assert_eq!(posture.fields.version, 2);
                        assert_eq!(posture.fields.state, 0);
                        assert!(posture.fields.options <= 1);
                        for motion in &posture.references[2..] {
                            if let Some(class) = &motion.class {
                                assert_eq!(class.name, "CMoveAcsOngen");
                                assert_eq!(class.schema, 3);
                            }
                        }
                    }
                    Err(error) => posture_failure = Some(error),
                }
            }
        });
        if let Some(error) = posture_failure {
            return Err(error);
        }
        instructions += scene.instructions.len();
        scenes += 1;
    }
    ensure!(scenes == 283, "unexpected scene count {scenes}");
    ensure!(
        postures == 152,
        "unexpected inline posture count {postures}"
    );
    println!(
        "Validated {scenes} scenes, {instructions} instructions and {postures} inline postures"
    );
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
    // Walk the startup scheduling chain into the image helper, still without
    // pretending to execute host effects in this unit test.
    for _ in 0..100 {
        if vm.cursor().is_some_and(|pc| pc.scene == 88) {
            break;
        }
        if matches!(vm.step()?, Step::Host { .. }) {
            vm.acknowledge_host()?;
        }
    }
    assert_eq!(vm.cursor().map(|pc| pc.scene), Some(88));
    assert_eq!(vm.arguments()[1].integer()?, 2);
    assert!(matches!(vm.step()?, Step::Host { .. }));
    vm.acknowledge_host()?;
    vm.step()?;
    assert_eq!(vm.arguments()[1].integer()?, 65538);
    let Step::Host { instruction, .. } = vm.step()? else {
        anyhow::bail!("expected camera message");
    };
    let message = vm.message(&instruction)?;
    assert_eq!(message.class, "OM_SetExclusiveCamera");
    assert_eq!(message.fields["排他指定"].integer()?, 65538);
    vm.acknowledge_host()?;
    assert!(matches!(vm.step()?, Step::Host { .. }));
    vm.acknowledge_host()?;
    let Step::Host { instruction, .. } = vm.step()? else {
        anyhow::bail!("expected image message");
    };
    let message = vm.message(&instruction)?;
    assert_eq!(message.class, "OM_HumanIn2");
    let rugp_scene_vm::vm::Value::Reference(image) = &message.fields["ベース画像"] else {
        anyhow::bail!("image parameter is not a resource");
    };
    assert_eq!(image.offset, Some(0xcdefba));
    Ok(())
}
