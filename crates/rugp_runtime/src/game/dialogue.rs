use super::*;
use crate::{
    dialogue::Dialogue,
    font::TextFont,
    message_window::{MessageConfig, MessageWindow, PageStyle, SerifColors, TextSettings},
};

pub(super) struct DialogueWait {
    pub(super) target: u64,
    pub(super) stream: Dialogue,
    pub(super) voice: Option<ObjectRef>,
    pub(super) forced_clock: bool,
}
fn output_speed(value: i32, default: u32) -> Result<u32> {
    ensure!(
        i16::try_from(value).is_ok(),
        "dialogue speed exceeds native signed word"
    );
    ensure!(default <= 3000, "invalid default dialogue speed");
    // Vm60 10015b34 uses the configured speed for nonnegative values and
    // the global speed for -1..-9. At 10015d78, <= -10 forces its magnitude,
    // clears the reveal/skip input mode and clamps the delay to 3000 ms.
    Ok(if value <= -10 {
        (-value).min(3000) as u32
    } else if value < 0 {
        default
    } else {
        value.min(3000) as u32
    })
}
fn supported_text_mode(flags: u32) -> bool {
    // The retail even-nibble families share the text/callback packet.
    // Vm60 1006f21c
    // forwards the output modifiers to the window/voice callback; they do not
    // replace explicit waits and pauses parsed from the text itself.
    // Portable presentation currently shares its layout for these variants;
    // native voice callback mixing/style modifiers remain approximate:
    // 10014f89 maps (flags >> 20) & 0xe00 into callback/audio options.
    // Odd-nibble families are not present in the audited game.
    flags & 0x10000000 == 0
}
impl Game {
    /// An outline font is supplied by the user/system, never extracted from
    /// the retail Windows-generated 5RF cache. Loaded lazily at first dialogue.
    pub fn set_font_file(&mut self, path: &Path, index: u32) -> Result<()> {
        ensure!(
            self.font.is_none(),
            "font cannot change after dialogue starts"
        );
        self.font_file = Some((path.to_owned(), index));
        Ok(())
    }
    pub fn waiting_for_dialogue(&self) -> bool {
        self.dialogue.is_some()
    }
    pub fn dialogue_waits_for_input(&self) -> bool {
        self.dialogue.as_ref().is_some_and(|d| d.stream.waiting())
    }
    /// Stable identity of the page now waiting for confirmation, for
    /// read-text tracking (読んだ文章は飛ばす).
    pub fn dialogue_page_key(&self) -> Option<u64> {
        let dialogue = self.dialogue.as_ref().filter(|d| d.stream.waiting())?;
        let cursor = self.vm.cursor()?;
        // FNV-1a over fixed-width fields.
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for word in [
            cursor.scene as u64,
            cursor.address as u64,
            dialogue.stream.remaining() as u64,
        ] {
            for byte in word.to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
            }
        }
        Some(hash)
    }
    /// Displayed parts only. Message control codes and unrevealed text stay
    /// inside the dialogue stream.
    pub fn dialogue_page(&self) -> Option<&[crate::dialogue::TextPart]> {
        self.dialogue.as_ref().map(|d| d.stream.page.as_slice())
    }
    pub fn dialogue_voice_playing(&self) -> bool {
        self.dialogue
            .as_ref()
            .and_then(|d| d.voice.as_ref())
            .and_then(|voice| voice.offset)
            .is_some_and(|source| !self.audio_state.source_tickets(source).is_empty())
    }
    /// Compose the scene with its message windows hidden. This changes no
    /// script visibility flags, textures, clocks or pending input.
    pub fn dialogue_background(&self) -> Result<Option<Framebuffer>> {
        if self.dialogue.is_none() {
            return Ok(None);
        }
        let mut scene = self.objects.clone();
        for id in self.message_windows.keys() {
            if let Some(object) = scene.get_mut(id) {
                object.visible = false;
            }
        }
        Self::render_objects(&scene, self.size()).map(Some)
    }
    pub fn set_text_speed(&mut self, speed_ms: u32) -> Result<()> {
        ensure!(speed_ms <= 3000, "invalid message output speed");
        self.ensure_text_font()?;
        self.message_config.as_mut().unwrap().speed_ms = speed_ms;
        if let Some(dialogue) = &mut self.dialogue
            && !dialogue.forced_clock
        {
            dialogue.stream.set_speed(speed_ms)?;
        }
        Ok(())
    }
    /// The title's speaker colours (CStdb CTxtStDlg_arraySerifSetting).
    pub(super) fn serif_colors(&mut self) -> &SerifColors {
        if self.serif_colors.is_none() {
            let table = self
                .archive
                .nodes
                .iter()
                .filter(|n| n.class.name == "CStdb")
                .filter_map(|n| self.archive.read_node(n.id).ok())
                .filter_map(|data| rugp_assets::profile::Profile::parse_object(&data).ok())
                .map(|profile| {
                    profile
                        .read_string("CTxtStDlg_arraySerifSetting", "")
                        .to_owned()
                })
                .find(|text| !text.is_empty())
                .unwrap_or_default();
            self.serif_colors = Some(SerifColors::parse(&table));
        }
        self.serif_colors.as_ref().unwrap()
    }
    /// Colour a page by its speaker; mark it while it waits for input.
    pub(super) fn page_style(
        &mut self,
        page: &[crate::dialogue::TextPart],
        marker: bool,
    ) -> PageStyle {
        let speaker = page.iter().find_map(|part| match part {
            crate::dialogue::TextPart::Speaker(name) => Some(name.clone()),
            _ => None,
        });
        PageStyle {
            color: self.serif_colors().color(speaker.as_deref()),
            marker,
        }
    }
    pub(super) fn ensure_message_window(&mut self, reference: &ObjectRef) -> Result<u64> {
        let id = reference.offset.context("message box has no identity")?;
        if !self.message_windows.contains_key(&id) {
            let window = MessageWindow::load(&self.archive, &mut self.images, reference)?;
            let [x, y, w, h] = window.data.rectangles[2];
            ensure!(w > 0 && h > 0, "invalid stored message rectangle");
            ensure!(
                !self.objects.contains_key(&id),
                "message window was instantiated without resource defaults"
            );
            let mut object = ObjectState {
                rect: (i32::from(x), i32::from(y), w as u32, h as u32),
                ..ObjectState::default()
            };
            if window.telop() {
                object
                    .properties
                    .insert("render.text.telop".into(), Value::Integer(1));
            }
            self.objects.insert(id, object);
            self.message_windows.insert(id, window);
        }
        Ok(id)
    }
    fn game_root(&self) -> Result<&Path> {
        self.archive.volumes[0]
            .path
            .parent()
            .context("game directory missing")
    }
    /// Setup.ini's face, size and decoration for message text.
    pub fn native_text_settings(&self) -> Result<TextSettings> {
        let root = self.game_root()?;
        let mut settings = TextSettings::load(root)?;
        settings.size = MessageConfig::load(root)?.font_size;
        Ok(settings)
    }
    fn load_text_font(&self, config: &MessageConfig, settings: &TextSettings) -> Result<TextFont> {
        let mut font = match (&settings.family, &self.font_file) {
            (None, Some((path, index))) => TextFont::from_file(path, *index, settings.size)?,
            (family, _) => TextFont::system_weighted(
                family.as_deref().unwrap_or(&config.family),
                settings.size,
                settings.face_bold,
            )?,
        };
        font.set_style(settings.style);
        Ok(font)
    }
    /// Change the message face/size/decoration, redrawing visible text.
    pub fn set_text_settings(&mut self, settings: TextSettings) -> Result<()> {
        ensure!(
            (4..=128).contains(&settings.size) && settings.style.outline <= 2,
            "invalid text settings"
        );
        if self.text_settings.as_ref() == Some(&settings) {
            return Ok(());
        }
        self.text_settings = Some(settings.clone());
        let Some(mut config) = self.message_config.clone() else {
            // Applied when the first message needs a font.
            return Ok(());
        };
        let font = self.load_text_font(&config, &settings)?;
        config.font_size = settings.size;
        if let Some(family) = &settings.family {
            config.family = family.clone();
        }
        self.font = Some(font);
        self.message_config = Some(config);
        self.redraw_text()
    }
    /// Recompose retained pages, a pending page and text choices.
    fn redraw_text(&mut self) -> Result<()> {
        let pending = self.dialogue.as_ref().map(|d| d.target);
        let choices = self.selection_text_target();
        let retained: Vec<u64> = self
            .message_windows
            .iter()
            .filter(|(id, w)| {
                !w.page.is_empty()
                    && Some(**id) != pending
                    && Some(**id) != choices
                    && self
                        .objects
                        .get(id)
                        .is_some_and(|o| o.layers.contains_key(&0))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in retained {
            let rect = self.objects[&id].rect;
            let page = self.message_windows[&id].page.clone();
            let style = self.page_style(&page, false);
            let window = &self.message_windows[&id];
            let texture =
                window.compose(self.font.as_mut().unwrap(), rect.2, rect.3, &page, style)?;
            self.objects
                .get_mut(&id)
                .unwrap()
                .layers
                .insert(0, Layer::Screen(Arc::new(texture)));
        }
        if let Some(target) = choices {
            self.refresh_text_menu(target)?;
        }
        if pending.is_some() {
            self.refresh_dialogue()?;
        }
        self.present_scene()
    }
    pub(super) fn ensure_text_font(&mut self) -> Result<()> {
        if self.font.is_none() {
            let root = self.game_root()?;
            let mut config = self
                .message_config
                .clone()
                .map_or_else(|| MessageConfig::load(root), Ok)?;
            let settings = match &self.text_settings {
                Some(settings) => settings.clone(),
                None => self.native_text_settings()?,
            };
            let font = self.load_text_font(&config, &settings)?;
            config.font_size = settings.size;
            if let Some(family) = &settings.family {
                config.family = family.clone();
            }
            eprintln!(
                "Dialogue font: {}; portable outline layout (native GDI/speaker styling and pagination are incomplete).",
                font.source
            );
            self.font = Some(font);
            self.message_config = Some(config);
        }
        Ok(())
    }
    pub(super) fn begin_dialogue(&mut self, instruction: &Instruction) -> Result<()> {
        ensure!(
            self.dialogue.is_none() && self.selection.is_none(),
            "overlapping dialogue/menu waits"
        );
        ensure!(
            supported_text_mode(instruction.flags),
            "unsupported CVmMsg mode flags {:#x}",
            instruction.flags & !0xfffff
        );
        let Value::Reference(target) = self.vm.evaluate(&instruction.arguments[0])? else {
            bail!("dialogue target is not a reference");
        };
        let Value::Text(text) = self.vm.evaluate(&instruction.arguments[1])? else {
            bail!("dialogue body is not text");
        };
        let voice = match self.vm.evaluate(&instruction.arguments[2])? {
            Value::Null => None,
            Value::Reference(reference)
                if reference
                    .class
                    .as_ref()
                    .is_some_and(|c| c.name == "COptimizedObs") =>
            {
                Some(reference)
            }
            _ => bail!("unsupported dialogue resource callback"),
        };
        let id = self.ensure_message_window(&target)?;
        self.ensure_text_font()?;
        let object = &self.objects[&id];
        let layout = number(
            &object.properties,
            "レイアウト値",
            self.message_windows[&id].data.layout as i32,
        )?;
        ensure!(
            [0x311, 0x711].contains(&layout),
            "unsupported dialogue layout {layout:#x}"
        );
        let configured_speed = number(&object.properties, "速度", -1)?;
        let speed = output_speed(
            configured_speed,
            self.message_config.as_ref().unwrap().speed_ms,
        )?;
        let stream = Dialogue::new(&text, speed, self.message_windows[&id].page.clone())?;
        if let Some(source) = &voice {
            self.queue_sound(SoundRequest {
                ticket: None,
                source: Some(source.clone()),
                kind: 2,
                flags: 0,
                stereo: Default::default(),
                parameters: [0; 3],
            })?;
        }
        let object = self.objects.get_mut(&id).unwrap();
        object.properties.insert(
            "render.text.flags".into(),
            Value::Integer((instruction.flags & !0xfffff) as i32),
        );
        // CVmMsg execution (Vm60 0x1006ecb5) clears CBox's hidden bit before
        // output. Chapter startup deliberately hides this box until then.
        object.visible = true;
        object.backdrop = Some(self.message_config.as_ref().unwrap().backdrop);
        self.dialogue = Some(DialogueWait {
            target: id,
            stream,
            voice,
            forced_clock: configured_speed <= -10,
        });
        self.dialogue_pressed = false;
        self.tick_dialogue(0)
    }
    fn refresh_dialogue(&mut self) -> Result<()> {
        let d = self.dialogue.as_ref().context("no pending dialogue")?;
        let (target, page, waiting) = (d.target, d.stream.page.clone(), d.stream.waiting());
        let style = self.page_style(&page, waiting);
        let rect = self.objects[&target].rect;
        let texture = Arc::new(self.message_windows[&target].compose(
            self.font.as_mut().unwrap(),
            rect.2,
            rect.3,
            &page,
            style,
        )?);
        let d = self.dialogue.as_ref().context("no pending dialogue")?;
        let object = self.objects.get_mut(&d.target).unwrap();
        object.layers.insert(0, Layer::Screen(texture));
        self.present_scene()
    }
    fn finish_dialogue_if_done(&mut self) -> Result<()> {
        if self.dialogue.as_ref().is_some_and(|d| d.stream.finished()) {
            self.vm.acknowledge_host()?;
            let d = self.dialogue.take().unwrap();
            if let Some(source) = d.voice {
                self.queue_sound(SoundRequest {
                    ticket: None,
                    source: Some(source),
                    kind: 3,
                    flags: 0,
                    stereo: Default::default(),
                    parameters: [0; 3],
                })?;
            }
            self.message_windows.get_mut(&d.target).unwrap().page = d.stream.page;
        }
        Ok(())
    }
    pub(super) fn tick_dialogue(&mut self, dt_ms: u32) -> Result<()> {
        if self
            .dialogue
            .as_mut()
            .context("no pending dialogue")?
            .stream
            .tick(dt_ms)?
        {
            self.refresh_dialogue()?;
        }
        self.finish_dialogue_if_done()
    }
    pub(super) fn advance_dialogue(&mut self) -> Result<()> {
        if self
            .dialogue
            .as_ref()
            .is_some_and(|d| d.forced_clock && !d.stream.waiting())
        {
            return Ok(());
        }
        if self
            .dialogue
            .as_mut()
            .context("no pending dialogue")?
            .stream
            .activate()?
        {
            self.refresh_dialogue()?;
        }
        self.finish_dialogue_if_done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_output_speeds_keep_forced_delays_default_modes_and_native_clamp() -> Result<()> {
        for (value, expected) in [
            (0, 0),
            (30, 30),
            (-1, 17),
            (-9, 17),
            (-10, 10),
            (-100, 100),
            (-32768, 3000),
            (32767, 3000),
        ] {
            assert_eq!(output_speed(value, 17)?, expected);
        }
        assert!(output_speed(i32::MIN, 17).is_err());
        assert!(output_speed(32768, 17).is_err());
        assert!(output_speed(-1, 3001).is_err());
        Ok(())
    }
    #[test]
    fn output_modifiers_do_not_reject_the_standard_text_packet_family() {
        for flags in [
            0, 0x02000000, 0x04000000, 0x20000000, 0x22000131, 0x40000000, 0x60000000, 0x60100000,
            0x61100000, 0x620001a6, 0x64200000, 0x66000000, 0x80000000, 0x80000038, 0xa0000000,
            0xc0000000, 0xe0000000,
        ] {
            assert!(supported_text_mode(flags));
        }
        for flags in [0x10000000, 0x30000000, 0x50000000, 0x90000000, 0xf0000000] {
            assert!(!supported_text_mode(flags));
        }
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_all_dialogue_bodies_primary_targets_and_voice_callbacks_parse() -> Result<()> {
        use rugp_scene_vm::Argument;
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let archive = RioArchive::open(root)?;
        let native = NativeSchema::from_directory(archive.volumes[0].path.parent().unwrap())?;
        let program = Program::load(&archive, &native)?;
        let mut count = 0;
        for scene in program.scenes.values() {
            for instruction in scene
                .instructions
                .iter()
                .filter(|i| i.class.name == "CVmMsg")
            {
                let location = || {
                    format!(
                        "dialogue format at {}:{:#x}",
                        scene.object_id, instruction.address
                    )
                };
                ensure!(
                    supported_text_mode(instruction.flags),
                    "unsupported {}",
                    location()
                );
                let [
                    Argument::Reference(target),
                    Argument::Text(text),
                    Argument::Reference(callback),
                    ..,
                ] = instruction.arguments.as_slice()
                else {
                    bail!("unsupported operands in {}", location());
                };
                ensure!(
                    target
                        .class
                        .as_ref()
                        .is_some_and(|c| c.name == "CMsgSelBox"),
                    "unsupported target in {}",
                    location()
                );
                ensure!(
                    callback
                        .class
                        .as_ref()
                        .is_some_and(|c| c.name == "COptimizedObs")
                        || (callback.cache_index == Some(0) && callback.offset.is_none()),
                    "unsupported callback in {}",
                    location()
                );
                // Check actual syntax and logical progress without logging or
                // embedding original text. This is not VM/window/route execution
                // or a claim that every dialogue layout has been rendered.
                let mut stream = Dialogue::new(text, 30, vec![]).with_context(location)?;
                stream.tick(17).with_context(location)?;
                let snapshot = stream.snapshot().with_context(location)?;
                let mut restored = snapshot.restore().with_context(location)?;
                ensure!(
                    snapshot == restored.snapshot()?,
                    "dialogue snapshot state changed in {}",
                    location()
                );
                ensure!(
                    stream.tick(u32::MAX)? == restored.tick(u32::MAX)?,
                    "dialogue snapshot advancement changed in {}",
                    location()
                );
                ensure!(
                    stream.snapshot()? == restored.snapshot()?,
                    "dialogue snapshot consumed controls differently in {}",
                    location()
                );
                count += 1;
            }
        }
        assert_eq!(count, 100923);
        Ok(())
    }

    #[test]
    #[ignore = "set RUGP_TEST_GAME; renders all retail dialogue pages with a Japanese outline font"]
    fn retail_all_isolated_dialogue_pages_fit_the_native_default_windows() -> Result<()> {
        use rugp_scene_vm::Argument;
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let archive = Arc::new(RioArchive::open(root)?);
        let root = archive.volumes[0].path.parent().context("game directory")?;
        let native = NativeSchema::from_directory(root)?;
        let program = Program::load(&archive, &native)?;
        let config = MessageConfig::load(root)?;
        let mut font = if let Some(path) = std::env::var_os("RUGP_TEST_FONT") {
            TextFont::from_file(path.as_ref(), 0, config.font_size)?
        } else {
            TextFont::system(&config.family, config.font_size)?
        };
        let mut images = ImageManager::new(Arc::clone(&archive));
        let mut windows = BTreeMap::new();
        let mut bodies = 0usize;
        let mut pages = 0usize;
        for scene in program.scenes.values() {
            for instruction in scene
                .instructions
                .iter()
                .filter(|i| i.class.name == "CVmMsg")
            {
                let location = || {
                    format!(
                        "isolated dialogue layout at {}:{:#x}",
                        scene.object_id, instruction.address
                    )
                };
                let [Argument::Reference(target), Argument::Text(text), ..] =
                    instruction.arguments.as_slice()
                else {
                    bail!("unsupported operands in {}", location());
                };
                let id = target.offset.context("message window identity")?;
                if let std::collections::btree_map::Entry::Vacant(entry) = windows.entry(id) {
                    entry.insert(MessageWindow::load(&archive, &mut images, target)?);
                }
                let window = &windows[&id];
                let [_, _, width, height] = window.data.rectangles[2];
                ensure!(width > 0 && height > 0, "invalid native message extent");
                // Independently audit each body with the stored window defaults.
                // This does not execute VM branches, retain pages across distinct
                // message records, or infer script-time window/layout overrides.
                // Splitting explicit clear controls also checks text revealed
                // before an in-record clear, rather than discarding it in one
                // large clock tick. Other controls use the real Dialogue parser.
                for segment in text.split('\u{5}') {
                    let mut stream =
                        Dialogue::new(segment, config.speed_ms, vec![]).with_context(location)?;
                    let mut steps = 0;
                    while !stream.finished() {
                        ensure!(steps <= segment.len(), "dialogue audit made no progress");
                        steps += 1;
                        stream.tick(u32::MAX).with_context(location)?;
                        if !stream.page.is_empty() {
                            let texture = window
                                .compose(
                                    &mut font,
                                    width as u32,
                                    height as u32,
                                    &stream.page,
                                    PageStyle::default(),
                                )
                                .with_context(location)?;
                            ensure!(
                                texture.width == width as u32
                                    && texture.height == height as u32
                                    && texture.rgba.len() == width as usize * height as usize * 4,
                                "invalid composed page in {}",
                                location()
                            );
                            pages += 1;
                        }
                        if stream.waiting() {
                            stream.activate().with_context(location)?;
                        }
                    }
                }
                bodies += 1;
                if bodies.is_multiple_of(10000) {
                    eprintln!("dialogue layout audit: bodies={bodies}, pages={pages}");
                }
            }
        }
        ensure!(bodies == 100923, "incomplete retail dialogue inventory");
        // Empty records and standalone clears legitimately have no glyph page.
        ensure!(
            pages == 100748 && windows.len() == 1,
            "incomplete retail page inventory"
        );
        eprintln!(
            "dialogue layout audit finished: bodies={bodies}, pages={pages}, windows={}, font={}",
            windows.len(),
            font.source
        );
        Ok(())
    }

    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_two_family_packet_keeps_text_input_wait_and_one_voice_callback() -> Result<()> {
        check_retail_packet(467, 0x7588, 0x22000000)
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_eight_family_packet_keeps_text_input_wait_and_one_voice_callback() -> Result<()> {
        check_retail_packet(469, 0x19c8, 0x80000000)
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_remaining_even_families_keep_explicit_waits_and_optional_voice() -> Result<()> {
        for (scene, address, modifiers) in [
            (201, 0x2ced4, 0x02000000),
            (201, 0xdcb0, 0x40000000),
            (201, 0xe230, 0xa0000000),
            (302, 0xe50, 0xc0000000),
            (201, 0xe19c, 0xe0000000),
        ] {
            check_retail_packet(scene, address, modifiers)?;
        }
        Ok(())
    }
    fn check_retail_packet(scene_id: usize, address: u32, modifiers: u32) -> Result<()> {
        use rugp_assets::script::Script;
        use rugp_scene_vm::{Argument, Scenario};
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let scene = Scenario::decode_with_schema(
            &Script::load(&game.archive, scene_id)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let mut message = scene
            .instructions
            .iter()
            .find(|i| i.address == address)
            .context("retail text packet missing")?
            .clone();
        assert_eq!(message.flags & !0xfffff, modifiers);
        let has_voice =
            matches!(&message.arguments[2], Argument::Reference(r) if r.class.is_some());
        let Argument::Reference(window) = &message.arguments[0] else {
            bail!("dialogue window missing");
        };
        game.message(Message {
            class: "OM_SetMsgOutputSpeed".into(),
            target: Value::Reference(window.clone()),
            fields: BTreeMap::from([("速度".into(), Value::Integer(30))]),
        })?;
        message.address = 0;
        message.next_address = 48;
        message.arguments[1] = Argument::Text("probe\u{3}line\u{1}".into());
        game.vm = message_then_return(message)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert!(!game.dialogue_waits_for_input());
        super::ui_snapshot::test_roundtrip(&mut game)?;
        assert_eq!(
            game.pending_sounds()
                .iter()
                .filter(|s| s.ticket.is_some())
                .count(),
            usize::from(has_voice)
        );
        assert_eq!(game.step(10000)?, RunState::Waiting);
        assert!(game.dialogue_waits_for_input());
        let pixels = game.frame().to_vec();
        let cursor = game.vm.cursor().cloned();
        let page = game.dialogue_page().unwrap().to_vec();
        let background = game
            .dialogue_background()?
            .context("hidden dialogue frame")?;
        assert_ne!(background.rgba, pixels);
        assert_eq!(game.frame(), pixels);
        assert_eq!(game.vm.cursor(), cursor.as_ref());
        assert_eq!(game.dialogue_page().unwrap(), page);
        assert_eq!(game.dialogue_voice_playing(), has_voice);
        game.pointer_move(0, 0)?;
        game.pointer_button(true)?;
        super::ui_snapshot::test_roundtrip(&mut game)?;
        game.pointer_button(false)?;
        assert!(game.dialogue_waits_for_input());
        assert_eq!(game.vm.cursor().unwrap().address, 0);
        assert_eq!(
            game.pending_sounds()
                .iter()
                .filter(|s| s.ticket.is_some())
                .count(),
            usize::from(has_voice)
        );
        assert!(
            game.frame()
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[..3] != [0; 3])
        );
        game.activate_selection()?;
        assert!(!game.waiting_for_dialogue());
        assert_eq!(
            game.pending_sounds()
                .iter()
                .any(|s| s.kind == 3 && s.source.is_some()),
            has_voice
        );
        assert_eq!(game.step(0)?, RunState::Ended);
        Ok(())
    }
    fn message_then_return(message: Instruction) -> Result<Vm> {
        use rugp_assets::object::ClassId;
        use rugp_scene_vm::Scenario;
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 60,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    message,
                    Instruction {
                        index: 1,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmRet".into(),
                            schema: 17,
                        },
                        address: 48,
                        next_address: u32::MAX,
                        flags: 0,
                        arguments: vec![],
                    },
                ],
            },
        )?;
        Vm::new(Arc::new(program), 1)
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_negative_speed_runs_on_clock_without_click_skip_or_invented_input_wait() -> Result<()>
    {
        use rugp_assets::script::Script;
        use rugp_scene_vm::{Argument, Scenario};
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let scene = Scenario::decode_with_schema(
            &Script::load(&game.archive, 469)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let setting = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x9fd8)
            .context("forced speed setting")?;
        let setting = game.vm.message(setting)?;
        assert_eq!(number(&setting.fields, "速度", 0)?, -100);
        game.message(setting)?;
        let mut message = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x9ff4)
            .context("timed text packet")?
            .clone();
        let Argument::Text(text) = &message.arguments[1] else {
            bail!("text missing");
        };
        assert!(!text.contains(['\u{1}', '\u{2}']));
        message.address = 0;
        message.next_address = 48;
        message.arguments[1] = Argument::Text("probe".into());
        game.vm = message_then_return(message)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert!(!game.dialogue_waits_for_input());
        assert_eq!(
            game.pending_sounds()
                .iter()
                .filter(|s| s.ticket.is_some())
                .count(),
            1
        );
        let before = game.frame().to_vec();
        super::ui_snapshot::test_roundtrip(&mut game)?;
        game.activate_selection()?;
        assert_eq!(game.frame(), before);
        assert_eq!(game.step(499)?, RunState::Waiting);
        assert!(!game.dialogue_waits_for_input());
        assert_eq!(game.vm.cursor().unwrap().address, 0);
        assert_eq!(
            game.pending_sounds()
                .iter()
                .filter(|s| s.ticket.is_some())
                .count(),
            1
        );
        assert!(
            game.frame()
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[..3] != [0; 3])
        );
        assert!(!game.pending_sounds().iter().any(|s| s.kind == 3));
        assert_eq!(game.step(1)?, RunState::Ended);
        assert!(!game.waiting_for_dialogue());
        assert!(
            game.pending_sounds()
                .iter()
                .any(|s| s.kind == 3 && s.source.is_some())
        );
        Ok(())
    }
}
