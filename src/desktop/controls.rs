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
#[cfg(test)]
mod tests {
    use super::*;
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
