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
}
fn supported_text_mode(flags: u32) -> bool {
    // The 0x2/0x6 families share the text/callback packet. Vm60 1006f21c
    // forwards the output modifiers to the window/voice callback; they do not
    // replace explicit waits and pauses parsed from the text itself.
    // Portable presentation currently shares its layout for these variants;
    // native voice callback mixing/style modifiers remain approximate.
    matches!(flags & 0xf0000000, 0x20000000 | 0x60000000)
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
        let speed = number(&object.properties, "速度", -1)?;
        ensure!(
            (-1..=3000).contains(&speed),
            "unsupported dialogue speed {speed}"
        );
        let speed = if speed < 0 {
            self.message_config.as_ref().unwrap().speed_ms
        } else {
            speed as u32
        };
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
    fn output_modifiers_do_not_reject_the_standard_text_packet_family() {
        for flags in [
            0x20000000, 0x22000131, 0x60000000, 0x60100000, 0x61100000, 0x620001a6, 0x64200000,
            0x66000000,
        ] {
            assert!(supported_text_mode(flags));
        }
        for flags in [
            0, 0x10000000, 0x30000000, 0x40000000, 0x80000000, 0xe0000000,
        ] {
            assert!(!supported_text_mode(flags));
        }
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_two_family_packet_keeps_text_input_wait_and_one_voice_callback() -> Result<()> {
        use rugp_assets::{object::ClassId, script::Script};
        use rugp_scene_vm::{Argument, Scenario};
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let scene = Scenario::decode_with_schema(
            &Script::load(&game.archive, 467)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let mut message = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x7588)
            .context("retail 0x2 text packet missing")?
            .clone();
        assert_eq!(message.flags & !0xfffff, 0x22000000);
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
        game.vm = Vm::new(Arc::new(program), 1)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert!(!game.dialogue_waits_for_input());
        assert_eq!(
            game.pending_sounds()
                .iter()
                .filter(|s| s.ticket.is_some())
                .count(),
            1
        );
        assert_eq!(game.step(10000)?, RunState::Waiting);
        assert!(game.dialogue_waits_for_input());
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
        game.activate_selection()?;
        assert!(!game.waiting_for_dialogue());
        assert!(
            game.pending_sounds()
                .iter()
                .any(|s| s.kind == 3 && s.source.is_some())
        );
        assert_eq!(game.step(0)?, RunState::Ended);
        Ok(())
    }
}
