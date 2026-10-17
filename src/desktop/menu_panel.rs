//! Right-click menu after the retail Vm60.dll CMessBox popup (1000f730): a
//! Win32 TrackPopupMenu at the cursor, drawn in the XP flat-menu style.
//! Confirmations are drawn as MB_YESNO message boxes. Every pointer action
//! needs a paired release.
use super::save_panel::rect;
use anyhow::Result;
use rugp_runtime::{font::TextFont, render::Framebuffer};
use winit::keyboard::KeyCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Page {
    Context,
    /// メニュー(&M) shows the retail menu bar; the port has none, so its
    /// 設定 and 終了 items open as a second popup.
    MenuBar,
    Restart,
    Quit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    None,
    Resume,
    HideText,
    Skip,
    Auto,
    History,
    Save,
    Load,
    RestartPrompt,
    Restart,
    MenuBar,
    Settings,
    QuitPrompt,
    Quit,
}
/// What the popup greys out or checks when it opens.
#[derive(Clone, Copy, Default)]
pub(super) struct State {
    pub read_skip: bool,
    pub auto: bool,
    pub save: bool,
    pub load: bool,
    pub restart: bool,
}
struct Item {
    /// Retail caption; `&` marks the mnemonic and `\t` starts the accelerator.
    caption: &'static str,
    action: Action,
    enabled: bool,
    checked: bool,
}
impl Item {
    fn new(caption: &'static str, action: Action, enabled: bool) -> Option<Self> {
        Some(Self {
            caption,
            action,
            enabled,
            checked: false,
        })
    }
    fn label(&self) -> (String, &str) {
        let (label, accel) = self.caption.split_once('\t').unwrap_or((self.caption, ""));
        (label.replace('&', ""), accel)
    }
    fn mnemonic(&self) -> Option<char> {
        let (_, after) = self.caption.split_once('&')?;
        after.chars().next().map(|c| c.to_ascii_uppercase())
    }
}

const FONT: i32 = 12;
const ITEM: i32 = 19;
const SEPARATOR: i32 = 9;
/// 1px border plus 2px padding, as XP flat menus draw them.
const FRAME: i32 = 3;
const GUTTER: i32 = 21;
const ACCEL_GAP: i32 = 24;
const SHADOW: i32 = 4;
const MENU: [u8; 4] = [255, 255, 255, 255];
const BORDER: [u8; 4] = [172, 168, 153, 255];
const HIGHLIGHT: [u8; 4] = [49, 106, 197, 255];
const TEXT: [u8; 3] = [0, 0, 0];
const HIGHLIGHT_TEXT: [u8; 3] = [255, 255, 255];
const GRAY_TEXT: [u8; 3] = [172, 168, 153];
const FACE: [u8; 4] = [236, 233, 216, 255];
const CAPTION: [u8; 4] = [0, 84, 227, 255];
const BOX: (i32, i32) = (300, 116);
const BUTTON: (i32, i32) = (75, 23);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    Item(usize),
    Inside,
    Outside,
}
pub(super) struct MenuPanel {
    pub page: Page,
    entries: Vec<Option<Item>>,
    anchor: (i32, i32),
    /// Popup or message-box bounds from the last render.
    bounds: (i32, i32, i32, i32),
    selected: Option<usize>,
    pressed: Option<Hit>,
}
impl MenuPanel {
    pub fn new(page: Page, anchor: (i32, i32), state: State) -> Self {
        let entries = match page {
            Page::Context => vec![
                Item::new("ウインドウを消す(&H)", Action::HideText, true),
                Item::new("読んだ文章を飛ばす(&K)", Action::Skip, true).map(|mut item| {
                    item.checked = state.read_skip;
                    item
                }),
                Item::new("自動文字送り(&A)", Action::Auto, true).map(|mut item| {
                    item.checked = state.auto;
                    item
                }),
                Item::new("テキスト履歴を読む(&B)", Action::History, true),
                None,
                Item::new("ゲームセーブ(&S)...", Action::Save, state.save),
                Item::new("ゲームロード(&L)...", Action::Load, state.load),
                Item::new(
                    "ゲームの一番最初に戻る(&R)...",
                    Action::RestartPrompt,
                    state.restart,
                ),
                None,
                Item::new("メニュー(&M)\tAlt", Action::MenuBar, true),
            ],
            Page::MenuBar => vec![
                Item::new("設定(&C)...", Action::Settings, true),
                None,
                Item::new("終了(&X)", Action::QuitPrompt, true),
            ],
            Page::Restart => vec![
                Item::new("はい(&Y)", Action::Restart, true),
                Item::new("いいえ(&N)", Action::Resume, true),
            ],
            Page::Quit => vec![
                Item::new("はい(&Y)", Action::Quit, true),
                Item::new("いいえ(&N)", Action::Resume, true),
            ],
        };
        Self {
            selected: matches!(page, Page::Restart | Page::Quit).then_some(0),
            page,
            entries,
            anchor,
            bounds: (0, 0, 0, 0),
            pressed: None,
        }
    }
    pub fn anchor(&self) -> (i32, i32) {
        self.anchor
    }
    fn dialog(&self) -> bool {
        matches!(self.page, Page::Restart | Page::Quit)
    }
    fn activate(&self, index: usize) -> Action {
        match &self.entries[index] {
            Some(item) if item.enabled => item.action,
            _ => Action::None,
        }
    }
    /// Next item in `step` direction, skipping separators; Win32 still
    /// highlights greyed items.
    fn step(&mut self, step: isize) {
        let count = self.entries.len() as isize;
        let mut at = self
            .selected
            .map_or(if step > 0 { -1 } else { count }, |i| i as isize);
        for _ in 0..count {
            at = (at + step).rem_euclid(count);
            if self.entries[at as usize].is_some() {
                self.selected = Some(at as usize);
                return;
            }
        }
    }
    pub fn key(&mut self, key: KeyCode) -> Action {
        self.pressed = None;
        match key {
            KeyCode::Escape | KeyCode::F2 => return Action::Resume,
            KeyCode::ArrowUp | KeyCode::ArrowLeft => self.step(-1),
            KeyCode::ArrowDown | KeyCode::ArrowRight | KeyCode::Tab => self.step(1),
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space => {
                return self.selected.map_or(Action::None, |i| self.activate(i));
            }
            _ => {
                let Some(letter) = letter(key) else {
                    return Action::None;
                };
                if let Some(i) = self
                    .entries
                    .iter()
                    .position(|e| e.as_ref().and_then(Item::mnemonic) == Some(letter))
                {
                    self.selected = Some(i);
                    return self.activate(i);
                }
            }
        }
        Action::None
    }
    fn hit(&self, x: i32, y: i32) -> Hit {
        let (left, top, right, bottom) = self.bounds;
        if !(left..right).contains(&x) || !(top..bottom).contains(&y) {
            return Hit::Outside;
        }
        if self.dialog() {
            return (0..2)
                .find(|&i| {
                    let (bx, by) = self.button_origin(i);
                    (bx..bx + BUTTON.0).contains(&x) && (by..by + BUTTON.1).contains(&y)
                })
                .map_or(Hit::Inside, Hit::Item);
        }
        let mut row = top + FRAME;
        for (i, entry) in self.entries.iter().enumerate() {
            let height = if entry.is_some() { ITEM } else { SEPARATOR };
            if (row..row + height).contains(&y) && (left + FRAME..right - FRAME).contains(&x) {
                return if entry.is_some() {
                    Hit::Item(i)
                } else {
                    Hit::Inside
                };
            }
            row += height;
        }
        Hit::Inside
    }
    pub fn pointer(&mut self, x: i32, y: i32) {
        match self.hit(x, y) {
            Hit::Item(i) => self.selected = Some(i),
            // Message boxes keep their focused button.
            _ if self.dialog() => {}
            _ => self.selected = None,
        }
    }
    pub fn button(&mut self, x: i32, y: i32, down: bool) -> Action {
        let hit = self.hit(x, y);
        if down {
            self.pressed = Some(hit);
            return Action::None;
        }
        if self.pressed.take() != Some(hit) {
            return Action::None;
        }
        match hit {
            Hit::Item(i) => {
                self.selected = Some(i);
                self.activate(i)
            }
            // A click elsewhere dismisses a popup but not a message box.
            Hit::Outside if !self.dialog() => Action::Resume,
            _ => Action::None,
        }
    }
    pub fn cancel_press(&mut self) {
        self.pressed = None;
    }
    fn button_origin(&self, index: usize) -> (i32, i32) {
        let (left, _, right, bottom) = self.bounds;
        let middle = (left + right) / 2;
        let x = if index == 0 {
            middle - BUTTON.0 - 3
        } else {
            middle + 3
        };
        (x, bottom - 12 - BUTTON.1)
    }
    pub fn render(
        &mut self,
        pixels: &[u8],
        size: (u32, u32),
        font: &mut TextFont,
        caption: &str,
    ) -> Result<Framebuffer> {
        let mut frame = Framebuffer::new(size.0, size.1)?;
        frame.rgba.copy_from_slice(pixels);
        let (width, height) = (size.0 as i32, size.1 as i32);
        if self.dialog() {
            let (left, top) = ((width - BOX.0) / 2, (height - BOX.1) / 2);
            self.bounds = (left, top, left + BOX.0, top + BOX.1);
            self.render_dialog(&mut frame, font, caption)?;
        } else {
            let mut label_width = 0;
            let mut accel_width = 0;
            let mut rows = 0;
            for entry in &self.entries {
                if let Some(item) = entry {
                    let (label, accel) = item.label();
                    label_width = label_width.max(measure(font, &label)?);
                    accel_width = accel_width.max(measure(font, accel)?);
                    rows += ITEM;
                } else {
                    rows += SEPARATOR;
                }
            }
            if accel_width > 0 {
                accel_width += ACCEL_GAP;
            }
            let w = 2 * FRAME + 2 * GUTTER + label_width + accel_width;
            let h = 2 * FRAME + rows;
            // TrackPopupMenu flips the popup to stay on screen.
            let place = |at: i32, extent: i32, limit: i32| {
                if at + extent <= limit {
                    at
                } else {
                    at - extent
                }
                .clamp(0, (limit - extent).max(0))
            };
            let (left, top) = (
                place(self.anchor.0, w, width),
                place(self.anchor.1, h, height),
            );
            self.bounds = (left, top, left + w, top + h);
            self.render_popup(&mut frame, font, accel_width)?;
        }
        Ok(frame)
    }
    fn render_popup(&self, frame: &mut Framebuffer, font: &mut TextFont, accel: i32) -> Result<()> {
        let (left, top, right, bottom) = self.bounds;
        rect(
            frame,
            (left + SHADOW, top + SHADOW, right + SHADOW, bottom + SHADOW),
            [0, 0, 0, 72],
        );
        rect(frame, (left, top, right, bottom), BORDER);
        rect(frame, (left + 1, top + 1, right - 1, bottom - 1), MENU);
        let mut y = top + FRAME;
        for (i, entry) in self.entries.iter().enumerate() {
            let Some(item) = entry else {
                let line = y + SEPARATOR / 2;
                rect(
                    frame,
                    (left + FRAME + 1, line, right - FRAME - 1, line + 1),
                    BORDER,
                );
                y += SEPARATOR;
                continue;
            };
            let color = if self.selected == Some(i) {
                rect(frame, (left + FRAME, y, right - FRAME, y + ITEM), HIGHLIGHT);
                HIGHLIGHT_TEXT
            } else if item.enabled {
                TEXT
            } else {
                GRAY_TEXT
            };
            if item.checked {
                check(frame, left + FRAME + 6, y + (ITEM - 7) / 2, color);
            }
            let (label, accel_text) = item.label();
            let text_y = y + (ITEM - FONT) / 2;
            draw(frame, font, left + FRAME + GUTTER, text_y, &label, color)?;
            if !accel_text.is_empty() {
                let x = right - FRAME - GUTTER - accel + ACCEL_GAP;
                draw(frame, font, x, text_y, accel_text, color)?;
            }
            y += ITEM;
        }
        Ok(())
    }
    fn render_dialog(
        &self,
        frame: &mut Framebuffer,
        font: &mut TextFont,
        caption: &str,
    ) -> Result<()> {
        let (left, top, right, bottom) = self.bounds;
        rect(
            frame,
            (left + SHADOW, top + SHADOW, right + SHADOW, bottom + SHADOW),
            [0, 0, 0, 72],
        );
        rect(frame, (left, top, right, bottom), CAPTION);
        rect(frame, (left + 3, top + 24, right - 3, bottom - 3), FACE);
        draw(frame, font, left + 8, top + 6, caption, HIGHLIGHT_TEXT)?;
        let message = match self.page {
            Page::Quit => "終了します。よろしいですか？",
            _ => "ゲームの一番最初に戻ります。よろしいですか？",
        };
        draw(frame, font, left + 20, top + 44, message, TEXT)?;
        for (i, entry) in self.entries.iter().enumerate() {
            let Some(item) = entry else { continue };
            let (x, y) = self.button_origin(i);
            let focused = self.selected == Some(i);
            rect(
                frame,
                (x, y, x + BUTTON.0, y + BUTTON.1),
                if focused {
                    [0, 60, 116, 255]
                } else {
                    [0, 60, 116, 160]
                },
            );
            rect(
                frame,
                (x + 1, y + 1, x + BUTTON.0 - 1, y + BUTTON.1 - 1),
                [252, 252, 250, 255],
            );
            if focused {
                rect(
                    frame,
                    (x + 1, y + 1, x + BUTTON.0 - 1, y + 3),
                    [246, 196, 86, 255],
                );
                rect(
                    frame,
                    (x + 1, y + BUTTON.1 - 3, x + BUTTON.0 - 1, y + BUTTON.1 - 1),
                    [229, 151, 0, 255],
                );
            }
            let (label, _) = item.label();
            let text_width = measure(font, &label)?;
            draw(
                frame,
                font,
                x + (BUTTON.0 - text_width) / 2,
                y + (BUTTON.1 - FONT) / 2,
                &label,
                TEXT,
            )?;
        }
        Ok(())
    }
}
fn letter(key: KeyCode) -> Option<char> {
    use KeyCode::*;
    let keys = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO,
        KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    let index = keys.iter().position(|k| *k == key)?;
    Some((b'A' + index as u8) as char)
}
fn measure(font: &mut TextFont, text: &str) -> Result<i32> {
    text.chars()
        .try_fold(0, |width, ch| Ok(width + font.glyph(ch)?.advance))
}
fn draw(
    frame: &mut Framebuffer,
    font: &mut TextFont,
    mut x: i32,
    y: i32,
    text: &str,
    color: [u8; 3],
) -> Result<()> {
    let clip = [0, 0, frame.width as i32, frame.height as i32];
    for ch in text.chars() {
        let glyph = font.glyph(ch)?;
        glyph.paint(frame, x, y, clip, color);
        x += glyph.advance;
    }
    Ok(())
}
/// The 7x7 OBM_CHECK glyph.
fn check(frame: &mut Framebuffer, x: i32, y: i32, color: [u8; 3]) {
    const BITS: [&str; 7] = [
        "......#", ".....##", "#...###", "##.###.", "#####..", ".###...", "..#....",
    ];
    for (dy, row) in BITS.iter().enumerate() {
        for (dx, bit) in row.bytes().enumerate() {
            if bit == b'#' {
                frame.pixel(
                    x + dx as i32,
                    y + dy as i32,
                    [color[0], color[1], color[2], 255],
                );
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn opened(page: Page, state: State) -> MenuPanel {
        let mut panel = MenuPanel::new(page, (100, 100), state);
        // Rendering places the popup; tests use the same geometry by hand.
        let rows = panel
            .entries
            .iter()
            .map(|e| if e.is_some() { ITEM } else { SEPARATOR })
            .sum::<i32>();
        panel.bounds = (100, 100, 300, 100 + 2 * FRAME + rows);
        panel
    }
    fn row(index: usize) -> i32 {
        // Context page: four items, a separator, three items, a separator.
        let separators = [4, 8].iter().filter(|&&s| s < index).count() as i32;
        100 + FRAME + (index as i32 - separators) * ITEM + separators * SEPARATOR + ITEM / 2
    }
    #[test]
    fn context_menu_follows_retail_order_and_skips_separators() {
        let all = State {
            save: true,
            load: true,
            restart: true,
            ..State::default()
        };
        let mut panel = opened(Page::Context, all);
        assert_eq!(panel.key(KeyCode::Enter), Action::None);
        for expected in [
            Action::HideText,
            Action::Skip,
            Action::Auto,
            Action::History,
            Action::Save,
            Action::Load,
            Action::RestartPrompt,
            Action::MenuBar,
            Action::HideText,
        ] {
            panel.key(KeyCode::ArrowDown);
            assert_eq!(panel.key(KeyCode::Enter), expected);
        }
        panel.key(KeyCode::ArrowUp);
        assert_eq!(panel.key(KeyCode::Enter), Action::MenuBar);
        assert_eq!(panel.key(KeyCode::KeyK), Action::Skip);
        assert_eq!(panel.key(KeyCode::KeyM), Action::MenuBar);
        let mut bar = opened(Page::MenuBar, all);
        assert_eq!(bar.key(KeyCode::KeyX), Action::QuitPrompt);
        assert_eq!(bar.key(KeyCode::KeyC), Action::Settings);
        for page in [Page::Quit, Page::Restart] {
            let mut confirm = MenuPanel::new(page, (0, 0), all);
            assert_ne!(confirm.key(KeyCode::Enter), Action::Resume);
            assert_eq!(confirm.key(KeyCode::KeyN), Action::Resume);
        }
    }
    #[test]
    fn greyed_items_do_not_activate() {
        let mut panel = opened(Page::Context, State::default());
        assert_eq!(panel.key(KeyCode::KeyS), Action::None);
        assert_eq!(panel.key(KeyCode::KeyL), Action::None);
        assert_eq!(panel.key(KeyCode::KeyR), Action::None);
        panel.button(150, row(5), true);
        assert_eq!(panel.button(150, row(5), false), Action::None);
        assert_eq!(panel.key(KeyCode::KeyH), Action::HideText);
    }
    #[test]
    fn unpaired_cancelled_or_dragged_releases_do_not_activate() {
        let mut panel = opened(Page::Context, State::default());
        assert_eq!(panel.button(150, row(0), false), Action::None);
        panel.button(150, row(0), true);
        assert_eq!(panel.button(150, row(1), false), Action::None);
        panel.button(150, row(0), true);
        panel.cancel_press();
        assert_eq!(panel.button(150, row(0), false), Action::None);
        panel.button(150, row(0), true);
        assert_eq!(panel.button(150, row(0), false), Action::HideText);
        panel.pointer(150, row(3));
        assert_eq!(panel.key(KeyCode::Enter), Action::History);
        panel.pointer(150, row(4) - ITEM / 2 + SEPARATOR / 2);
        assert_eq!(panel.key(KeyCode::Enter), Action::None);
        panel.button(10, 10, true);
        assert_eq!(panel.button(10, 10, false), Action::Resume);
        assert_eq!(panel.key(KeyCode::Escape), Action::Resume);
    }
}
