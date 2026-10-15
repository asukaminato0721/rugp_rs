//! Executes the real portable host; reports scheduling/frame statistics, never dialogue.
use anyhow::{Context, Result};
use rugp_runtime::{Game, RunState, audio::mute_requests};
fn main() -> Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .context("game directory required")?;
    let limit = std::env::args()
        .nth(2)
        .map_or(Ok(30000), |s| s.parse::<u32>())?;
    let mut game = Game::open(root.as_ref())?;
    let mut audio = None;
    let mut previous = None;
    let mut elapsed = 0;
    loop {
        let state = game.step(if elapsed == 0 { 0 } else { 100 });
        let pc = game.vm.cursor().map(|pc| (pc.scene, pc.address));
        if pc != previous || state.is_err() {
            let rgb: u64 = game
                .frame()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]))
                .sum();
            println!(
                "t={elapsed:05} PC={pc:?} RGBsum={rgb} args={:?}",
                game.vm
                    .arguments()
                    .iter()
                    .map(|v| match v {
                        rugp_scene_vm::vm::Value::Integer(n) => Some(*n),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            );
            previous = pc;
        }
        if state? == RunState::Ended || elapsed >= limit {
            break;
        }
        mute_requests(&mut game, &mut audio)?;
        elapsed += 100;
    }
    Ok(())
}
