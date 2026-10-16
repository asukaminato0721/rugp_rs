//! Original ending tails, earned extras and alternate first-chapter playthroughs.
//! Isolated prerequisite tails do not establish their earlier story prefixes.
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

macro_rules! earned_first_chapter {
    ($name:ident, $prefix:literal) => {
        #[test]
        #[ignore = "set RUGP_TEST_GAME; long earned first-chapter alternate-route playback"]
        fn $name() -> Result<()> {
            check_ending_followup(249, 0, false, false, Some($prefix))
        }
    };
}

earned_first_chapter!(retail_earned_first_chapter_if_213_returns_to_title, 213);
earned_first_chapter!(retail_earned_first_chapter_if_229_returns_to_title, 229);
earned_first_chapter!(retail_earned_first_chapter_if_304_returns_to_title, 304);
earned_first_chapter!(retail_earned_first_chapter_if_369_returns_to_title, 369);

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
    check_ending_followup(scene, variant, rasterize, play_extra, None)
}

fn check_ending_followup(
    scene: usize,
    variant: i32,
    rasterize: bool,
    play_extra: bool,
    first_chapter: Option<usize>,
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
            if let Some(prefix) = first_chapter {
                return check_earned_first_chapter(
                    &mut game,
                    &mut audio,
                    title.as_ref().context("original title cursor")?,
                    prefix,
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

fn check_earned_first_chapter(
    game: &mut Game,
    audio: &mut Option<crate::audio::AudioResources>,
    title: &rugp_scene_vm::vm::Cursor,
    prefix: usize,
) -> Result<()> {
    use std::collections::BTreeSet;
    // The preceding original True End tail earns chapter-clear permission.
    // Enter through New Game and its actual four-item post-clear chooser.
    // The native third option sets alternate mode 246878; the test never does.
    choose_image(game, 1)?;
    wait_selection(game, audio)?;
    ensure!(
        game.vm
            .cursor()
            .is_some_and(|pc| pc.scene == 252 && pc.address == 4204),
        "earned new-game chapter chooser missing"
    );
    let target = game
        .selection_text_target()
        .context("chapter chooser is not text")?;
    ensure!(
        game.text_menus[&target].entries.len() == 4,
        "earned chapter chooser does not expose alternate mode plus cancel"
    );
    for _ in 0..3 {
        game.navigate_selection(1)?;
    }
    game.activate_selection()?;
    let mut pages = 0;
    let mut confirmations = 0;
    let mut choices = 0;
    let mut scenes = BTreeSet::new();
    let mut saw_credits = false;
    for tick in 0..20000 {
        if let Some(pc) = game.vm.cursor() {
            scenes.insert(pc.scene);
            if pc.scene == 238
                && game
                    .staffroll
                    .as_ref()
                    .is_some_and(|(_, roll)| !roll.finished())
            {
                saw_credits = true;
            }
        }
        if game.waiting_for_selection() {
            if game.vm.cursor() == Some(title) {
                ensure!(
                    pages > 0 && scenes.contains(&prefix),
                    "alternate route prefix {prefix} was not played"
                );
                ensure!(
                    saw_credits
                        && scenes.contains(&238)
                        && game
                            .staffroll
                            .as_ref()
                            .is_some_and(|(_, roll)| roll.finished())
                        && !game.staffroll_wait
                        && game.image_wait.is_none(),
                    "alternate first-chapter ending/credits did not finish"
                );
                eprintln!(
                    "earned first-chapter {prefix} returned: tick={tick}, pages={pages}, choices={choices}, confirmations={confirmations}, scenes={}",
                    scenes.len()
                );
                return Ok(());
            }
            ensure!(
                game.selection_text_target().is_some(),
                "alternate chapter reached an unexpected image menu"
            );
            let pc = game.vm.cursor().context("alternate choice cursor")?;
            // Native choices select the early branch, the later school branch,
            // or the final first-chapter branch. The two second choices for 229
            // leave its original counter at zero; the first choices for 369
            // let the native script increment it twice. No plot/counter writes.
            let index = match (pc.scene, pc.address, prefix) {
                (222, 8612, 213) => 0,
                (222, 8612, _) => 1,
                (487, 46324, 229 | 369) => 0,
                (487, 46324, 304) | (469, 14348, 304) => 1,
                (368, 63108, 229) | (375, 18028, 229) => 1,
                _ => 0,
            };
            eprintln!("earned first-chapter {prefix}: choose {index} at {pc:?}");
            for _ in 0..=index {
                game.navigate_selection(1)?;
            }
            game.activate_selection()?;
            choices += 1;
        }
        if game.dialogue_waits_for_input() {
            ensure!(
                game.vm.variables[&246878].integer()? == 1,
                "native alternate mode was not selected"
            );
            game.activate_selection()?;
            pages += 1;
        }
        if game.waiting_for_input() {
            game.activate_selection()?;
            confirmations += 1;
        }
        crate::audio::mute_requests(game, audio)?;
        if tick % 1000 == 0 {
            eprintln!(
                "earned first-chapter {prefix}: tick={tick}, pages={pages}, pc={:?}",
                game.vm.cursor()
            );
        }
        ensure!(
            game.step(1000).with_context(|| format!(
                "alternate first-chapter {prefix}, tick {tick}, pages {pages}"
            ))? != RunState::Ended,
            "alternate first-chapter exited instead of returning to title"
        );
    }
    bail!(
        "alternate first-chapter {prefix} budget exceeded at {:?}",
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
