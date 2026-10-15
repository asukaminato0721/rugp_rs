//! Desktop-only slot chooser. Its pixels and input never enter game snapshots.
use anyhow::Result;
use rugp_runtime::{font::TextFont, render::Framebuffer};
use winit::keyboard::KeyCode;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Save,
    Load,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    None,
    Cancel,
    Save(u8),
    Load(u8),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    Slot(u8),
    Confirm,
    Cancel,
}
pub(super) struct Panel {
    pub mode: Mode,
    pub slot: u8,
    occupied: [bool; 10],
    overwrite: bool,
    pressed: Option<Hit>,
    pub notice: String,
}
impl Panel {
    pub fn new(mode: Mode, slot: u8, occupied: [bool; 10]) -> Self {
        Self {
            mode,
            slot: slot.min(9),
            occupied,
            overwrite: false,
            pressed: None,
            notice: String::new(),
        }
    }
    fn select(&mut self, slot: u8) {
        self.slot = slot;
        self.overwrite = false;
        self.notice.clear();
    }
    fn confirm(&mut self) -> Action {
        if self.mode == Mode::Load {
            if !self.occupied[self.slot as usize] {
                self.notice = "This slot is empty.".into();
                return Action::None;
            }
            return Action::Load(self.slot);
        }
        if self.occupied[self.slot as usize] && !self.overwrite {
            self.overwrite = true;
            self.notice = "Replace this save? Press Enter or Save again.".into();
            return Action::None;
        }
        Action::Save(self.slot)
    }
    pub fn key(&mut self, key: KeyCode) -> Action {
        self.pressed = None;
        match key {
            KeyCode::Escape => return Action::Cancel,
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space => return self.confirm(),
            KeyCode::ArrowDown | KeyCode::ArrowRight | KeyCode::Tab => {
                self.select((self.slot + 1) % 10)
            }
            KeyCode::ArrowUp | KeyCode::ArrowLeft => self.select((self.slot + 9) % 10),
            KeyCode::Digit0 | KeyCode::Numpad0 => self.select(0),
            KeyCode::Digit1 | KeyCode::Numpad1 => self.select(1),
            KeyCode::Digit2 | KeyCode::Numpad2 => self.select(2),
            KeyCode::Digit3 | KeyCode::Numpad3 => self.select(3),
            KeyCode::Digit4 | KeyCode::Numpad4 => self.select(4),
            KeyCode::Digit5 | KeyCode::Numpad5 => self.select(5),
            KeyCode::Digit6 | KeyCode::Numpad6 => self.select(6),
            KeyCode::Digit7 | KeyCode::Numpad7 => self.select(7),
            KeyCode::Digit8 | KeyCode::Numpad8 => self.select(8),
            KeyCode::Digit9 | KeyCode::Numpad9 => self.select(9),
            _ => {}
        }
        Action::None
    }
    fn hit(x: i32, y: i32) -> Option<Hit> {
        if (170..850).contains(&x) && (118..428).contains(&y) {
            return Some(Hit::Slot(((y - 118) / 31) as u8));
        }
        if (464..502).contains(&y) {
            if (580..850).contains(&x) {
                return Some(Hit::Confirm);
            }
            if (170..440).contains(&x) {
                return Some(Hit::Cancel);
            }
        }
        None
    }
    pub fn button(&mut self, x: i32, y: i32, down: bool) -> Action {
        let hit = Self::hit(x, y);
        if down {
            self.pressed = hit;
            return Action::None;
        }
        let previous = self.pressed.take();
        if hit.is_none() || hit != previous {
            return Action::None;
        }
        match hit.unwrap() {
            Hit::Slot(slot) => {
                self.select(slot);
                Action::None
            }
            Hit::Confirm => self.confirm(),
            Hit::Cancel => Action::Cancel,
        }
    }
    pub fn cancel_press(&mut self) {
        self.pressed = None;
    }
    pub fn render(
        &self,
        pixels: &[u8],
        size: (u32, u32),
        font: &mut TextFont,
    ) -> Result<Framebuffer> {
        let mut frame = Framebuffer::new(size.0, size.1)?;
        frame.rgba.copy_from_slice(pixels);
        rect(
            &mut frame,
            (0, 0, size.0 as i32, size.1 as i32),
            [0, 0, 0, 160],
        );
        rect(&mut frame, (140, 48, 884, 548), [18, 26, 38, 255]);
        let mode = if self.mode == Mode::Save {
            "SAVE"
        } else {
            "LOAD"
        };
        text(
            &mut frame,
            font,
            170,
            70,
            &format!("{mode} GAME  -  Portable Rust slots"),
        )?;
        for slot in 0..10 {
            let y = 118 + i32::from(slot) * 31;
            if slot == self.slot {
                rect(&mut frame, (170, y, 850, y + 30), [38, 93, 125, 255]);
            }
            let status = if self.occupied[slot as usize] {
                "Saved"
            } else {
                "Empty"
            };
            text(&mut frame, font, 186, y + 2, &format!("{slot}   {status}"))?;
        }
        text(&mut frame, font, 170, 435, &self.notice)?;
        rect(&mut frame, (170, 464, 440, 502), [48, 58, 74, 255]);
        rect(&mut frame, (580, 464, 850, 502), [38, 93, 125, 255]);
        text(&mut frame, font, 195, 470, "Cancel  [Esc]")?;
        text(&mut frame, font, 605, 470, &format!("{mode}  [Enter]"))?;
        text(
            &mut frame,
            font,
            170,
            514,
            "0-9 / Arrows: slot    F5: Save    F9: Load",
        )?;
        Ok(frame)
    }
}
fn rect(frame: &mut Framebuffer, bounds: (i32, i32, i32, i32), color: [u8; 4]) {
    for y in bounds.1..bounds.3 {
        for x in bounds.0..bounds.2 {
            frame.pixel(x, y, color);
        }
    }
}
fn text(
    frame: &mut Framebuffer,
    font: &mut TextFont,
    mut x: i32,
    y: i32,
    text: &str,
) -> Result<()> {
    for ch in text.chars() {
        let glyph = font.glyph(ch)?;
        if x + glyph.advance > 865 {
            break;
        }
        for gy in 0..glyph.height {
            for gx in 0..glyph.width {
                frame.pixel(
                    x + glyph.left + gx as i32,
                    y + glyph.top + gy as i32,
                    [242, 246, 252, glyph.alpha[(gy * glyph.width + gx) as usize]],
                );
            }
        }
        x += glyph.advance;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_slots_overwrite_confirmation_navigation_and_held_release_are_safe() {
        let mut occupied = [false; 10];
        occupied[1] = true;
        let mut panel = Panel::new(Mode::Load, 0, occupied);
        assert_eq!(panel.key(KeyCode::Enter), Action::None);
        panel.key(KeyCode::ArrowDown);
        assert_eq!(panel.key(KeyCode::Enter), Action::Load(1));
        assert_eq!(panel.button(600, 480, false), Action::None);
        panel.button(600, 480, true);
        panel.cancel_press();
        assert_eq!(panel.button(600, 480, false), Action::None);
        let mut panel = Panel::new(Mode::Save, 1, occupied);
        assert_eq!(panel.key(KeyCode::Enter), Action::None);
        panel.key(KeyCode::ArrowDown);
        panel.key(KeyCode::ArrowUp);
        assert_eq!(panel.key(KeyCode::Enter), Action::None);
        assert_eq!(panel.key(KeyCode::Enter), Action::Save(1));
        panel.button(180, 120, true);
        panel.button(180, 120, false);
        assert_eq!(panel.key(KeyCode::Enter), Action::Save(0));
        assert_eq!(panel.key(KeyCode::Escape), Action::Cancel);
    }
}
