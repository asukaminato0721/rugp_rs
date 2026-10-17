//! Host input state. Modal screens and focus loss cancel continuous input.
use winit::keyboard::KeyCode;

#[derive(Default)]
pub(super) struct Controls {
    left: bool,
    right: bool,
    /// 読んだ文章を飛ばす is passing read pages now (after confirming one).
    reading_skip: bool,
    automatic: bool,
    auto_elapsed: u32,
}
impl Controls {
    pub fn key(&mut self, key: KeyCode, down: bool) {
        if down {
            self.automatic = false;
            self.auto_elapsed = 0;
        }
        match key {
            KeyCode::ControlLeft => self.left = down,
            KeyCode::ControlRight => self.right = down,
            _ => {}
        }
    }
    /// Ctrl skips everything only while held (retail manual).
    pub fn active(&self) -> bool {
        self.left || self.right
    }
    pub fn reading_skip(&self) -> bool {
        self.reading_skip
    }
    /// Pass read pages until unread text, a choice or other input.
    pub fn set_reading_skip(&mut self, skipping: bool) {
        self.reading_skip = skipping;
    }
    pub fn toggle_auto(&mut self) {
        let automatic = !self.automatic;
        self.stop();
        self.automatic = automatic;
    }
    pub fn automatic(&self) -> bool {
        self.automatic
    }
    /// Start the reading delay after both reveal and voice playback finish.
    /// Each ready result permits exactly one dialogue confirmation.
    pub fn auto_ready(&mut self, waiting: bool, voice: bool, dt: u32, delay: u32) -> bool {
        if !self.automatic || !waiting || voice {
            self.auto_elapsed = 0;
            return false;
        }
        self.auto_elapsed = self.auto_elapsed.saturating_add(dt);
        if self.auto_elapsed < delay {
            return false;
        }
        self.auto_elapsed = 0;
        true
    }
    pub fn stop(&mut self) {
        *self = Self::default();
    }
}
/// Preserve fractional milliseconds and slow-frame time for VM/media clocks.
pub(super) fn elapsed_ms(duration: std::time::Duration) -> (u32, std::time::Duration) {
    let whole = duration.as_millis().min(u128::from(u32::MAX)) as u32;
    (
        whole,
        duration - std::time::Duration::from_millis(u64::from(whole)),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_clock_keeps_submillisecond_and_slow_frame_time() {
        use std::time::Duration;
        let mut remainder = Duration::ZERO;
        let mut total = 0;
        for _ in 0..60 {
            let (whole, rest) = elapsed_ms(Duration::from_nanos(16_666_667) + remainder);
            total += whole;
            remainder = rest;
        }
        assert_eq!(total, 1000);
        assert_eq!(remainder, Duration::from_nanos(20));
        assert_eq!(elapsed_ms(Duration::from_millis(537)).0, 537);
    }
    #[test]
    fn both_control_keys_release_independently_and_modal_input_cannot_stick() {
        let mut controls = Controls::default();
        controls.key(KeyCode::ControlLeft, true);
        controls.key(KeyCode::ControlRight, true);
        controls.key(KeyCode::ControlLeft, false);
        assert!(controls.active());
        controls.key(KeyCode::ControlRight, false);
        assert!(!controls.active());
        controls.set_reading_skip(true);
        assert!(controls.reading_skip() && !controls.active());
        controls.stop();
        assert!(!controls.reading_skip());
    }
    #[test]
    fn auto_waits_for_reveal_and_voice_and_cancels_for_skip_or_modal_input() {
        let mut controls = Controls::default();
        controls.toggle_auto();
        assert!(!controls.auto_ready(false, false, 10000, 1500));
        assert!(!controls.auto_ready(true, true, 10000, 1500));
        assert!(!controls.auto_ready(true, false, 1000, 1500));
        assert!(!controls.auto_ready(true, true, 500, 1500));
        assert!(!controls.auto_ready(true, false, 1000, 1500));
        assert!(controls.auto_ready(true, false, 500, 1500));
        assert!(!controls.auto_ready(true, false, 1, 1500));
        assert!(controls.auto_ready(true, false, u32::MAX, 1500));
        assert!(!controls.auto_ready(true, false, 1, 1500));
        controls.set_reading_skip(true);
        controls.toggle_auto();
        assert!(!controls.reading_skip() && !controls.automatic());
        controls.toggle_auto();
        assert!(controls.automatic());
        controls.key(KeyCode::ControlLeft, true);
        assert!(!controls.automatic());
        controls.toggle_auto();
        controls.stop();
        assert!(!controls.automatic());
    }
}
