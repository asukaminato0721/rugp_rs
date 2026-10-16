//! Session dialogue history. This view never advances or rewinds the VM.
use super::save_panel::{rect, text};
use anyhow::Result;
use rugp_runtime::{dialogue::TextPart, font::TextFont, render::Framebuffer};
use std::collections::VecDeque;
use winit::keyboard::KeyCode;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    Previous,
    Next,
    Close,
}
#[derive(Default)]
pub(super) struct History {
    pages: VecDeque<String>,
    bytes: usize,
    last: Option<((usize, u32), String)>,
    pub opened: bool,
    entry: usize,
    line: usize,
    pressed: Option<Hit>,
}
impl History {
    pub fn record(&mut self, cursor: (usize, u32), parts: &[TextPart]) {
        let mut body = String::new();
        for part in parts {
            match part {
                TextPart::Glyph(ch) => body.push(*ch),
                TextPart::Newline => body.push('\n'),
                TextPart::Speaker(name) => {
                    body.push('【');
                    body.push_str(name);
                    body.push_str("】\n");
                }
            }
        }
        if body.trim().is_empty()
            || self
                .last
                .as_ref()
                .is_some_and(|(key, text)| *key == cursor && *text == body)
        {
            return;
        }
        self.last = Some((cursor, body.clone()));
        self.bytes += body.len();
        self.pages.push_back(body);
        while self.pages.len() > 256 || self.bytes > 2 * 1024 * 1024 {
            self.bytes -= self.pages.pop_front().unwrap().len();
        }
    }
    pub fn open(&mut self) {
        self.opened = true;
        self.entry = self.pages.len().saturating_sub(1);
        self.line = 0;
        self.pressed = None;
    }
    pub fn close(&mut self) {
        self.opened = false;
        self.pressed = None;
    }
    pub fn forget_cursor(&mut self) {
        self.last = None;
    }
    pub fn cancel_press(&mut self) {
        self.pressed = None;
    }
    pub fn move_entry(&mut self, direction: i32) {
        self.entry = if direction < 0 {
            self.entry.saturating_sub(1)
        } else {
            (self.entry + 1).min(self.pages.len().saturating_sub(1))
        };
        self.line = 0;
    }
    pub fn key(&mut self, key: KeyCode) -> bool {
        self.cancel_press();
        match key {
            KeyCode::Escape | KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space => {
                return true;
            }
            KeyCode::PageUp | KeyCode::ArrowLeft => self.move_entry(-1),
            KeyCode::PageDown | KeyCode::ArrowRight => self.move_entry(1),
            KeyCode::ArrowUp => self.line = self.line.saturating_sub(1),
            KeyCode::ArrowDown => self.line = self.line.saturating_add(1),
            KeyCode::Home => self.line = 0,
            KeyCode::End => self.line = usize::MAX,
            _ => {}
        }
        false
    }
    fn hit(x: i32, y: i32) -> Option<Hit> {
        if !(493..531).contains(&y) {
            return None;
        }
        match x {
            170..=369 => Some(Hit::Previous),
            412..=611 => Some(Hit::Next),
            654..=853 => Some(Hit::Close),
            _ => None,
        }
    }
    pub fn button(&mut self, x: i32, y: i32, down: bool) -> bool {
        let hit = Self::hit(x, y);
        if down {
            self.pressed = hit;
            return false;
        }
        let pressed = self.pressed.take();
        if hit.is_none() || pressed != hit {
            return false;
        }
        match hit.unwrap() {
            Hit::Previous => self.move_entry(-1),
            Hit::Next => self.move_entry(1),
            Hit::Close => return true,
        }
        false
    }
    pub fn render(
        &mut self,
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
        text(
            &mut frame,
            font,
            170,
            70,
            &format!(
                "文章履歴   {}/{}",
                if self.pages.is_empty() {
                    0
                } else {
                    self.entry + 1
                },
                self.pages.len()
            ),
        )?;
        let body = self
            .pages
            .get(self.entry)
            .map_or("履歴はまだありません。", String::as_str);
        let mut lines = vec![String::new()];
        let mut width = 0;
        for ch in body.chars() {
            if ch == '\n' {
                lines.push(String::new());
                width = 0;
                continue;
            }
            let advance = font.glyph(ch)?.advance;
            if width + advance > 680 && width > 0 {
                lines.push(String::new());
                width = 0;
            }
            lines.last_mut().unwrap().push(ch);
            width += advance;
        }
        self.line = self.line.min(lines.len().saturating_sub(16));
        for (row, line) in lines.iter().skip(self.line).take(16).enumerate() {
            text(&mut frame, font, 170, 110 + row as i32 * 23, line)?;
        }
        for (x, label) in [
            (170, "← 前の文章"),
            (412, "次の文章 →"),
            (654, "ゲームに戻る"),
        ] {
            rect(&mut frame, (x, 493, x + 200, 531), [38, 93, 125, 255]);
            text(&mut frame, font, x + 12, 500, label)?;
        }
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_keeps_distinct_pages_bounds_memory_and_requires_a_paired_release() {
        let mut history = History::default();
        history.record(
            (1, 2),
            &[TextPart::Speaker("名前".into()), TextPart::Glyph('文')],
        );
        history.record(
            (1, 2),
            &[TextPart::Speaker("名前".into()), TextPart::Glyph('文')],
        );
        assert_eq!(history.pages.len(), 1);
        assert_eq!(history.pages[0], "【名前】\n文");
        for n in 0..300 {
            history.record((1, n), &[TextPart::Glyph('次')]);
        }
        assert_eq!(history.pages.len(), 256);
        history.open();
        assert_eq!(history.entry, 255);
        history.key(KeyCode::PageUp);
        assert_eq!(history.entry, 254);
        history.button(700, 510, true);
        assert!(!history.button(0, 0, false));
        assert!(!history.button(700, 510, false));
        history.button(700, 510, true);
        history.cancel_press();
        assert!(!history.button(700, 510, false));
        history.button(700, 510, true);
        assert!(history.button(700, 510, false));
    }
}
