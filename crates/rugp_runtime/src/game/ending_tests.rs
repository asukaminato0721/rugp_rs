//! Isolated real ending coverage, not a claim that a story route has completed.
use super::test_support::{choose_image, wait_selection};
use super::*;

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_ending_ten_runs_real_script_and_returns_to_original_title() -> Result<()> {
    check_ending(244, 10, true)
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_ending_three_unlocks_title_premium_and_returns_without_fabricated_progress() -> Result<()>
{
    check_ending(244, 3, true)
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_three_true_ending_tails_unlock_bonus_choices_and_cancel_back() -> Result<()> {
    // Original terminal scenes, not a claim to have played each route prefix.
    // Each writes its own clear state through the unchanged native script.
    for scene in [249, 241, 246] {
        check_ending(scene, 0, false).with_context(|| format!("true ending tail {scene}"))?;
    }
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME; long original extra-route playback"]
fn retail_three_earned_extra_routes_play_original_scripts_and_return() -> Result<()> {
    for scene in [249, 241, 246] {
        check_ending_with_extra(scene, 0, false, true)
            .with_context(|| format!("earned extra route after ending {scene}"))?;
    }
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_first_chapter_tail_does_not_unlock_second_chapter_premium() -> Result<()> {
    check_ending(238, 0, false)
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_credit_branches_zero_through_seven_return_to_original_title() -> Result<()> {
    for variant in 0..8 {
        check_ending(244, variant, false).with_context(|| format!("ending branch {variant}"))?;
    }
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_credit_pan_branches_eight_and_nine_return_to_original_title() -> Result<()> {
    for variant in 8..10 {
        check_ending(244, variant, true).with_context(|| format!("ending branch {variant}"))?;
    }
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_alternate_ending_runs_epilogue_roll_and_returns_to_original_title() -> Result<()> {
    check_ending(249, 0, true)
}

fn check_ending(scene: usize, variant: i32, rasterize: bool) -> Result<()> {
    check_ending_with_extra(scene, variant, rasterize, false)
}

fn check_ending_with_extra(
    scene: usize,
    variant: i32,
    rasterize: bool,
    play_extra: bool,
) -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
    let mut game = Game::open(root.as_ref())?;
    game.set_scene_rasterization(rasterize);
    let mut audio = None;
    // Establish real startup windows/settings before entering the ending.
    // No bypassed host messages or fabricated completion values.
    for _ in 0..200 {
        crate::audio::mute_requests(&mut game, &mut audio)?;
        game.step(10000)?;
        if game.waiting_for_selection() {
            break;
        }
    }
    ensure!(game.waiting_for_selection(), "startup did not reach title");
    let title = game.vm.cursor().cloned();
    let true_ending = matches!(scene, 241 | 246 | 249);
    if (scene == 244 && variant == 3) || true_ending || scene == 238 {
        let target = game
            .selection
            .as_ref()
            .context("title selection missing")?
            .target;
        let menu = game.objects[&target]
            .menu
            .context("title image menu missing")?;
        ensure!(
            game.menus[&menu]
                .data
                .selectors
                .iter()
                .any(|s| s.id == 3 && s.status & 1 == 0),
            "fresh-profile premium was already unlocked"
        );
    }
    let native = NativeSchema::from_directory(root.as_ref())?;
    let program = Program::load(&game.archive, &native)?;
    let mut vm = Vm::new(Arc::new(program), scene)?;
    vm.variables = std::mem::take(&mut game.vm.variables);
    // Scene 244's native global selector (0..10); 10 contains 3b7c.
    if scene == 244 {
        vm.variables.insert(213487, Value::Integer(variant));
    }
    game.vm = vm;
    game.selection = None;
    let mut confirmations = 0;
    for tick in 0..2000 {
        if game.dialogue_waits_for_input() || game.waiting_for_input() {
            game.activate_selection()?;
            confirmations += 1;
        }
        crate::audio::mute_requests(&mut game, &mut audio)?;
        let state = game.step(10000).with_context(|| {
            format!(
                "ending probe tick {tick}, confirmations {confirmations}, cursor {:?}",
                game.vm.cursor()
            )
        })?;
        if state == RunState::Ended || game.waiting_for_selection() {
            ensure!(
                game.waiting_for_selection() && game.vm.cursor() == title.as_ref(),
                "ending did not return to the original title selection: {:?}",
                game.vm.cursor()
            );
            ensure!(
                game.staffroll
                    .as_ref()
                    .is_some_and(|(_, roll)| roll.finished()),
                "ending roll unfinished"
            );
            ensure!(
                (scene != 244 || variant == 0 || !game.async_images.is_empty())
                    && game.async_images.iter().all(|fade| fade.finished()),
                "ending fade unfinished"
            );
            ensure!(
                game.image_wait.is_none() && !game.staffroll_wait,
                "ending wait leaked"
            );
            if (scene == 244 && variant == 3) || true_ending {
                // The original ending path writes its real CStdb clear
                // state. Enter through the title's gated selector, rather
                // than injecting another VM entry or editing unlock flags.
                choose_image(&mut game, 3)?;
                wait_selection(&mut game, &mut audio)?;
                ensure!(
                    game.vm.cursor().is_some_and(|pc| pc.scene == 251),
                    "earned premium entry missing"
                );
                if true_ending {
                    let hub = game.vm.cursor().cloned();
                    choose_image(&mut game, 5)?;
                    wait_selection(&mut game, &mut audio)?;
                    let target = game
                        .selection_text_target()
                        .context("earned bonus choices missing")?;
                    ensure!(
                        game.text_menus[&target].entries.len() == 2,
                        "earned bonus did not expose exactly one route plus cancel"
                    );
                    game.navigate_selection(-1)?;
                    game.activate_selection()?;
                    wait_selection(&mut game, &mut audio)?;
                    ensure!(
                        game.vm.cursor() == hub.as_ref(),
                        "bonus cancel did not return to original hub"
                    );
                    if play_extra {
                        let entry = match scene {
                            249 => 202,
                            241 => 264,
                            246 => 349,
                            _ => unreachable!(),
                        };
                        return check_earned_extra(
                            &mut game,
                            &mut audio,
                            title.as_ref().unwrap(),
                            hub.as_ref().unwrap(),
                            entry,
                        );
                    }
                }
                choose_image(&mut game, 6)?;
                wait_selection(&mut game, &mut audio)?;
                ensure!(
                    game.vm.cursor() == title.as_ref(),
                    "earned premium did not return to original title"
                );
            }
            if scene == 238 {
                let target = game
                    .selection
                    .as_ref()
                    .context("title selection missing")?
                    .target;
                let menu = game.objects[&target].menu.context("title menu missing")?;
                ensure!(
                    game.menus[&menu]
                        .data
                        .selectors
                        .iter()
                        .any(|s| s.id == 3 && s.status & 1 == 0),
                    "first-chapter completion unlocked second-chapter premium"
                );
            }
            return Ok(());
        }
    }
    bail!(
        "ending probe exceeded tick budget at {:?}",
        game.vm.cursor()
    )
}

fn check_earned_extra(
    game: &mut Game,
    audio: &mut Option<crate::audio::AudioResources>,
    title: &rugp_scene_vm::vm::Cursor,
    hub: &rugp_scene_vm::vm::Cursor,
    entry: usize,
) -> Result<()> {
    // Re-enter after testing Cancel, select the one legitimately earned route.
    // No test writes a clear flag or substitutes a VM entry here.
    choose_image(game, 5)?;
    wait_selection(game, audio)?;
    let target = game
        .selection_text_target()
        .context("earned route menu missing")?;
    ensure!(
        game.text_menus[&target].entries.len() == 2,
        "earned route list changed"
    );
    game.navigate_selection(1)?;
    game.activate_selection()?;
    let mut entered = false;
    let mut pages = 0;
    let mut confirmations = 0;
    let mut choices = 0;
    for tick in 0u32..100_000 {
        entered |= game.vm.cursor().is_some_and(|pc| pc.scene == entry);
        if game.waiting_for_selection() {
            if game.vm.cursor() == Some(title) || game.vm.cursor() == Some(hub) {
                ensure!(
                    entered && pages > 0,
                    "extra returned without playing its original entry"
                );
                if game.vm.cursor() == Some(hub) {
                    choose_image(game, 6)?;
                    wait_selection(game, audio)?;
                }
                ensure!(
                    game.vm.cursor() == Some(title),
                    "extra did not return to original title"
                );
                eprintln!(
                    "earned extra {entry} returned: ticks={tick}, pages={pages}, confirmations={confirmations}, choices={choices}"
                );
                return Ok(());
            }
            game.navigate_selection(1)?;
            game.activate_selection()?;
            choices += 1;
        }
        if game.dialogue_waits_for_input() {
            game.activate_selection()?;
            pages += 1;
        }
        if game.waiting_for_input() {
            game.activate_selection()?;
            confirmations += 1;
        }
        crate::audio::mute_requests(game, audio)?;
        let state = game
            .step(1000)
            .with_context(|| format!("extra {entry}, tick {tick}, pc {:?}", game.vm.cursor()))?;
        ensure!(
            state != RunState::Ended,
            "extra ended instead of returning to a menu"
        );
        if tick.is_multiple_of(1000) {
            eprintln!(
                "extra {entry}: tick={tick}, pages={pages}, pc={:?}",
                game.vm.cursor()
            );
        }
    }
    bail!(
        "extra {entry} exceeded diagnostic budget at {:?}",
        game.vm.cursor()
    )
}
