use anyhow::{Context, Result};
use rugp_runtime::{Game, RunState};
#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn first_logo_is_loaded_by_real_vm_and_faded_to_framebuffer() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let mut game = Game::open(root.as_ref())?;
    assert_eq!(game.step(0)?, RunState::Waiting);
    assert_eq!(
        game.vm.cursor().map(|pc| (pc.scene, pc.address)),
        Some((252, 0x400))
    );
    let original = rugp_assets::image::decode_r6ti(
        &game.archive.read_at(0xcdefba, 23171)?,
        game.archive.address_shift,
    )?;
    assert_eq!(game.size(), (original.width, original.height));
    assert_eq!(game.step(500)?, RunState::Waiting);
    assert_ne!(game.frame(), original.rgba);
    assert_eq!(game.step(500)?, RunState::Waiting);
    assert_eq!(game.frame(), original.rgba);
    assert_eq!(game.take_sounds().len(), 2); // startup stop-all requests; no audio backend in this test
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn startup_snapshot_and_native_white_fade() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let mut game = Game::open(root.as_ref())?;
    let mut audio = None;
    game.step(0)?;
    for _ in 0..100 {
        rugp_runtime::audio::mute_requests(&mut game, &mut audio)?;
        if game
            .vm
            .cursor()
            .is_some_and(|pc| pc.scene == 137 && pc.address == 0x7c)
        {
            break;
        }
        game.step(100)?;
    }
    assert_eq!(
        game.vm.cursor().map(|pc| (pc.scene, pc.address)),
        Some((137, 0x7c))
    );
    assert_eq!(game.vm.arguments()[0].integer()?, 800);
    let before = game.frame().to_vec();
    assert!(before.iter().any(|v| *v != 255));
    assert_eq!(game.step(400)?, RunState::Waiting);
    for (actual, source) in game.frame().iter().zip(&before) {
        assert_eq!(u16::from(*actual), (u16::from(*source) + 255).div_ceil(2));
    }
    // Resumes the script, clears the temporary image area, and installs the
    // serialized white CBg2d background; no host request is manually skipped.
    assert_eq!(game.step(400)?, RunState::Waiting);
    assert!(game.frame().iter().all(|v| *v == 255));
    assert_eq!(
        game.vm.cursor().map(|pc| (pc.scene, pc.address)),
        Some((252, 0xb74))
    );
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn startup_half_scale_bitmap_and_double_focal_length() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let mut game = Game::open(root.as_ref())?;
    let mut audio = None;
    game.step(0)?;
    for _ in 0..100 {
        rugp_runtime::audio::mute_requests(&mut game, &mut audio)?;
        if game
            .vm
            .cursor()
            .is_some_and(|pc| pc.scene == 252 && pc.address == 0xc40)
        {
            break;
        }
        game.step(100)?;
    }
    assert_eq!(
        game.vm.cursor().map(|pc| (pc.scene, pc.address)),
        Some((252, 0xc40))
    );
    let image = rugp_assets::image::decode_r6ti(
        &game.archive.read_at(0xcf677c, 86679)?,
        game.archive.address_shift,
    )?;
    assert_eq!(image.display_scale, 0.5);
    assert_eq!(
        game.objects
            .get(&0x33d23)
            .context("background object missing")?
            .camera
            .focal_length,
        1440
    );
    assert_eq!(game.step(1000)?, RunState::Waiting);
    assert_eq!(game.frame(), image.rgba);
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn resolves_bgm_wrapper_to_cached_pcm() -> Result<()> {
    use rugp_assets::object::{ClassId, ObjectRef};
    use std::sync::Arc;
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let archive = Arc::new(rugp_assets::RioArchive::open(&root)?);
    let mut audio = rugp_runtime::audio::AudioResources::new(archive)?;
    let reference = ObjectRef {
        class: Some(ClassId {
            name: "CPmBgm".into(),
            schema: 0,
        }),
        cache_index: None,
        flags: 0xc108,
        offset: Some(0xd0ba1c),
        size: Some(61),
        owner: None,
        selector: 0,
        inline: None,
    };
    let pcm = audio.load(&reference)?;
    assert_eq!(pcm.sample_rate, 44100);
    assert_eq!(pcm.channels, 2);
    assert!(pcm.pcm.len() > 44100 * 2);
    assert!(Arc::ptr_eq(&pcm, &audio.load(&reference)?));
    println!("BGM: {} stereo PCM frames", pcm.pcm.len() / 2);
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn async_sound_query_wait_tail_and_device_completion_identity() -> Result<()> {
    use rugp_assets::object::{ClassId, ObjectRef};
    use rugp_scene_vm::{
        Argument, Instruction, Scenario,
        vm::{Program, Vm},
    };
    use std::sync::Arc;
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let mut game = Game::open(root.as_ref())?;
    let sound = ObjectRef {
        class: Some(ClassId {
            name: "CPmEffect".into(),
            schema: 0,
        }),
        cache_index: None,
        flags: 0xc108,
        offset: Some(251819724),
        size: Some(60),
        owner: None,
        selector: 0,
        inline: None,
    };
    let mut resources = rugp_runtime::audio::AudioResources::new(Arc::clone(&game.archive))?;
    assert!(!resources.looped(&sound)?);
    let data = resources.load(&sound)?;
    let ms = (data.pcm.len() as u64 / u64::from(data.channels) * 1000)
        .div_ceil(u64::from(data.sample_rate)) as u32;
    assert!(ms > 3);
    let mut resources = Some(resources);
    let make_vm = |kind: u32, flags: u32, parameters: [u32; 3]| -> Result<Vm> {
        let n = Argument::Number;
        let r = || Argument::Reference(sound.clone());
        let mut instructions = Vec::new();
        for (address, name, arguments) in [
            (
                0,
                "CVmSound",
                vec![
                    r(),
                    n(kind),
                    n(flags),
                    n(parameters[0]),
                    n(parameters[1]),
                    n(parameters[2]),
                ],
            ),
            (24, "CVmSync", vec![r(), n(32), n(0)]),
            (48, "CVmSync", vec![r(), n(16), n(0)]),
            (72, "CVmSync", vec![r(), n(32), n(0)]),
            (96, "CVmRet", vec![]),
        ] {
            instructions.push(Instruction {
                index: instructions.len() as u32,
                stream_offset: 0,
                class: ClassId {
                    name: name.into(),
                    schema: 17,
                },
                address,
                next_address: if address == 96 {
                    u32::MAX
                } else {
                    address + 24
                },
                flags: 0,
                arguments,
            });
        }
        let mut program = Program::default();
        program.insert(
            10,
            Scenario {
                object_id: 1,
                memory_size: 120,
                instructions,
                null_records: vec![],
                trailing_bytes: 0,
            },
        )?;
        Vm::new(Arc::new(program), 1)
    };
    game.vm = make_vm(4, 2, [0, 4096, 100])?;
    assert_eq!(game.step(0)?, RunState::Waiting);
    assert_eq!(game.vm.cursor().map(|pc| pc.address), Some(48));
    assert_eq!(game.vm.synchronization_status, Some(false)); // query, not wait
    let elapsed = ms / 3;
    assert_eq!(game.step(elapsed)?, RunState::Waiting);
    rugp_runtime::audio::mute_requests(&mut game, &mut resources)?;
    assert_eq!(game.step(ms - elapsed - 1)?, RunState::Waiting);
    assert_eq!(game.vm.cursor().map(|pc| pc.address), Some(48));
    assert_eq!(game.step(1)?, RunState::Ended);
    assert_eq!(game.vm.synchronization_status, Some(true));

    // Start a separate runtime for the play-and-wait flag (not a CVmSync).
    let mut waiting = Game::open(root.as_ref())?;
    waiting.vm = make_vm(2, 1, [0; 3])?;
    assert_eq!(waiting.step(0)?, RunState::Waiting);
    assert_eq!(waiting.vm.cursor().map(|pc| pc.address), Some(0));
    rugp_runtime::audio::mute_requests(&mut waiting, &mut resources)?;
    assert_eq!(waiting.step(ms - 1)?, RunState::Waiting);
    assert_eq!(waiting.step(1)?, RunState::Ended);

    // A real output backend reports identity completion rather than a duration.
    let mut device = Game::open(root.as_ref())?;
    device.vm = make_vm(4, 1, [4096, 0, 100])?;
    assert_eq!(device.step(0)?, RunState::Waiting);
    let request = device.take_sounds().pop().context("queued sound missing")?;
    device.finish_audio_ticket(request.ticket.context("runtime ticket missing")?);
    assert_eq!(device.step(0)?, RunState::Ended);
    assert_eq!(device.vm.synchronization_status, Some(true));
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn startup_movie_uses_real_sync_and_fractional_frame_clock() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let mut game = Game::open(root.as_ref())?;
    let mut audio = None;
    game.step(0)?;
    for _ in 0..150 {
        rugp_runtime::audio::mute_requests(&mut game, &mut audio)?;
        if game.video().is_some() {
            break;
        }
        game.step(100)?;
    }
    let movie_pc = game.vm.cursor().cloned();
    assert_eq!(
        movie_pc.as_ref().map(|pc| (pc.scene, pc.address)),
        Some((252, 0xd30))
    );
    let player = game.video().context("startup movie missing")?;
    assert_eq!(player.frame_index(), 0);
    assert_eq!(player.duration_ms(), 117034);
    assert_eq!(game.frame(), player.image().rgba);
    assert_eq!(game.step(33)?, RunState::Waiting);
    assert_eq!(game.video().unwrap().frame_index(), 0);
    assert_eq!(game.step(1)?, RunState::Waiting);
    assert_eq!(game.video().unwrap().frame_index(), 1);
    assert_eq!(game.step(33)?, RunState::Waiting);
    assert_eq!(game.video().unwrap().frame_index(), 2); // temporal frame
    assert_eq!(game.vm.cursor(), movie_pc.as_ref());
    let stream = rugp_assets::crsi::Crsi::read(&game.archive, 0xd0ba5f, 234130539)?;
    let mut previous = None;
    for i in 0..=2 {
        previous = Some(rugp_assets::crsi::codec::decode_frame(
            stream.width,
            stream.height,
            &stream.frames[i],
            &stream.frame_data(&game.archive, 0xd0ba5f, i)?,
            previous.as_ref(),
        )?);
    }
    assert_eq!(game.frame(), previous.unwrap().rgba);
    // Catch up without decoding the whole two-minute movie into memory, then
    // keep the synchronization instruction pending until its exact end.
    assert_eq!(game.step(116966)?, RunState::Waiting);
    assert_eq!(game.video().unwrap().frame_index(), 3510);
    assert!(!game.video().unwrap().finished());
    assert_eq!(game.vm.cursor(), movie_pc.as_ref());
    // A sentinel proves the database query really writes its output rather
    // than just leaving the variable's original zero in place.
    game.vm
        .variables
        .insert(213487, rugp_scene_vm::vm::Value::Integer(99));
    game.step(1)?;
    assert!(game.video().unwrap().finished());
    assert_ne!(game.vm.cursor(), movie_pc.as_ref());
    assert_eq!(game.vm.variables[&213487].integer()?, 0);
    assert!(game.waiting_for_selection());
    Ok(())
}

fn open_title() -> Result<Game> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let mut game = Game::open(root.as_ref())?;
    let mut audio = None;
    game.step(0)?;
    for _ in 0..150 {
        rugp_runtime::audio::mute_requests(&mut game, &mut audio)?;
        if game.video().is_some() {
            break;
        }
        game.step(100)?;
    }
    game.step(117034)?;
    assert!(game.waiting_for_selection());
    let starting = game.frame().to_vec();
    game.step(1000)?;
    assert_ne!(game.frame(), starting);
    Ok(game)
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn title_menu_real_mask_mouse_keyboard_and_vm_result() -> Result<()> {
    let mut game = open_title()?;
    let pc = game.vm.cursor().cloned();
    assert_eq!(
        pc.as_ref().map(|p| (p.scene, p.address)),
        Some((252, 0xfa0))
    );
    let normal = game.frame().to_vec();
    game.step(5000)?;
    assert_eq!(game.vm.cursor(), pc.as_ref());
    assert_eq!(game.frame(), normal); // asynchronous title fade cannot erase buttons
    game.pointer_move(200, 217)?;
    game.pointer_button(true)?;
    game.pointer_button(false)?;
    assert!(game.waiting_for_selection());
    assert_eq!(game.frame(), normal);
    game.pointer_move(200, 100)?;
    assert_ne!(game.frame(), normal);
    let hover = game.frame().to_vec();
    game.pointer_button(true)?;
    assert_ne!(game.frame(), hover);
    game.pointer_move(-1, -1)?;
    game.pointer_button(false)?;
    assert!(game.waiting_for_selection());
    assert_eq!(game.frame(), normal);
    game.navigate_selection(1)?;
    assert_eq!(game.frame(), hover);
    game.navigate_selection(1)?;
    game.navigate_selection(1)?; // skip the disabled third item
    game.navigate_selection(-1)?;
    game.navigate_selection(-1)?;
    assert_eq!(game.frame(), hover);
    game.pointer_move(200, 100)?;
    game.pointer_button(true)?;
    game.pointer_button(false)?;
    assert!(!game.waiting_for_selection());
    assert_eq!(game.vm.variables[&213487].integer()?, 1);
    assert_eq!(
        game.vm.cursor().map(|p| (p.scene, p.address)),
        Some((252, 0xfbc))
    );
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn title_keyboard_activation_returns_the_numeric_selector_not_the_mask_id() -> Result<()> {
    let mut game = open_title()?;
    game.navigate_selection(1)?;
    game.navigate_selection(1)?;
    game.navigate_selection(1)?; // disabled ID 3 is skipped, ID 4 is selected
    game.activate_selection()?;
    assert!(!game.waiting_for_selection());
    assert_eq!(game.vm.variables[&213487].integer()?, 4);
    assert_eq!(
        game.vm.cursor().map(|p| (p.scene, p.address)),
        Some((252, 0xfbc))
    );
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME; requires an installed Japanese font or RUGP_TEST_FONT"]
fn chapter_one_dialogue_blocks_renders_and_advances() -> Result<()> {
    let mut game = open_title()?;
    if let Some(font) = std::env::var_os("RUGP_TEST_FONT") {
        game.set_font_file(font.as_ref(), 0)?;
    }
    game.navigate_selection(1)?;
    game.activate_selection()?;
    let mut audio = None;
    for _ in 0..100 {
        rugp_runtime::audio::mute_requests(&mut game, &mut audio)?;
        game.step(100)?;
        if game.waiting_for_dialogue() {
            break;
        }
    }
    assert!(game.dialogue_waits_for_input());
    assert_eq!(
        game.vm.cursor().map(|pc| (pc.scene, pc.address)),
        Some((483, 0x118))
    );
    assert_eq!(game.objects[&212741].rect, (88, 402, 848, 190)); // script overrides resource defaults
    game.step(1000)?; // finish any asynchronous scene fade
    let first = game.frame().to_vec();
    assert!(game.objects[&212741].visible);
    assert!(
        first
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[..3] == [24, 24, 120])
    ); // native configured backdrop over black
    assert!(
        first
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[..3] == [255, 255, 255])
    ); // actual glyph coverage
    game.step(10000)?;
    assert!(game.frame() == first);
    assert_eq!(game.vm.cursor().map(|pc| pc.address), Some(0x118));
    // A missing/cancelled press (including focus loss) never advances text.
    game.pointer_button(false)?;
    game.pointer_move(300, 500)?;
    game.pointer_button(true)?;
    game.pointer_move(-1, -1)?;
    game.pointer_button(false)?;
    assert!(game.dialogue_waits_for_input());
    game.pointer_move(300, 500)?;
    game.pointer_button(true)?;
    game.pointer_button(false)?;
    assert!(!game.waiting_for_dialogue());
    game.step(0)?;
    assert_eq!(
        game.vm.cursor().map(|pc| (pc.scene, pc.address)),
        Some((483, 0x1c8))
    );
    assert!(game.dialogue_waits_for_input());
    assert!(
        game.frame() != first,
        "new dialogue must update presented pixels"
    );
    game.activate_selection()?; // keyboard advances the real pending message
    for _ in 0..100 {
        rugp_runtime::audio::mute_requests(&mut game, &mut audio)?;
        match game.step(100)? {
            RunState::Ended => panic!("chapter ended before next message"),
            _ if game.waiting_for_dialogue() => break,
            _ => {}
        }
    }
    assert!(game.dialogue_waits_for_input());
    assert_ne!(game.vm.cursor().map(|pc| pc.address), Some(0x1c8));
    // Follow real page input through sound status queries to the next boundary.
    let mut boundary = None;
    for _ in 0..100 {
        if game.dialogue_waits_for_input() {
            game.activate_selection()?;
        }
        rugp_runtime::audio::mute_requests(&mut game, &mut audio)?;
        match game.step(100) {
            Err(error) => {
                boundary = Some(error);
                break;
            }
            Ok(RunState::Ended) => panic!("chapter ended before pending synchronization"),
            _ => {}
        }
    }
    let boundary = boundary.context("expected next unsupported host operation")?;
    let camera = game
        .objects
        .get(&212259)
        .context("chapter background missing")?
        .camera;
    assert_eq!(camera.focal_length, 1752);
    assert_eq!(camera.angles_q4, [0, -32, 0]);
    let layers = game.objects[&212259].world_layers;
    assert!(layers.draws_in_sprite_pass(0));
    for slot in 1..=3 {
        assert!(!layers.draws_in_sprite_pass(slot));
    }
    assert_eq!(
        game.vm.cursor().map(|pc| (pc.scene, pc.address)),
        Some((88, 0x7c))
    );
    assert!(
        game.vm.synchronization_status.is_some(),
        "sound queries must write VM status"
    );
    assert!(
        format!("{boundary:#}").contains("background needs 0-plane/motion rendering"),
        "{boundary:#}"
    );
    let ramp = game
        .take_sounds()
        .into_iter()
        .find(|r| r.kind == 4)
        .context("chapter must queue the real ramped playback")?;
    assert!(ramp.ticket.is_some());
    assert_eq!(ramp.parameters, [0, 4096, 2500]);
    assert_eq!(ramp.gain_envelope()?.duration_ms(), 2500);
    if let rugp_scene_vm::vm::Step::Host { instruction, .. } = game.vm.step()? {
        assert_eq!(instruction.class.name, "CVmGenericMsg");
        let message = game.vm.message(&instruction)?;
        assert_eq!(message.class, "OM_HumanIn2");
        let rugp_scene_vm::vm::Value::Reference(image) = &message.fields["ベース画像"] else {
            panic!("pending background is not a resource reference");
        };
        assert_eq!(image.class.as_ref().unwrap().name, "CBg2d");
        assert_eq!(image.offset, Some(251819850));
    } else {
        panic!("unsupported background composition was acknowledged");
    }
    Ok(())
}
