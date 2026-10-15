//! Real menu input helpers: never edit selector flags or VM result variables.
use super::*;

pub(super) fn wait_selection(
    game: &mut Game,
    audio: &mut Option<crate::audio::AudioResources>,
) -> Result<()> {
    for _ in 0..500 {
        crate::audio::mute_requests(game, audio)?;
        let state = game.step(1000)?;
        if game.waiting_for_selection() {
            return Ok(());
        }
        ensure!(state != RunState::Ended, "game exited instead of selecting");
    }
    bail!("menu selection budget exceeded at {:?}", game.vm.cursor())
}

pub(super) fn choose_image(game: &mut Game, id: u32) -> Result<()> {
    let target = game.selection.as_ref().context("selection missing")?.target;
    let menu = game.objects[&target].menu.context("image menu missing")?;
    ensure!(
        game.menus[&menu]
            .data
            .selectors
            .iter()
            .any(|s| s.id == id && s.status & 1 != 0),
        "menu selector {id} is disabled"
    );
    for _ in 0..=game.menus[&menu].data.selectors.len() {
        game.navigate_selection(1)?;
        if game.menus[&menu]
            .hover
            .and_then(|key| game.menus[&menu].result(key))
            == Some(id)
        {
            return game.activate_selection();
        }
    }
    bail!("menu selector {id} was not reached")
}
