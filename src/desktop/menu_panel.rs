//! Right-click menu. Settings live in the popup config window. Every pointer action needs a paired release.
use super::save_panel::{rect, text};
use anyhow::Result;
use rugp_runtime::{font::TextFont, render::Framebuffer};
use winit::keyboard::KeyCode;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Page {
    Context,
    Quit,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    None,
    Resume,
    HideText,
    Auto,
    History,
    Load,
    Settings,
    Skip,
    QuitPrompt,
    Quit,
}
pub(super) struct MenuPanel {
    pub page: Page,
    selected: usize,
    pressed: Option<usize>,
}
impl MenuPanel {
    pub fn new(page: Page) -> Self {
        Self {
            page,
            selected: 0,
            pressed: None,
        }
    }
    fn count(&self) -> usize {
        match self.page {
            Page::Context => 8,
            Page::Quit => 2,
        }
    }
    fn activate(&self) -> Action {
        match self.page {
            Page::Context => [
                Action::Resume,
                Action::HideText,
                Action::Skip,
                Action::Auto,
                Action::History,
                Action::Load,
                Action::Settings,
                Action::QuitPrompt,
            ]
            .into_iter()
            .nth(self.selected)
            .unwrap(),
            Page::Quit => {
                if self.selected == 0 {
                    Action::Resume
                } else {
                    Action::Quit
                }
            }
        }
    }
    pub fn key(&mut self, key: KeyCode) -> Action {
        self.pressed = None;
        match key {
            KeyCode::Escape | KeyCode::F2 => return Action::Resume,
            KeyCode::ArrowUp => self.selected = (self.selected + self.count() - 1) % self.count(),
            KeyCode::ArrowDown | KeyCode::Tab => self.selected = (self.selected + 1) % self.count(),
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space => return self.activate(),
            _ => {}
        }
        Action::None
    }
    fn hit(&self, x: i32, y: i32) -> Option<usize> {
        if !(240..784).contains(&x) || y < 110 {
            return None;
        }
        let row = ((y - 110) / 40) as usize;
        (row < self.count() && (y - 110) % 40 < 36).then_some(row)
    }
    pub fn pointer(&mut self, x: i32, y: i32) {
        if let Some(row) = self.hit(x, y) {
            self.selected = row;
        }
    }
    pub fn button(&mut self, x: i32, y: i32, down: bool) -> Action {
        let row = self.hit(x, y);
        if down {
            self.pressed = row;
            return Action::None;
        }
        let pressed = self.pressed.take();
        if row.is_none() || pressed != row {
            return Action::None;
        }
        self.selected = row.unwrap();
        self.activate()
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
        rect(&mut frame, (210, 48, 814, 550), [18, 26, 38, 255]);
        let title = match self.page {
            Page::Context => "メニュー",
            Page::Quit => "ゲームを終了しますか？",
        };
        text(&mut frame, font, 240, 70, title)?;
        let labels: Vec<String> = match self.page {
            Page::Context => [
                "ゲームに戻る",
                "文字を消す   [Space]",
                "読んだ文章を飛ばす   [S]",
                "文章自動送り   [A]",
                "文章履歴   [PageUp]",
                "ロード   [F9]",
                "設定   [F2]",
                "終了",
            ]
            .map(String::from)
            .into(),
            Page::Quit => ["ゲームに戻る", "終了する"].map(String::from).into(),
        };
        for (row, label) in labels.iter().enumerate() {
            let y = 110 + row as i32 * 40;
            rect(
                &mut frame,
                (240, y, 784, y + 36),
                if row == self.selected {
                    [38, 93, 125, 255]
                } else {
                    [30, 40, 54, 255]
                },
            );
            text(&mut frame, font, 258, y + 7, label)?;
        }
        text(
            &mut frame,
            font,
            240,
            518,
            "↑↓: 選択   Enter: 決定   Esc / 右クリック: 戻る",
        )?;
        Ok(frame)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_menu_dispatches_each_action_and_quit_needs_confirmation() {
        let mut panel = MenuPanel::new(Page::Context);
        for expected in [
            Action::Resume,
            Action::HideText,
            Action::Skip,
            Action::Auto,
            Action::History,
            Action::Load,
            Action::Settings,
            Action::QuitPrompt,
        ] {
            assert_eq!(panel.key(KeyCode::Enter), expected);
            panel.key(KeyCode::ArrowDown);
        }
        let mut quit = MenuPanel::new(Page::Quit);
        assert_eq!(quit.key(KeyCode::Enter), Action::Resume);
        quit.key(KeyCode::ArrowDown);
        assert_eq!(quit.key(KeyCode::Enter), Action::Quit);
    }
    #[test]
    fn unpaired_cancelled_or_dragged_releases_do_not_activate() {
        let mut panel = MenuPanel::new(Page::Context);
        assert_eq!(panel.button(600, 120, false), Action::None);
        panel.button(600, 120, true);
        assert_eq!(panel.button(600, 165, false), Action::None);
        panel.button(600, 120, true);
        panel.cancel_press();
        assert_eq!(panel.button(600, 120, false), Action::None);
        panel.button(600, 120, true);
        assert_eq!(panel.button(600, 120, false), Action::Resume);
        assert_eq!(panel.key(KeyCode::Escape), Action::Resume);
    }
}
