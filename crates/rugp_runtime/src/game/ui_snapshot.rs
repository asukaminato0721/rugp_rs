//! UI progress and script settings, independent of platform input/device state.
//! Restore immutable resources, never rerun CVmMsg/selection instructions or
//! issue voice-play callbacks. Audio playback is coordinated by its own snapshot.
use super::*;
use crate::{
    dialogue::{TextPart, snapshot::DialogueSnapshot},
    font::TextFont,
    menu::{MenuState, snapshot::MenuSnapshot},
    message_window::{MessageConfig, MessageWindow, snapshot::MessageWindowSnapshot},
};
use rugp_assets::object::snapshot::ReferenceSnapshotWriter;
use rugp_scene_vm::vm::snapshot::ValueSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_BYTES: usize = 64 * 1024 * 1024;
type Properties = Vec<(String, ValueSnapshot)>;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiSnapshot {
    version: u32,
    menus: Vec<(u64, MenuSnapshot)>,
    windows: Vec<(u64, MessageWindowSnapshot)>,
    text_menus: Vec<(u64, text_menu::TextMenu)>,
    selection: Option<SavedSelection>,
    dialogue: Option<SavedDialogue>,
    config: Option<MessageConfig>,
    settings: Vec<(String, i32, Properties)>,
    navigation: String,
    script_save_enabled: bool,
    ended: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedSelection {
    target: u64,
    result: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedDialogue {
    target: u64,
    stream: DialogueSnapshot,
    voice: Option<u32>,
    forced_clock: bool,
}
#[derive(Default)]
struct Budget {
    bytes: usize,
    values: usize,
    roots: HashSet<u32>,
}
impl Budget {
    fn bytes(&mut self, bytes: usize) -> Result<()> {
        ensure!(
            bytes <= MAX_BYTES.saturating_sub(self.bytes),
            "UI snapshot payload exceeds limit"
        );
        self.bytes += bytes;
        Ok(())
    }
    fn root(&mut self, root: u32) -> Result<()> {
        ensure!(self.roots.insert(root), "reused UI snapshot reference root");
        Ok(())
    }
    fn page(&mut self, page: &[TextPart]) -> Result<()> {
        crate::dialogue::snapshot::validate_page(page)?;
        for part in page {
            self.bytes(match part {
                TextPart::Speaker(name) => name.len(),
                _ => 4,
            })?;
        }
        Ok(())
    }
    fn properties(&mut self, properties: &Properties) -> Result<()> {
        ensure!(
            properties.len() <= 4096,
            "UI snapshot property count exceeds limit"
        );
        let mut names = HashSet::new();
        for (name, value) in properties {
            ensure!(
                name.len() <= 4096 && names.insert(name),
                "invalid or duplicate UI snapshot property"
            );
            ensure!(
                self.values < 1_000_000,
                "UI snapshot value count exceeds limit"
            );
            self.values += 1;
            self.bytes(name.len())?;
            match value {
                ValueSnapshot::Reference(root) => self.root(*root)?,
                ValueSnapshot::Text(text) => self.bytes(text.len())?,
                ValueSnapshot::Bytes(bytes) => self.bytes(bytes.len())?,
                ValueSnapshot::Real(value) => {
                    ensure!(value.is_finite(), "nonfinite UI snapshot value")
                }
                _ => {}
            }
        }
        Ok(())
    }
}
impl Game {
    pub fn snapshot_ui(&self, references: &mut ReferenceSnapshotWriter) -> Result<UiSnapshot> {
        ensure!(
            self.font.is_some() == self.message_config.is_some(),
            "UI font/config initialization differs"
        );
        let menus = self
            .menus
            .iter()
            .map(|(id, m)| Ok((*id, m.snapshot(references)?)))
            .collect::<Result<_>>()?;
        let windows = self
            .message_windows
            .iter()
            .map(|(id, w)| Ok((*id, w.snapshot(references)?)))
            .collect::<Result<_>>()?;
        let selection = self
            .selection
            .as_ref()
            .map(|s| {
                Ok::<_, anyhow::Error>(SavedSelection {
                    target: s.target,
                    result: references.insert(&s.result)?,
                })
            })
            .transpose()?;
        let dialogue = self
            .dialogue
            .as_ref()
            .map(|d| {
                Ok::<_, anyhow::Error>(SavedDialogue {
                    target: d.target,
                    stream: d.stream.snapshot()?,
                    voice: d.voice.as_ref().map(|v| references.insert(v)).transpose()?,
                    forced_clock: d.forced_clock,
                })
            })
            .transpose()?;
        let settings = self
            .settings
            .iter()
            .map(|((key, kind), properties)| {
                Ok((
                    key.clone(),
                    *kind,
                    properties
                        .iter()
                        .map(|(name, value)| {
                            Ok((name.clone(), ValueSnapshot::capture(value, references)?))
                        })
                        .collect::<Result<_>>()?,
                ))
            })
            .collect::<Result<_>>()?;
        let snapshot = UiSnapshot {
            version: 1,
            menus,
            windows,
            text_menus: self
                .text_menus
                .iter()
                .map(|(id, m)| (*id, m.clone()))
                .collect(),
            selection,
            dialogue,
            config: self.message_config.clone(),
            settings,
            navigation: self.navigation.clone(),
            script_save_enabled: self.script_save_enabled,
            ended: self.ended,
        };
        snapshot.validate()?;
        snapshot.validate_vm(&self.vm)?;
        Ok(snapshot)
    }
}
impl UiSnapshot {
    pub(super) fn ended(&self) -> bool {
        self.ended
    }
    pub(crate) fn wait_count(&self) -> usize {
        usize::from(self.selection.is_some()) + usize::from(self.dialogue.is_some())
    }
    fn validate_vm(&self, vm: &Vm) -> Result<()> {
        if self.wait_count() == 0 {
            return Ok(());
        }
        let pending = vm
            .pending_instruction()
            .context("UI snapshot wait has no suspended VM instruction")?;
        if self.dialogue.is_some() {
            ensure!(
                pending.class.name == "CVmMsg",
                "dialogue snapshot has a different pending VM instruction"
            );
        } else if self.selection.is_some() {
            ensure!(
                pending.class.name == "CVmGenericMsg",
                "selection snapshot has a different pending VM instruction"
            );
        }
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported UI snapshot version");
        ensure!(self.wait_count() <= 1, "overlapping UI snapshot waits");
        ensure!(
            self.menus.len() <= 4096
                && self.windows.len() <= 4096
                && self.text_menus.len() <= 4096
                && self.settings.len() <= 4096,
            "UI snapshot table count exceeds limit"
        );
        let mut budget = Budget::default();
        budget.bytes(self.navigation.len())?;
        let mut menu_ids = HashSet::new();
        for (id, menu) in &self.menus {
            ensure!(menu_ids.insert(*id), "duplicate UI snapshot menu");
            budget.root(menu.source_root())?;
        }
        let mut window_ids = HashSet::new();
        for (id, window) in &self.windows {
            ensure!(
                window_ids.insert(*id),
                "duplicate UI snapshot message window"
            );
            budget.root(window.source_root())?;
            budget.page(window.page())?;
        }
        let mut text_ids = HashSet::new();
        for (id, menu) in &self.text_menus {
            ensure!(
                text_ids.insert(*id) && window_ids.contains(id),
                "invalid UI snapshot text-menu owner"
            );
            ensure!(
                !menu.entries.is_empty()
                    && menu.entries.len() <= 256
                    && (menu.hits.is_empty() || menu.hits.len() == menu.entries.len()),
                "invalid UI snapshot text-menu entry/hit count"
            );
            ensure!(
                menu.hover.is_none_or(|i| i < menu.entries.len())
                    && menu.pressed.is_none_or(|i| i < menu.entries.len()),
                "invalid UI snapshot text-menu hover/press"
            );
            for text in &menu.entries {
                ensure!(
                    !text.is_empty()
                        && text.len() <= 16 * 1024
                        && text.chars().all(|ch| !ch.is_control() || ch == '\n'),
                    "invalid UI snapshot selector text"
                );
                budget.bytes(text.len())?;
            }
            for (_, _, w, h) in &menu.hits {
                ensure!(
                    *w > 0 && *h > 0 && *w <= 65535 && *h <= 65535,
                    "invalid UI snapshot text-menu hit rectangle"
                );
            }
        }
        if let Some(selection) = &self.selection {
            budget.root(selection.result)?;
        }
        if let Some(dialogue) = &self.dialogue {
            ensure!(
                window_ids.contains(&dialogue.target),
                "UI snapshot dialogue window is missing"
            );
            let stream = dialogue.stream.restore()?;
            budget.page(&stream.page)?;
            if let Some(root) = dialogue.voice {
                budget.root(root)?;
            }
        }
        if let Some(config) = &self.config {
            ensure!(
                config.family.len() <= 4096
                    && (4..=128).contains(&config.font_size)
                    && config.speed_ms <= 3000
                    && config.backdrop.source <= 256
                    && config.backdrop.destination_shift <= 8,
                "invalid UI snapshot message configuration"
            );
            budget.bytes(config.family.len())?;
        }
        ensure!(
            (self.dialogue.is_none()
                && self
                    .selection
                    .as_ref()
                    .is_none_or(|s| !text_ids.contains(&s.target)))
                || self.config.is_some(),
            "UI snapshot text output has no font configuration"
        );
        let mut settings = HashSet::new();
        for (key, kind, properties) in &self.settings {
            ensure!(
                key.len() <= 4096 && settings.insert((key, *kind)),
                "invalid or duplicate UI snapshot setting"
            );
            budget.bytes(key.len())?;
            budget.properties(properties)?;
        }
        Ok(())
    }
    pub fn restore(
        &self,
        game: &Game,
        references: &[ObjectRef],
        scene: &scene_snapshot::RestoredScene,
        vm: &Vm,
    ) -> Result<RestoredUi> {
        self.validate()?;
        self.validate_vm(vm)?;
        ensure!(
            self.wait_count() + scene.wait_count() <= 1,
            "overlapping scene/UI snapshot waits"
        );
        let mut mask_bytes = 0usize;
        let menus = self
            .menus
            .iter()
            .map(|(id, saved)| {
                let menu = saved.restore(&game.archive, references)?;
                ensure!(
                    menu.source_offset() == Some(*id),
                    "UI snapshot menu source identity differs"
                );
                ensure!(
                    menu.mask_bytes() <= 64 * 1024 * 1024 - mask_bytes,
                    "UI snapshot hit-map budget exceeded"
                );
                mask_bytes += menu.mask_bytes();
                Ok((*id, menu))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut images = ImageManager::new(Arc::clone(&game.archive));
        let mut piece_bytes = 0usize;
        let windows = self
            .windows
            .iter()
            .map(|(id, saved)| {
                let window = saved.restore(&game.archive, &mut images, references)?;
                ensure!(
                    window.source_offset() == Some(*id) && scene.object(*id).is_some(),
                    "UI snapshot message-window identity/target differs"
                );
                ensure!(
                    window.piece_bytes() <= 128 * 1024 * 1024 - piece_bytes,
                    "UI snapshot frame-piece budget exceeded"
                );
                piece_bytes += window.piece_bytes();
                Ok((*id, window))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        for object in scene.objects().values() {
            if let Some(id) = object.menu {
                ensure!(
                    menus.contains_key(&id),
                    "UI snapshot image-menu state is missing"
                );
            }
        }
        for (id, _) in &self.text_menus {
            ensure!(
                scene.object(*id).is_some(),
                "UI snapshot text-menu window is missing"
            );
        }
        let font = self
            .config
            .as_ref()
            .map(|config| {
                if let Some(font) = &game.font {
                    font.resized(config.font_size)
                } else if let Some((path, index)) = &game.font_file {
                    TextFont::from_file(path, *index, config.font_size)
                } else {
                    TextFont::system(&config.family, config.font_size)
                }
            })
            .transpose()?;
        let selection = self
            .selection
            .as_ref()
            .map(|s| {
                let object = scene
                    .object(s.target)
                    .context("UI snapshot selection window is missing")?;
                if let Some((_, menu)) = self.text_menus.iter().find(|(id, _)| *id == s.target) {
                    ensure!(
                        menu.hits.len() == menu.entries.len(),
                        "waiting UI text menu has no complete hit rectangles"
                    );
                } else {
                    ensure!(
                        object.menu.is_some_and(|id| menus.contains_key(&id)),
                        "UI snapshot selection image menu is missing"
                    );
                }
                let result = references
                    .get(s.result as usize)
                    .context("UI snapshot selection output root out of range")?;
                ensure!(
                    result.class.as_ref().is_some_and(|c| c.name == "CInt"),
                    "unsupported UI snapshot selection output type"
                );
                Ok::<_, anyhow::Error>(menu::SelectionWait {
                    target: s.target,
                    result: result.clone(),
                })
            })
            .transpose()?;
        let dialogue = self
            .dialogue
            .as_ref()
            .map(|d| {
                ensure!(
                    scene.object(d.target).is_some(),
                    "UI snapshot dialogue target is missing"
                );
                let voice = d
                    .voice
                    .map(|root| {
                        let voice = references
                            .get(root as usize)
                            .context("UI snapshot voice root out of range")?;
                        ensure!(
                            voice
                                .class
                                .as_ref()
                                .is_some_and(|c| c.name == "COptimizedObs")
                                && voice.offset.is_some()
                                && voice.size.is_some()
                                && voice.inline.is_none(),
                            "unsupported UI snapshot voice callback"
                        );
                        Ok::<_, anyhow::Error>(voice.clone())
                    })
                    .transpose()?;
                Ok::<_, anyhow::Error>(dialogue::DialogueWait {
                    target: d.target,
                    stream: d.stream.restore()?,
                    voice,
                    forced_clock: d.forced_clock,
                })
            })
            .transpose()?;
        let settings = self
            .settings
            .iter()
            .map(|(key, kind, properties)| {
                Ok((
                    (key.clone(), *kind),
                    properties
                        .iter()
                        .map(|(name, value)| Ok((name.clone(), value.restore(references)?)))
                        .collect::<Result<_>>()?,
                ))
            })
            .collect::<Result<_>>()?;
        Ok(RestoredUi {
            size: scene.size(),
            menus,
            windows,
            text_menus: self
                .text_menus
                .iter()
                .map(|(id, m)| (*id, m.clone()))
                .collect(),
            selection,
            dialogue,
            font,
            config: self.config.clone(),
            settings,
            navigation: self.navigation.clone(),
            script_save_enabled: self.script_save_enabled,
            ended: self.ended,
        })
    }
}
pub struct RestoredUi {
    size: (u32, u32),
    menus: BTreeMap<u64, MenuState>,
    windows: BTreeMap<u64, MessageWindow>,
    text_menus: BTreeMap<u64, text_menu::TextMenu>,
    selection: Option<menu::SelectionWait>,
    dialogue: Option<dialogue::DialogueWait>,
    font: Option<TextFont>,
    config: Option<MessageConfig>,
    settings: BTreeMap<(String, i32), BTreeMap<String, Value>>,
    navigation: String,
    script_save_enabled: bool,
    ended: bool,
}
impl RestoredUi {
    pub fn apply(self, game: &mut Game) -> Result<()> {
        ensure!(
            game.size() == self.size,
            "UI restore target size changed during preparation"
        );
        self.commit(game);
        Ok(())
    }
    pub(super) fn commit(self, game: &mut Game) {
        game.menus = self.menus;
        game.message_windows = self.windows;
        game.text_menus = self.text_menus;
        game.selection = self.selection;
        game.dialogue = self.dialogue;
        game.font = self.font;
        game.message_config = self.config;
        game.settings = self.settings;
        game.navigation = self.navigation;
        game.script_save_enabled = self.script_save_enabled;
        game.ended = self.ended;
        // Saved pressed images remain visually intact until an actual input
        // event, but neither a pre-load press nor the load button's release may
        // activate the resumed game. Live cursor placement is host-owned.
        game.pointer_inside = false;
        game.dialogue_pressed = false;
        game.pointer_armed = false;
    }
}

#[cfg(test)]
pub(super) fn test_roundtrip(game: &mut Game) -> Result<()> {
    use crate::render::snapshot::{TextureSnapshotTable, TextureSnapshotWriter};
    use rugp_assets::object::snapshot::ReferenceSnapshot;
    use rugp_scene_vm::vm::snapshot::VmSnapshot;
    let frame = game.frame().to_vec();
    let cursor = game.vm.cursor().cloned();
    let sound_count = game.pending_sounds().len();
    let mut references = ReferenceSnapshotWriter::default();
    let mut textures = TextureSnapshotWriter::default();
    let vm = game.vm.snapshot(&mut references)?;
    let scene = game.snapshot_scene(&mut references, &mut textures)?;
    let actors = game.snapshot_actors(&mut references)?;
    let ui = game.snapshot_ui(&mut references)?;
    let wire = serde_json::to_vec(&(
        vm,
        scene,
        actors,
        ui,
        references.finish()?,
        textures.finish()?,
    ))?;
    let (vm, scene, actors, ui, references, textures): (
        VmSnapshot,
        scene_snapshot::SceneSnapshot,
        actor_snapshot::ActorSnapshot,
        UiSnapshot,
        ReferenceSnapshot,
        TextureSnapshotTable,
    ) = serde_json::from_slice(&wire)?;
    let (vm, references) = game.vm.restore_snapshot(&vm, &references)?;
    let textures = textures.restore()?;
    let scene = scene.restore(game, &references, &textures)?;
    ensure!(
        actors.wait_count()
            + scene.wait_count()
            + ui.wait_count()
            + usize::from(game.audio_wait.is_some())
            <= 1,
        "overlapping coordinated UI test waits"
    );
    let actors = actors.restore(game, &references, &scene, &vm)?;
    let ui = ui.restore(game, &references, &scene, &vm)?;
    scene.apply(game)?;
    actors.apply(game)?;
    ui.apply(game)?;
    game.vm = vm;
    ensure!(
        game.frame() == frame,
        "UI restore changed frozen scene pixels"
    );
    ensure!(
        game.vm.cursor() == cursor.as_ref(),
        "UI restore acknowledged an instruction"
    );
    ensure!(
        game.pending_sounds().len() == sound_count,
        "UI restore replayed a voice callback"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bare() -> UiSnapshot {
        UiSnapshot {
            version: 1,
            menus: vec![],
            windows: vec![],
            text_menus: vec![],
            selection: None,
            dialogue: None,
            config: None,
            settings: vec![],
            navigation: String::new(),
            script_save_enabled: true,
            ended: false,
        }
    }
    #[test]
    fn script_flags_navigation_and_variant_settings_roundtrip_without_profile_rollback()
    -> Result<()> {
        let mut saved = bare();
        saved.script_save_enabled = false;
        saved.navigation = "synthetic chapter".into();
        saved.ended = true;
        saved.settings = vec![(
            "display".into(),
            2,
            vec![
                ("a".into(), ValueSnapshot::Integer(-3)),
                ("b".into(), ValueSnapshot::Real(0.25)),
                ("c".into(), ValueSnapshot::Text("test".into())),
                ("d".into(), ValueSnapshot::Bytes(vec![1, 2, 3])),
                ("e".into(), ValueSnapshot::Null),
            ],
        )];
        saved.validate()?;
        let restored: UiSnapshot = serde_json::from_slice(&serde_json::to_vec(&saved)?)?;
        restored.validate()?;
        assert!(!restored.script_save_enabled && restored.ended);
        assert_eq!(restored.navigation, saved.navigation);
        assert!(
            serde_json::to_value(&restored.settings)? == serde_json::to_value(&saved.settings)?
        );
        Ok(())
    }
    #[test]
    fn invalid_ui_versions_tables_variants_configurations_and_text_hit_state_fail() -> Result<()> {
        let mut bad = bare();
        bad.version = 9;
        assert!(bad.validate().is_err());
        let mut bad = bare();
        bad.config = Some(MessageConfig {
            family: "test".into(),
            font_size: 0,
            speed_ms: 0,
            backdrop: crate::message_window::Backdrop {
                color: [0; 3],
                source: 256,
                destination_shift: 8,
            },
        });
        assert!(bad.validate().is_err());
        let mut bad = bare();
        bad.settings = vec![(
            "a".into(),
            0,
            vec![("v".into(), ValueSnapshot::Real(f64::NAN))],
        )];
        assert!(bad.validate().is_err());
        let mut bad = bare();
        bad.settings = vec![("a".into(), 0, vec![]), ("a".into(), 0, vec![])];
        assert!(bad.validate().is_err());
        let mut bad = bare();
        bad.settings = vec![(
            "a".into(),
            0,
            vec![
                ("v".into(), ValueSnapshot::Reference(0)),
                ("other".into(), ValueSnapshot::Reference(0)),
            ],
        )];
        assert!(bad.validate().is_err());
        let window = || {
            serde_json::from_value::<MessageWindowSnapshot>(
                serde_json::json!({"version":1,"source":1,"page":[]}),
            )
        };
        let mut bad = bare();
        bad.windows = vec![(1, window()?)];
        bad.text_menus = vec![(
            1,
            text_menu::TextMenu {
                entries: vec!["A".into()],
                hover: Some(1),
                pressed: None,
                hits: vec![],
            },
        )];
        assert!(bad.validate().is_err());
        let mut bad = bare();
        bad.windows = vec![(1, window()?)];
        bad.text_menus = vec![(
            1,
            text_menu::TextMenu {
                entries: vec!["A\u{1}".into()],
                hover: None,
                pressed: None,
                hits: vec![],
            },
        )];
        assert!(bad.validate().is_err());
        let mut budget = Budget {
            bytes: MAX_BYTES,
            ..Default::default()
        };
        assert!(budget.bytes(1).is_err());
        Ok(())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_ui_keeps_retained_and_streaming_pages_separate() -> Result<()> {
        use rugp_assets::script::Script;
        use rugp_scene_vm::{Argument, Scenario};
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let scene = Scenario::decode_with_schema(
            &Script::load(&game.archive, 467)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let mut first = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x7588)
            .context("retail text packet missing")?
            .clone();
        first.address = 0;
        first.next_address = 48;
        first.arguments[1] = Argument::Text("A\u{2}".into());
        let mut second = first.clone();
        second.index = 1;
        second.address = 48;
        second.next_address = 96;
        second.arguments[1] = Argument::Text("B\u{1}".into());
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 128,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    first,
                    second,
                    Instruction {
                        index: 2,
                        stream_offset: 0,
                        class: rugp_assets::object::ClassId {
                            name: "CVmRet".into(),
                            schema: 17,
                        },
                        address: 96,
                        next_address: u32::MAX,
                        flags: 0,
                        arguments: vec![],
                    },
                ],
            },
        )?;
        game.vm = Vm::new(Arc::new(program), 1)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        game.step(10000)?;
        assert!(game.dialogue_waits_for_input());
        game.activate_selection()?;
        let target = *game
            .message_windows
            .keys()
            .next()
            .context("window missing")?;
        assert_eq!(
            game.message_windows[&target].page,
            vec![TextPart::Glyph('A')]
        );
        test_roundtrip(&mut game)?;
        game.step(0)?;
        game.step(10000)?;
        assert!(game.dialogue_waits_for_input());
        assert_eq!(
            game.message_windows[&target].page,
            vec![TextPart::Glyph('A')]
        );
        assert_eq!(
            game.dialogue.as_ref().unwrap().stream.page,
            vec![TextPart::Glyph('A'), TextPart::Glyph('B')]
        );
        game.pointer_move(0, 0)?;
        game.pointer_button(true)?;
        test_roundtrip(&mut game)?;
        game.pointer_button(false)?;
        assert!(game.dialogue_waits_for_input());
        assert_eq!(game.vm.cursor().unwrap().address, 48);
        game.activate_selection()?;
        assert!(game.message_windows[&target].page.is_empty());
        assert_eq!(game.step(0)?, RunState::Ended);
        Ok(())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_ui_title_preserves_selector_state_but_old_release_cannot_choose_a_result()
    -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let mut audio = None;
        for _ in 0..200 {
            game.step(10000)?;
            crate::audio::mute_requests(&mut game, &mut audio)?;
            if game.waiting_for_selection() {
                break;
            }
        }
        ensure!(game.waiting_for_selection(), "title not reached");
        game.navigate_selection(1)?;
        let wait = game.selection.as_ref().unwrap();
        let result = wait.result.clone();
        let target = wait.target;
        let id = game.objects[&target].menu.context("title menu missing")?;
        let key = game.menus[&id].hover.context("title item not selected")?;
        let expected = game.menus[&id].result(key).unwrap() as i32;
        game.vm.write_object(&result, Value::Integer(-99))?;
        game.pointer_button(true)?;
        assert_eq!(game.menus[&id].pressed, Some(key));
        let cursor = game.vm.cursor().cloned();
        test_roundtrip(&mut game)?;
        assert_eq!(game.menus[&id].hover, Some(key));
        assert_eq!(game.menus[&id].pressed, Some(key));
        game.pointer_button(false)?;
        assert!(game.waiting_for_selection());
        assert_eq!(game.vm.cursor(), cursor.as_ref());
        assert_eq!(
            game.vm
                .evaluate(&rugp_scene_vm::Argument::Reference(result.clone()))?
                .integer()?,
            -99
        );
        assert_eq!(game.menus[&id].pressed, None);
        game.activate_selection()?;
        assert!(!game.waiting_for_selection());
        assert_eq!(
            game.vm
                .evaluate(&rugp_scene_vm::Argument::Reference(result))?
                .integer()?,
            expected
        );
        Ok(())
    }
}
