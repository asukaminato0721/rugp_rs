//! ユーザーデータ管理ウインドウ after rvmm.dll dialog 5002 セーブデータ選択 and
//! its owner-drawn CUucFileList: a thumbnail, game name, time, scene text and
//! comment per file. Comments are entered as in GMfc.dll dialog 3000.
use super::{
    egui_window::EguiWindow,
    save_store::{self, Entry, Kind},
};
use anyhow::Result;
use std::{collections::HashMap, time::Instant};
use winit::{event::WindowEvent, event_loop::ActiveEventLoop, window::WindowId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Save,
    Load,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Action {
    #[default]
    None,
    Close,
    Save {
        slot: u32,
        comment: String,
    },
    Load {
        slot: u32,
        kind: Kind,
    },
    Comment {
        slot: u32,
        comment: String,
    },
    Important {
        slot: u32,
        important: bool,
    },
    Delete(u32),
}
impl Action {
    fn merge(&mut self, other: Self) {
        if other != Action::None {
            *self = other;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Modal {
    Message(String),
    Overwrite(u32),
    Delete(u32),
    /// コメントを入力してください。 before saving, or for コメント編集.
    Comment {
        slot: u32,
        text: String,
        save: bool,
    },
}

pub(super) struct SaveWindow {
    host: EguiWindow,
    browser: Browser,
}
impl SaveWindow {
    pub fn open(
        event_loop: &dyn ActiveEventLoop,
        face: (&[u8], u32),
        browser: Browser,
    ) -> Result<Self> {
        let host = EguiWindow::open(event_loop, browser.title(), (600.0, 460.0), face)?;
        Ok(Self { host, browser })
    }
    pub fn id(&self) -> WindowId {
        self.host.id()
    }
    pub fn focus(&self) {
        self.host.focus();
    }
    pub fn mode(&self) -> Mode {
        self.browser.mode
    }
    pub fn wake(&self, now: Instant) {
        self.host.wake(now);
    }
    pub fn browser(&mut self) -> &mut Browser {
        self.host.request_redraw();
        &mut self.browser
    }
    pub fn event(&mut self, event: &WindowEvent) -> Action {
        if self.host.event(event) {
            Action::Close
        } else {
            Action::None
        }
    }
    pub fn redraw(&mut self) -> Result<Action> {
        let mut action = Action::None;
        let browser = &mut self.browser;
        self.host.redraw(|ui| action.merge(browser.ui(ui)))?;
        Ok(action)
    }
}

pub(super) struct Browser {
    pub mode: Mode,
    entries: Vec<Entry>,
    selected: Option<u32>,
    /// システム: execute a list item by double-clicking it.
    double_click: bool,
    default_comment: String,
    modal: Option<Modal>,
    textures: HashMap<u32, egui::TextureHandle>,
    scroll_to_selection: bool,
}
impl Browser {
    pub fn new(
        mode: Mode,
        entries: Vec<Entry>,
        selected: u32,
        double_click: bool,
        default_comment: String,
    ) -> Self {
        let mut browser = Self {
            mode,
            entries: Vec::new(),
            selected: None,
            double_click,
            default_comment,
            modal: None,
            textures: HashMap::new(),
            scroll_to_selection: true,
        };
        browser.set_entries(entries);
        browser.selected = browser
            .rows()
            .into_iter()
            .find(|&slot| slot == selected)
            .or_else(|| browser.rows().first().copied());
        browser
    }
    fn title(&self) -> &'static str {
        match self.mode {
            Mode::Save => "セーブするデータを選んでください",
            Mode::Load => "ロードするデータを選んでください",
        }
    }
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        self.entries = entries;
        self.textures.clear();
        if self
            .selected
            .is_some_and(|slot| !self.rows().contains(&slot))
        {
            self.selected = self.rows().first().copied();
        }
    }
    pub fn show_message(&mut self, message: impl Into<String>) {
        self.modal = Some(Modal::Message(message.into()));
    }
    fn entry(&self, slot: u32) -> Option<&Entry> {
        self.entries.iter().find(|e| e.slot == slot)
    }
    /// Loading lists only files; saving adds 新規ファイル rows up to the next ten.
    fn rows(&self) -> Vec<u32> {
        match self.mode {
            Mode::Load => self.entries.iter().map(|e| e.slot).collect(),
            Mode::Save => (1..=save_store::save_rows(&self.entries)).collect(),
        }
    }
    /// セーブ / ロード, also a double click and Enter.
    fn execute(&mut self) -> Action {
        let Some(slot) = self.selected else {
            return Action::None;
        };
        let entry = self.entry(slot);
        match self.mode {
            Mode::Load => match entry {
                Some(entry) if !entry.broken => Action::Load {
                    slot,
                    kind: entry.kind,
                },
                Some(_) => self.refuse("ヘッダ情報の読み込みに失敗しました。"),
                None => Action::None,
            },
            Mode::Save => {
                match entry {
                    Some(entry) if entry.important => self.refuse(
                        "重要度高のデータには上書きできません。\n上書きが必要な場合、重要度を下げてください。",
                    ),
                    Some(_) => {
                        self.modal = Some(Modal::Overwrite(slot));
                        Action::None
                    }
                    None => self.ask_comment(slot, true),
                }
            }
        }
    }
    fn refuse(&mut self, message: &str) -> Action {
        self.show_message(message);
        Action::None
    }
    fn ask_comment(&mut self, slot: u32, save: bool) -> Action {
        let text = if save {
            self.default_comment.clone()
        } else {
            self.entry(slot)
                .map(|e| e.comment.clone())
                .unwrap_or_default()
        };
        self.modal = Some(Modal::Comment { slot, text, save });
        Action::None
    }
    fn edit_comment(&mut self) -> Action {
        match self.selected.and_then(|slot| self.entry(slot)) {
            Some(entry) if entry.important => self.refuse(
                "重要度高のデータは変更できません。\n変更する場合、いったん重要度を下げてください。",
            ),
            Some(entry) if entry.kind == Kind::Port && !entry.broken => {
                self.ask_comment(entry.slot, false)
            }
            _ => Action::None,
        }
    }
    fn toggle_importance(&mut self) -> Action {
        match self.selected.and_then(|slot| self.entry(slot)) {
            Some(entry) if entry.kind == Kind::Port => Action::Important {
                slot: entry.slot,
                important: !entry.important,
            },
            _ => Action::None,
        }
    }
    fn delete(&mut self) -> Action {
        match self.selected.and_then(|slot| self.entry(slot)) {
            Some(entry) if entry.important => self.refuse(
                "重要度高のデータは削除できません。\n削除したい場合、重要度を下げてください。",
            ),
            Some(entry) if entry.kind == Kind::Port => {
                self.modal = Some(Modal::Delete(entry.slot));
                Action::None
            }
            _ => Action::None,
        }
    }
    fn step(&mut self, delta: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let at = self
            .selected
            .and_then(|slot| rows.iter().position(|&r| r == slot))
            .map_or(0, |i| {
                (i as isize + delta).clamp(0, rows.len() as isize - 1) as usize
            });
        self.selected = Some(rows[at]);
        self.scroll_to_selection = true;
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::None;
        let ctx = ui.ctx().clone();
        if let Some(modal) = self.modal.clone() {
            action.merge(self.modal_ui(&ctx, modal));
        } else {
            action.merge(self.keys(&ctx));
        }
        egui::Panel::top("buttons").show(ui, |ui| {
            ui.add_space(4.0);
            action.merge(self.buttons(ui));
            ui.add_space(4.0);
        });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::WHITE))
            .show(ui, |ui| action.merge(self.list(ui)));
        action
    }
    fn keys(&mut self, ctx: &egui::Context) -> Action {
        use egui::Key;
        let pressed = |key| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
        let alt = |key| ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, key));
        if pressed(Key::Escape) {
            return Action::Close;
        }
        if pressed(Key::ArrowDown) {
            self.step(1);
        }
        if pressed(Key::ArrowUp) {
            self.step(-1);
        }
        if pressed(Key::PageDown) {
            self.step(5);
        }
        if pressed(Key::PageUp) {
            self.step(-5);
        }
        if pressed(Key::Enter) {
            return self.execute();
        }
        if alt(Key::C) {
            return self.edit_comment();
        }
        if alt(Key::P) {
            return self.toggle_importance();
        }
        if alt(Key::D) || pressed(Key::Delete) {
            return self.delete();
        }
        Action::None
    }
    fn buttons(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::None;
        let entry = self.selected.and_then(|slot| self.entry(slot)).cloned();
        let port = entry.as_ref().is_some_and(|e| e.kind == Kind::Port);
        ui.horizontal_top(|ui| {
            let primary = match self.mode {
                Mode::Save => "セーブ",
                Mode::Load => "ロード",
            };
            let enabled = match self.mode {
                Mode::Save => self.selected.is_some(),
                Mode::Load => entry.is_some(),
            };
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::new(primary).min_size(egui::vec2(110.0, 52.0)),
                )
                .clicked()
            {
                action.merge(self.execute());
            }
            let small = egui::vec2(117.0, 23.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                let editable = port && entry.as_ref().is_some_and(|e| !e.broken);
                if ui
                    .add_enabled(
                        editable,
                        egui::Button::new("コメント編集(C)").min_size(small),
                    )
                    .clicked()
                {
                    action.merge(self.edit_comment());
                }
                let label = if entry.as_ref().is_some_and(|e| e.important) {
                    "重要度を戻す(P)"
                } else {
                    "重要度を上げる(P)"
                };
                if ui
                    .add_enabled(port, egui::Button::new(label).min_size(small))
                    .clicked()
                {
                    action.merge(self.toggle_importance());
                }
            });
            if ui
                .add_enabled(
                    port,
                    egui::Button::new("セーブファイル削除(D)").min_size(small),
                )
                .clicked()
            {
                action.merge(self.delete());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                ui.add_space(6.0);
                if ui
                    .add(egui::Button::new("キャンセル").min_size(egui::vec2(73.0, 24.0)))
                    .clicked()
                {
                    action = Action::Close;
                }
            });
        });
        action
    }
    fn list(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::None;
        let rows = self.rows();
        if rows.is_empty() {
            ui.add_space(8.0);
            ui.label("ファイルがありません");
            return action;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.add_space(4.0);
                for slot in rows {
                    let response = self.row(ui, slot);
                    if self.scroll_to_selection && self.selected == Some(slot) {
                        response.scroll_to_me(None);
                    }
                    if response.clicked() || response.double_clicked() {
                        self.selected = Some(slot);
                    }
                    if response.double_clicked() && self.double_click {
                        action.merge(self.execute());
                    }
                }
            });
        self.scroll_to_selection = false;
        action
    }
    fn row(&mut self, ui: &mut egui::Ui, slot: u32) -> egui::Response {
        const HEIGHT: f32 = 58.0;
        let width = ui.available_width() - 12.0;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::click());
        let painter = ui.painter_at(rect.expand(4.0));
        let selected = self.selected == Some(slot);
        let card = egui::Rect::from_min_max(rect.min + egui::vec2(22.0, 0.0), rect.max);
        if selected {
            painter.rect_filled(
                card.translate(egui::vec2(4.0, 4.0)),
                0.0,
                egui::Color32::from_rgb(48, 0, 64),
            );
        }
        painter.rect_filled(card, 0.0, egui::Color32::from_rgb(204, 238, 255));
        if selected {
            painter.rect_stroke(
                card,
                0.0,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(49, 106, 197)),
                egui::StrokeKind::Inside,
            );
        }
        let text = egui::Color32::BLACK;
        let font = egui::FontId::proportional(14.0);
        painter.text(
            rect.min + egui::vec2(2.0, 2.0),
            egui::Align2::LEFT_TOP,
            slot.to_string(),
            font.clone(),
            text,
        );
        let thumb =
            egui::Rect::from_min_size(card.min + egui::vec2(6.0, 5.0), egui::vec2(64.0, 48.0));
        painter.rect_filled(thumb, 0.0, egui::Color32::WHITE);
        let entry = self.entry(slot).cloned();
        if let Some(texture) = entry.as_ref().and_then(|e| self.texture(ui.ctx(), e)) {
            painter.image(
                texture.id(),
                thumb,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        let x = thumb.max.x + 8.0;
        let line = |row: usize, value: String| {
            painter.text(
                egui::pos2(x, card.min.y + 3.0 + row as f32 * 17.0),
                egui::Align2::LEFT_TOP,
                value,
                font.clone(),
                text,
            );
        };
        match entry {
            None => line(0, "新規ファイル".into()),
            Some(entry) if entry.broken => line(0, "ヘッダ情報の読み込みに失敗しました。".into()),
            Some(entry) => {
                let mut first = format!(
                    "{}    {}",
                    save_store::game_name(&entry.application),
                    entry
                        .saved_at
                        .map(save_store::format_time)
                        .unwrap_or_default()
                );
                if entry.kind == Kind::Original {
                    first.push_str("  ［原版］");
                }
                if entry.important {
                    first.push_str("  ［重要度：高］");
                }
                line(0, first);
                line(1, entry.navigation.clone());
                line(2, entry.comment.lines().next().unwrap_or_default().into());
            }
        }
        response
    }
    fn texture(&mut self, ctx: &egui::Context, entry: &Entry) -> Option<egui::TextureHandle> {
        let thumbnail = entry.thumbnail.as_ref()?;
        Some(
            self.textures
                .entry(entry.slot)
                .or_insert_with(|| {
                    ctx.load_texture(
                        format!("slot{}", entry.slot),
                        egui::ColorImage::from_rgba_unmultiplied(
                            [usize::from(thumbnail.width), usize::from(thumbnail.height)],
                            &thumbnail.rgba,
                        ),
                        egui::TextureOptions::LINEAR,
                    )
                })
                .clone(),
        )
    }
    fn modal_ui(&mut self, ctx: &egui::Context, modal: Modal) -> Action {
        let mut action = Action::None;
        let mut close = false;
        let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        egui::Modal::new(egui::Id::unique("save-modal")).show(ctx, |ui| {
            ui.set_max_width(360.0);
            let size = egui::vec2(75.0, 23.0);
            match &modal {
                Modal::Message(message) => {
                    ui.label(message);
                    ui.add_space(8.0);
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.add(egui::Button::new("OK").min_size(size)).clicked() || enter || escape {
                        close = true;
                    }
                }
                Modal::Overwrite(slot) | Modal::Delete(slot) => {
                    ui.label(if matches!(modal, Modal::Overwrite(_)) {
                        format!("ファイル{slot} は既に存在します。上書きしますか？")
                    } else {
                        format!("ファイル {slot:04} を削除します。本当によろしいですか？")
                    });
                    ui.add_space(8.0);
                    let (yes, no) = ui
                        .horizontal(|ui| {
                            (
                                ui.add(egui::Button::new("はい(Y)").min_size(size))
                                    .clicked()
                                    || ui.input(|i| i.key_pressed(egui::Key::Y)),
                                ui.add(egui::Button::new("いいえ(N)").min_size(size))
                                    .clicked()
                                    || ui.input(|i| i.key_pressed(egui::Key::N)),
                            )
                        })
                        .inner;
                    if yes {
                        close = true;
                        action = match modal {
                            Modal::Overwrite(slot) => {
                                // The comment prompt replaces this modal.
                                close = false;
                                self.ask_comment(slot, true)
                            }
                            _ => Action::Delete(*slot),
                        };
                    } else if no || escape {
                        close = true;
                    }
                }
                Modal::Comment { slot, text, save } => {
                    ui.label("コメントを入力してください。");
                    let mut edited = text.clone();
                    let response = ui.add(
                        egui::TextEdit::multiline(&mut edited)
                            .desired_rows(4)
                            .desired_width(340.0),
                    );
                    if !response.has_focus() && !response.lost_focus() {
                        response.request_focus();
                    }
                    ui.add_space(6.0);
                    let (ok, cancel) = ui
                        .horizontal(|ui| {
                            (
                                ui.add(egui::Button::new("OK").min_size(size)).clicked(),
                                ui.add(egui::Button::new("キャンセル").min_size(size))
                                    .clicked(),
                            )
                        })
                        .inner;
                    if ok {
                        close = true;
                        action = if *save {
                            Action::Save {
                                slot: *slot,
                                comment: edited.clone(),
                            }
                        } else {
                            Action::Comment {
                                slot: *slot,
                                comment: edited.clone(),
                            }
                        };
                    } else if cancel || escape {
                        close = true;
                    } else if let Some(Modal::Comment { text, .. }) = &mut self.modal {
                        *text = edited;
                    }
                }
            }
        });
        if close {
            self.modal = None;
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(slot: u32, kind: Kind, important: bool) -> Entry {
        Entry {
            slot,
            kind,
            application: "ゲーム".into(),
            saved_at: Some(0),
            navigation: String::new(),
            comment: "前のコメント".into(),
            thumbnail: None,
            important,
            broken: false,
        }
    }
    #[test]
    fn saving_asks_before_overwriting_and_refuses_important_files() {
        let entries = vec![entry(1, Kind::Port, false), entry(2, Kind::Port, true)];
        let mut browser = Browser::new(Mode::Save, entries.clone(), 3, false, "台詞".into());
        assert_eq!(browser.rows(), (1..=10).collect::<Vec<_>>());
        assert_eq!(browser.execute(), Action::None);
        assert_eq!(
            browser.modal,
            Some(Modal::Comment {
                slot: 3,
                text: "台詞".into(),
                save: true
            })
        );
        browser.selected = Some(1);
        browser.execute();
        assert_eq!(browser.modal, Some(Modal::Overwrite(1)));
        browser.selected = Some(2);
        browser.execute();
        assert!(matches!(browser.modal, Some(Modal::Message(_))));
        assert_eq!(
            browser.toggle_importance(),
            Action::Important {
                slot: 2,
                important: false
            }
        );
        browser.modal = None;
        browser.delete();
        assert!(matches!(browser.modal, Some(Modal::Message(_))));
    }
    #[test]
    fn loading_lists_files_only_and_originals_are_read_only() {
        let entries = vec![
            entry(4, Kind::Original, false),
            entry(12, Kind::Port, false),
        ];
        let mut browser = Browser::new(Mode::Load, entries, 1, false, String::new());
        assert_eq!(browser.rows(), vec![4, 12]);
        assert_eq!(browser.selected, Some(4));
        assert_eq!(
            browser.execute(),
            Action::Load {
                slot: 4,
                kind: Kind::Original
            }
        );
        assert_eq!(browser.toggle_importance(), Action::None);
        assert_eq!(browser.delete(), Action::None);
        assert_eq!(browser.edit_comment(), Action::None);
        browser.step(1);
        assert_eq!(browser.edit_comment(), Action::None);
        assert!(matches!(
            browser.modal,
            Some(Modal::Comment {
                slot: 12,
                save: false,
                ..
            })
        ));
        let mut empty = Browser::new(Mode::Load, Vec::new(), 1, false, String::new());
        assert_eq!(empty.execute(), Action::None);
    }
}
