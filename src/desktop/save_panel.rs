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
    Save(u32),
    Load(u32),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    Slot(usize),
    Confirm,
    Cancel,
}
pub(super) struct Panel {
    pub mode: Mode,
    pub slot: usize,
    slots: Vec<(u32, bool)>,
    pressed: Option<Hit>,
    pub notice: String,
    /// システム: execute a slot by double-clicking it.
    pub double_click: bool,
    last_click: Option<(usize, std::time::Instant)>,
}
impl Panel {
    pub fn new(mode: Mode, slot: usize, slots: Vec<(u32, bool)>) -> Self {
        Self {
            mode,
            slot: slot.min(slots.len() - 1),
            slots,
            pressed: None,
            double_click: false,
            last_click: None,
            notice: if mode == Mode::Save {
                "原版セーブの書き込みは未対応です。".into()
            } else {
                String::new()
            },
        }
    }
    fn select(&mut self, slot: usize) {
        self.slot = slot;
        if self.mode == Mode::Load {
            self.notice.clear();
        }
    }
    fn confirm(&mut self) -> Action {
        if self.mode == Mode::Load {
            if !self.slots[self.slot].1 {
                self.notice = "このセーブは空です。".into();
                return Action::None;
            }
            return Action::Load(self.slots[self.slot].0);
        }
        Action::Save(self.slots[self.slot].0)
    }
    pub fn key(&mut self, key: KeyCode) -> Action {
        self.pressed = None;
        match key {
            KeyCode::Escape => return Action::Cancel,
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space => return self.confirm(),
            KeyCode::ArrowDown | KeyCode::ArrowRight | KeyCode::Tab => {
                self.select((self.slot + 1) % self.slots.len())
            }
            KeyCode::ArrowUp | KeyCode::ArrowLeft => {
                self.select((self.slot + self.slots.len() - 1) % self.slots.len())
            }
            KeyCode::PageDown => self.select((self.slot + 10).min(self.slots.len() - 1)),
            KeyCode::PageUp => self.select(self.slot.saturating_sub(10)),
            KeyCode::Digit0 | KeyCode::Numpad0 => {
                self.select((self.slot / 10 * 10).min(self.slots.len() - 1))
            }
            KeyCode::Digit1 | KeyCode::Numpad1 => {
                self.select((self.slot / 10 * 10 + 1).min(self.slots.len() - 1))
            }
            KeyCode::Digit2 | KeyCode::Numpad2 => {
                self.select((self.slot / 10 * 10 + 2).min(self.slots.len() - 1))
            }
            KeyCode::Digit3 | KeyCode::Numpad3 => {
                self.select((self.slot / 10 * 10 + 3).min(self.slots.len() - 1))
            }
            KeyCode::Digit4 | KeyCode::Numpad4 => {
                self.select((self.slot / 10 * 10 + 4).min(self.slots.len() - 1))
            }
            KeyCode::Digit5 | KeyCode::Numpad5 => {
                self.select((self.slot / 10 * 10 + 5).min(self.slots.len() - 1))
            }
            KeyCode::Digit6 | KeyCode::Numpad6 => {
                self.select((self.slot / 10 * 10 + 6).min(self.slots.len() - 1))
            }
            KeyCode::Digit7 | KeyCode::Numpad7 => {
                self.select((self.slot / 10 * 10 + 7).min(self.slots.len() - 1))
            }
            KeyCode::Digit8 | KeyCode::Numpad8 => {
                self.select((self.slot / 10 * 10 + 8).min(self.slots.len() - 1))
            }
            KeyCode::Digit9 | KeyCode::Numpad9 => {
                self.select((self.slot / 10 * 10 + 9).min(self.slots.len() - 1))
            }
            _ => {}
        }
        Action::None
    }
    fn hit(&self, x: i32, y: i32) -> Option<Hit> {
        if (170..850).contains(&x) && (118..428).contains(&y) {
            let index = self.slot / 10 * 10 + ((y - 118) / 31) as usize;
            return (index < self.slots.len()).then_some(Hit::Slot(index));
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
        let hit = self.hit(x, y);
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
                let now = std::time::Instant::now();
                let repeated = self.last_click.is_some_and(|(previous, at)| {
                    previous == slot && now.duration_since(at).as_millis() < 500
                });
                self.last_click = Some((slot, now));
                if self.double_click && repeated {
                    self.last_click = None;
                    return self.confirm();
                }
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
            &format!("{mode}  -  原版 UUC セーブ"),
        )?;
        for slot in (self.slot / 10 * 10)..((self.slot / 10 * 10 + 10).min(self.slots.len())) {
            let y = 118 + (slot % 10) as i32 * 31;
            if slot == self.slot {
                rect(&mut frame, (170, y, 850, y + 30), [38, 93, 125, 255]);
            }
            let status = if self.slots[slot].1 { "Saved" } else { "Empty" };
            text(
                &mut frame,
                font,
                186,
                y + 2,
                &format!("vmr{:05}.uuc   {status}", self.slots[slot].0),
            )?;
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
            "↑↓: 選択   PgUp/PgDn: ページ   F5: Save   F9: Load",
        )?;
        Ok(frame)
    }
}
pub(super) fn rect(frame: &mut Framebuffer, bounds: (i32, i32, i32, i32), color: [u8; 4]) {
    for y in bounds.1..bounds.3 {
        for x in bounds.0..bounds.2 {
            frame.pixel(x, y, color);
        }
    }
}
pub(super) fn text(
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
    fn empty_slots_navigation_and_held_release_are_safe() {
        let mut occupied: Vec<_> = (1..=10).map(|n| (n, false)).collect();
        occupied[1].1 = true;
        let mut panel = Panel::new(Mode::Load, 0, occupied.clone());
        assert_eq!(panel.key(KeyCode::Enter), Action::None);
        panel.key(KeyCode::ArrowDown);
        assert_eq!(panel.key(KeyCode::Enter), Action::Load(2));
        assert_eq!(panel.button(600, 480, false), Action::None);
        panel.button(600, 480, true);
        panel.cancel_press();
        assert_eq!(panel.button(600, 480, false), Action::None);
        let mut panel = Panel::new(Mode::Save, 1, occupied);
        assert_eq!(panel.key(KeyCode::Enter), Action::Save(2));
        panel.key(KeyCode::ArrowDown);
        panel.key(KeyCode::ArrowUp);
        assert_eq!(panel.key(KeyCode::Enter), Action::Save(2));
        panel.button(180, 120, true);
        panel.button(180, 120, false);
        assert_eq!(panel.key(KeyCode::Enter), Action::Save(1));
        assert_eq!(panel.key(KeyCode::Escape), Action::Cancel);
        panel.button(180, 120, true);
        assert_eq!(panel.button(180, 120, false), Action::None);
        panel.double_click = true;
        panel.button(180, 120, true);
        assert_eq!(panel.button(180, 120, false), Action::Save(1));
    }
}
