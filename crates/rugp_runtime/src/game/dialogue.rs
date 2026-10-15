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
    // The 0x6 family is the same text/callback packet with output modifiers.
    // Portable presentation currently shares its layout for these variants;
    // controls in the text still determine page/input waits.
    flags & 0xf0000000 == 0x60000000
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
            0x60000000, 0x60100000, 0x61100000, 0x620001a6, 0x64200000, 0x66000000,
        ] {
            assert!(supported_text_mode(flags));
        }
        for flags in [0, 0x20000000, 0x40000000, 0x80000000, 0xe0000000] {
            assert!(!supported_text_mode(flags));
        }
    }
}
