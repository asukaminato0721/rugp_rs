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
