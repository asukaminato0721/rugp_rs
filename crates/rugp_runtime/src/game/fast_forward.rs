//! Continuous skip uses normal VM host operations and respects route choices.
use super::*;
impl Game {
    /// Advance dialogue and timers faster. Explicit non-text confirmations,
    /// selector menus, load dialogs, and forced-clock text retain their input.
    pub fn fast_forward_step(&mut self, dt_ms: u32) -> Result<RunState> {
        if self.waiting_for_selection() || self.waiting_for_load() || self.waiting_for_input() {
            return self.step(dt_ms);
        }
        self.skip_video()?;
        if self.dialogue.is_some() {
            self.advance_dialogue()?;
            if self.dialogue_waits_for_input() {
                self.advance_dialogue()?;
            }
        }
        self.step(dt_ms.saturating_mul(8).min(800))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_skip_advances_pages_and_preserves_title_choices() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        game.set_scene_rasterization(false);
        let mut audio = None;
        super::super::test_support::wait_selection(&mut game, &mut audio)?;
        let title_cursor = game.vm.cursor().cloned();
        for _ in 0..10 {
            game.fast_forward_step(100)?;
        }
        assert_eq!(game.vm.cursor(), title_cursor.as_ref());
        assert!(game.waiting_for_selection());
        super::super::test_support::choose_image(&mut game, 1)?;
        let mut pages = std::collections::BTreeSet::new();
        for _ in 0..300 {
            crate::audio::mute_requests(&mut game, &mut audio)?;
            if game.waiting_for_dialogue() {
                let cursor = game.vm.cursor().context("dialogue cursor missing")?;
                pages.insert((cursor.scene, cursor.address));
            }
            game.fast_forward_step(100)?;
            if pages.len() >= 12 {
                break;
            }
            ensure!(!game.ended, "route ended before selection");
        }
        ensure!(pages.len() >= 12, "skip did not advance dialogue pages");
        Ok(())
    }
}
