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
