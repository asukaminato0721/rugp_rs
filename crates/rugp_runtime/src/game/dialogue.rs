use super::*;
use crate::{
    dialogue::Dialogue,
    font::TextFont,
    message_window::{MessageConfig, MessageWindow},
};

pub(super) struct DialogueWait {
    target: u64,
    stream: Dialogue,
    voice: Option<ObjectRef>,
    forced_clock: bool,
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
            self.objects.insert(
                id,
                ObjectState {
                    rect: (i32::from(x), i32::from(y), w as u32, h as u32),
                    ..ObjectState::default()
                },
            );
            self.message_windows.insert(id, window);
        }
        Ok(id)
    }
    pub(super) fn ensure_text_font(&mut self) -> Result<()> {
        if self.font.is_none() {
            let root = self.archive.volumes[0]
                .path
                .parent()
                .context("game directory missing")?;
            let config = MessageConfig::load(root)?;
            let font = if let Some((path, index)) = &self.font_file {
                TextFont::from_file(path, *index, config.font_size)?
            } else {
                TextFont::system(&config.family, config.font_size)?
            };
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
        let rect = self.objects[&d.target].rect;
        let texture = Arc::new(self.message_windows[&d.target].compose(
            self.font.as_mut().unwrap(),
            rect.2,
            rect.3,
            &d.stream.page,
        )?);
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
                // Check actual syntax without logging or embedding original text.
                // This is a static packet audit, not execution/layout verification.
                Dialogue::new(text, 0, vec![]).with_context(location)?;
                count += 1;
            }
        }
        assert_eq!(count, 100923);
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
        assert_eq!(
            game.pending_sounds()
                .iter()
                .filter(|s| s.ticket.is_some())
                .count(),
            usize::from(has_voice)
        );
        assert_eq!(game.step(10000)?, RunState::Waiting);
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
