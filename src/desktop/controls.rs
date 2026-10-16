//! Host input state. Modal screens and focus loss cancel continuous input.
use winit::keyboard::KeyCode;

#[derive(Default)]
pub(super) struct Controls {
    left: bool,
    right: bool,
    toggle: bool,
}
impl Controls {
    pub fn key(&mut self, key: KeyCode, down: bool) {
        match key {
            KeyCode::ControlLeft => self.left = down,
            KeyCode::ControlRight => self.right = down,
            _ => {}
        }
    }
    pub fn active(&self) -> bool {
        self.left || self.right || self.toggle
    }
    pub fn toggle(&mut self) {
        self.toggle = !self.active();
        self.left = false;
        self.right = false;
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
        controls.toggle();
        assert!(controls.active());
        controls.stop();
        assert!(!controls.active());
    }
}
