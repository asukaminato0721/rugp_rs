//! Isolated real ending coverage, not a claim that a story route has completed.
use super::*;

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_ending_ten_runs_real_script_and_returns_to_original_title() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
    let mut game = Game::open(root.as_ref())?;
    game.set_scene_rasterization(false);
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
    let native = NativeSchema::from_directory(root.as_ref())?;
    let program = Program::load(&game.archive, &native)?;
    let mut vm = Vm::new(Arc::new(program), 244)?;
    vm.variables = std::mem::take(&mut game.vm.variables);
    // Scene 244's native global selector: the branch containing 3b7c is 10.
    vm.variables.insert(213487, Value::Integer(10));
    game.vm = vm;
    game.selection = None;
    let mut confirmations = 0;
    for tick in 0..2000 {
        if game.dialogue_waits_for_input() {
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
                !game.async_images.is_empty()
                    && game.async_images.iter().all(|fade| fade.finished()),
                "ending fade unfinished"
            );
            ensure!(
                game.image_wait.is_none() && !game.staffroll_wait,
                "ending wait leaked"
            );
            return Ok(());
        }
    }
    bail!(
        "ending probe exceeded tick budget at {:?}",
        game.vm.cursor()
    )
}
