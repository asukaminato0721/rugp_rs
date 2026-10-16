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
