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
