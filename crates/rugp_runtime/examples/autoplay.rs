//! Headless playability diagnostic: executes the real host, advances dialogue,
//! chooses the first menu item and mutes output while retaining PCM timing.
//! Reports counters/cursors, never retail dialogue or asset data.
use anyhow::{Context, Result};
use rugp_runtime::{Game, RunState, audio::mute_requests};
use std::collections::BTreeSet;

fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game root")?;
    let ticks = std::env::args()
        .nth(2)
        .map_or(Ok(10000), |s| s.parse::<u32>())?;
    let dt = std::env::args()
        .nth(3)
        .map_or(Ok(1000), |s| s.parse::<u32>())?;
    let mut game = Game::open(root.as_ref())?;
    let mut audio = None;
    let mut scenes = BTreeSet::new();
    let mut dialogue_pages = 0;
    let mut choices = 0;
    let mut sounds = 0;
    for tick in 0..ticks {
        if game.dialogue_waits_for_input() {
            game.activate_selection()?;
            dialogue_pages += 1;
        }
        if game.waiting_for_selection() {
            game.navigate_selection(1)?;
            game.activate_selection()?;
            choices += 1;
        }
        sounds += game
            .pending_sounds()
            .iter()
            .filter(|s| s.ticket.is_some())
            .count();
        mute_requests(&mut game, &mut audio)?;
        if let Some(pc) = game.vm.cursor() {
            scenes.insert(pc.scene);
        }
        if tick % 100 == 0 {
            println!(
                "tick={tick} pc={:?} scenes={} pages={dialogue_pages} choices={choices} sounds={sounds}",
                game.vm.cursor(),
                scenes.len()
            );
        }
        match game.step(if tick == 0 {0} else {dt}) {
            Ok(RunState::Ended) => {
                println!("ended tick={tick} scenes={} pages={dialogue_pages} choices={choices} sounds={sounds}", scenes.len());
                return Ok(());
            }
            Ok(_) => {},
            Err(error) => return Err(error.context(format!("autoplay tick={tick}, scenes={}, pages={dialogue_pages}, choices={choices}, sounds={sounds}", scenes.len()))),
        }
    }
    anyhow::bail!(
        "tick limit {ticks} reached at {:?}; scenes={}, pages={dialogue_pages}, choices={choices}, sounds={sounds}",
        game.vm.cursor(),
        scenes.len()
    )
}
