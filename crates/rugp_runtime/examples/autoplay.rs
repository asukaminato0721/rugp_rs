//! Headless playability diagnostic: executes the real host, advances dialogue,
//! chooses the first menu item and mutes output while retaining PCM timing.
//! Reports counters/cursors, never retail dialogue or asset data.
use anyhow::{Context, Result};
use rugp_runtime::{Game, RunState, audio::mute_requests};
use rugp_scene_vm::vm::Value;
use std::collections::BTreeSet;

fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Integer(n) => n.to_string(),
        Value::Real(n) => n.to_string(),
        Value::Reference(r) => {
            let class = r
                .class
                .as_ref()
                .map_or("untyped object", |c| c.name.as_str());
            if let Some(offset) = r.offset {
                format!("{class}@{offset:#x}+{}", r.size.unwrap_or(0))
            } else {
                class.into()
            }
        }
        Value::Text(s) => format!("<text: {} bytes>", s.len()),
        Value::Bytes(b) => format!("<bytes: {}>", b.len()),
    }
}

fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game root")?;
    let ticks = std::env::args()
        .nth(2)
        .map_or(Ok(10000), |s| s.parse::<u32>())?;
    let dt = std::env::args()
        .nth(3)
        .map_or(Ok(1000), |s| s.parse::<u32>())?;
    let mut game = Game::open(root.as_ref())?;
    let mut choice_plan = Vec::new();
    let mut stop_on_title_return = false;
    for arg in std::env::args().skip(4) {
        if arg == "--no-scene-raster" {
            eprintln!(
                "Diagnostic: scene rasterization disabled; host logic/resources/timing still run. Not visual verification."
            );
            game.set_scene_rasterization(false);
        } else if let Some(plan) = arg.strip_prefix("--choices=") {
            choice_plan = plan
                .split(',')
                .map(|s| {
                    let index: u8 = s
                        .parse()
                        .context("choice plan requires zero-based indices 0..255")?;
                    Ok(index)
                })
                .collect::<Result<Vec<_>>>()?;
        } else if arg == "--stop-on-title-return" {
            stop_on_title_return = true;
        } else {
            anyhow::bail!("unknown autoplay option {arg}");
        }
    }
    let mut audio = None;
    let mut scenes = BTreeSet::new();
    let mut dialogue_pages = 0;
    let mut choices = 0;
    let mut sounds = 0;
    let mut entry_menu = None;
    // An explicit zero disables the diagnostic budget. Long story routes
    // should not need a fresh replay merely because their length was unknown.
    let mut tick = 0u64;
    while ticks == 0 || tick < u64::from(ticks) {
        if game.dialogue_waits_for_input() {
            game.activate_selection()?;
            dialogue_pages += 1;
        }
        if game.waiting_for_selection() {
            if choices == 0 {
                entry_menu = game.vm.cursor().cloned();
            } else if stop_on_title_return
                && dialogue_pages > 0
                && entry_menu.is_some()
                && game.vm.cursor() == entry_menu.as_ref()
            {
                println!(
                    "returned-to-title tick={tick} scenes={} pages={dialogue_pages} choices={choices} sounds={sounds}",
                    scenes.len()
                );
                return Ok(());
            }
            // Plan includes the title menu (index 0 starts a new game).
            // Unspecified choices select the first enabled entry.
            for _ in 0..=choice_plan.get(choices).copied().unwrap_or(0) {
                game.navigate_selection(1)?;
            }
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
        match game.step(if tick == 0 { 0 } else { dt }) {
            Ok(RunState::Ended) => {
                println!(
                    "ended tick={tick} scenes={} pages={dialogue_pages} choices={choices} sounds={sounds}",
                    scenes.len()
                );
                return Ok(());
            }
            Ok(_) => {}
            Err(error) => {
                if let Some(ins) = game.vm.pending_instruction()
                    && ins.class.name == "CVmGenericMsg"
                    && let Ok(message) = game.vm.message(ins)
                {
                    eprintln!(
                        "host {} target={}: {:?}",
                        message.class,
                        describe(&message.target),
                        message
                            .fields
                            .iter()
                            .map(|(key, value)| (key, describe(value)))
                            .collect::<std::collections::BTreeMap<_, _>>()
                    );
                } else if let Some(ins) = game.vm.pending_instruction()
                    && ins.class.name == "CVmSync"
                {
                    eprintln!(
                        "host CVmSync: {:?}",
                        ins.arguments
                            .iter()
                            .map(|arg| game
                                .vm
                                .evaluate(arg)
                                .map_or("<unresolved>".into(), |value| describe(&value)))
                            .collect::<Vec<_>>()
                    );
                }
                return Err(error.context(format!("autoplay tick={tick}, scenes={}, pages={dialogue_pages}, choices={choices}, sounds={sounds}", scenes.len())));
            }
        }
        tick += 1;
    }
    anyhow::bail!(
        "tick limit {ticks} reached at {:?}; scenes={}, pages={dialogue_pages}, choices={choices}, sounds={sounds}",
        game.vm.cursor(),
        scenes.len()
    )
}
