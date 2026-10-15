use super::*;
use rugp_assets::{
    object::{ClassId, ObjectReader},
    script::Script,
};
use rugp_scene_vm::{Argument, Scenario};

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_empty_object_event_differs_from_literal_null_without_inventing_waits() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
    let mut game = Game::open(root.as_ref())?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let scene = Scenario::decode_with_schema(
        &Script::load(&game.archive, 244)?,
        game.archive.address_shift,
        Some(&native),
    )?;
    let query = scene
        .instructions
        .iter()
        .find(|i| i.address == 0x442c)
        .context("empty-event query")?;
    let Argument::Reference(variable) = &query.arguments[0] else {
        bail!("query destination missing")
    };
    let initial = game.vm.evaluate(&query.arguments[0])?;
    assert_eq!(initial.integer()?, 0); // actual CBasicValue default, not a fake event
    let make_vm = |argument: Argument, mode: u32| -> Result<Vm> {
        let mut query = query.clone();
        query.address = 0;
        query.next_address = 24;
        query.arguments[0] = argument;
        query.arguments[1] = Argument::Number(mode);
        query.arguments[2] = Argument::Number(if mode == 32 { u32::MAX } else { 4000 });
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 40,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    query,
                    Instruction {
                        index: 1,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmRet".into(),
                            schema: 17,
                        },
                        address: 24,
                        next_address: u32::MAX,
                        flags: 0,
                        arguments: vec![],
                    },
                ],
            },
        )?;
        Vm::new(Arc::new(program), 1)
    };
    let ticket = game.audio_state.play(1, crate::audio_state::Bus::Effect)?;
    let before = game.frame().to_vec();
    for value in [
        initial,
        Value::Null,
        Value::Integer(7),
        Value::Real(1.25),
        Value::Text("probe".into()),
        Value::Bytes(vec![1, 2]),
    ] {
        for mode in [0, 16, 32] {
            game.vm = make_vm(Argument::Reference(variable.clone()), mode)?;
            game.vm.write_object(variable, value.clone())?;
            game.ended = false;
            assert_eq!(game.step(0)?, RunState::Ended);
            assert_eq!(
                game.vm.synchronization_status,
                (mode == 32).then_some(false)
            );
            assert!(game.wait_ms.is_none() && game.image_wait.is_none());
        }
    }
    let null = ObjectReader::new(&[0; 3]).reference(0)?;
    game.vm = make_vm(Argument::Reference(null), 32)?;
    game.ended = false;
    assert_eq!(game.step(0)?, RunState::Ended);
    assert_eq!(game.vm.synchronization_status, Some(true));
    // Do not turn arbitrary scalar synchronization targets into empty objects.
    let mut scalar = variable.clone();
    scalar.class.as_mut().unwrap().name = "CInt".into();
    game.vm = make_vm(Argument::Reference(scalar.clone()), 32)?;
    game.vm.write_object(&scalar, Value::Integer(0))?;
    game.ended = false;
    assert!(format!("{:#}", game.step(0).unwrap_err()).contains("invalid synchronization target"));
    assert_eq!(game.frame(), before);
    assert!(!game.audio_state.finished(ticket));
    assert!(game.pending_sounds().is_empty());
    Ok(())
}
