use super::*;
use crate::staffroll::StaffRoll;
use rugp_assets::layout_text::{LayoutText, TextRollDefaults};

// Private overlay slot; never replace the window's base image planes/camera.
const ROLL_LAYER: i32 = i32::MAX;

impl Game {
    pub(super) fn play_staffroll(
        &mut self,
        target: &ObjectRef,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        ensure!(
            target
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CTextStaffrollSeq" && c.schema == 2),
            "unsupported staff-roll sequence"
        );
        let id = target.offset.context("staff-roll identity missing")?;
        let size = target.size.context("staff-roll size missing")?;
        ensure!(size <= 1024 * 1024, "oversized staff-roll defaults");
        let defaults = TextRollDefaults::parse(
            &self.archive.read_at(id, size as usize)?,
            2,
            self.archive.address_shift,
        )?;
        ensure!(
            defaults.sources.iter().all(|r| r.class.is_none())
                && defaults.rectangle == [0; 4]
                && defaults.parameters == [0; 7],
            "non-default staff-roll resource settings are not implemented"
        );
        ensure!(
            self.staffroll.as_ref().is_none_or(|(_, p)| p.finished()),
            "concurrent staff rolls are not implemented"
        );
        ensure!(
            self.video.as_ref().is_none_or(|(_, p)| p.finished()),
            "concurrent movie/staff-roll playback is not implemented"
        );
        ensure!(
            number(fields, "同期属性", 0)? == 0x8000
                && number(fields, "拡張スタイル", 0)? == 0
                && number(fields, "ループ回数", 0)? == 0,
            "unsupported staff-roll synchronization/style/looping"
        );
        for key in ["開始座標加算値", "終了範囲加算値", "ユーザー値"] {
            ensure!(
                number(fields, key, 0)? == 0,
                "unsupported staff-roll offset/user value"
            );
        }
        let Some(Value::Reference(window)) = fields.get("ウインドウ") else {
            bail!("staff-roll window missing");
        };
        ensure!(
            window
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CParseBgBox"),
            "unsupported staff-roll window type"
        );
        let window = window
            .offset
            .context("staff-roll window identity missing")?;
        let Some(Value::Reference(source)) = fields.get("ロール画像") else {
            bail!("staff-roll text source missing");
        };
        let class = source
            .class
            .as_ref()
            .context("staff-roll source class missing")?;
        ensure!(
            class.name == "CLayoutTextImg",
            "unsupported staff-roll source type"
        );
        let size = source.size.context("staff-roll source size missing")?;
        ensure!(size <= 16 * 1024 * 1024, "oversized staff-roll source");
        let layout = LayoutText::parse(
            &self.archive.read_at(
                source.offset.context("staff-roll source extent missing")?,
                size as usize,
            )?,
            class.schema,
            self.archive.address_shift,
        )?;
        let duration = number(fields, "指定映像時間", 0)?;
        let width = number(fields, "指定矩形W", layout.rectangle[2])?;
        let height = number(fields, "指定矩形H", layout.rectangle[3])?;
        ensure!(
            duration > 0 && width > 0 && height > 0,
            "invalid staff-roll duration/viewport"
        );
        self.ensure_text_font()?;
        let rect = self
            .objects
            .get(&window)
            .map_or((0, 0, 1024, 600), |o| o.rect);
        ensure!(
            rect.3 == height as u32,
            "staff-roll viewport height must match its target window"
        );
        let roll = StaffRoll::new(
            window,
            &layout,
            self.font.as_ref().unwrap(),
            (width as u32, height as u32),
            rect.2,
            duration as u32,
        )?;
        let object = self.objects.entry(window).or_default();
        object
            .layers
            .insert(ROLL_LAYER, Layer::Screen(Arc::new(roll.surface()?)));
        self.staffroll = Some((id, roll));
        self.present_scene()
    }
    pub(super) fn synchronize_staffroll(
        &mut self,
        reference: &ObjectRef,
        mode: i32,
        timeout: i32,
    ) -> Result<()> {
        ensure!(
            mode == 16 && timeout == 0,
            "unsupported staff-roll synchronization mode/timeout"
        );
        let (id, roll) = self
            .staffroll
            .as_ref()
            .context("no staff roll for synchronization target")?;
        ensure!(
            reference.offset == Some(*id),
            "staff-roll synchronization target mismatch"
        );
        if roll.finished() {
            self.vm.acknowledge_host()
        } else {
            self.staffroll_wait = true;
            Ok(())
        }
    }
    pub(super) fn advance_staffroll(&mut self, dt_ms: u32) -> Result<()> {
        let Some((_, roll)) = &mut self.staffroll else {
            return Ok(());
        };
        if roll.advance(dt_ms) {
            let object = self
                .objects
                .get_mut(&roll.window)
                .context("staff-roll window disappeared")?;
            if roll.finished() {
                object.layers.remove(&ROLL_LAYER);
            } else {
                object
                    .layers
                    .insert(ROLL_LAYER, Layer::Screen(Arc::new(roll.surface()?)));
            }
            self.present_scene()?;
        }
        if self.staffroll_wait && self.staffroll.as_ref().is_some_and(|(_, p)| p.finished()) {
            self.staffroll_wait = false;
            self.vm.acknowledge_host()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugp_assets::{object::ClassId, script::Script};
    use rugp_scene_vm::{Argument, Scenario};
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_staffroll_renders_all_four_sources_and_sync_keeps_original_planes() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let scene = Scenario::decode_with_schema(
            &Script::load(&game.archive, 238)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let instruction = scene
            .instructions
            .iter()
            .find(|i| i.address == 0xa8)
            .context("credits packet")?;
        let message = game.vm.message(instruction)?;
        let Value::Reference(sequence) = &message.target else {
            bail!("sequence reference missing");
        };
        let sequence = sequence.clone();
        let Some(Value::Reference(window)) = message.fields.get("ウインドウ") else {
            bail!("window reference missing");
        };
        let window = window.offset.unwrap();
        game.ensure_text_font()?;
        for (offset, size, width) in [
            (0x6ad8fe2a, 5848, 472),
            (0x74a408d7, 5267, 1024),
            (0x8b9735b0, 4279, 472),
            (0x943cafe2, 5848, 400),
        ] {
            let layout = LayoutText::parse(
                &game.archive.read_at(offset, size)?,
                1,
                game.archive.address_shift,
            )?;
            let mut roll = StaffRoll::new(
                window,
                &layout,
                game.font.as_ref().unwrap(),
                (width, 600),
                1024,
                1000,
            )?;
            roll.advance(500);
            assert!(
                roll.surface()?
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[3] != 0)
            );
        }
        let mut base = Framebuffer::new(1024, 600)?;
        base.clear([3, 7, 11]);
        let expected = base.rgba.clone();
        let base = Arc::new(Texture {
            width: 1024,
            height: 600,
            display_scale: 1.0,
            rgba: base.rgba,
        });
        game.objects
            .entry(window)
            .or_default()
            .layers
            .insert(0, Layer::Screen(Arc::clone(&base)));
        let camera = Camera {
            position: [12, 34, 56],
            ..Camera::default()
        };
        let pending = crate::world::CameraTween::new(
            camera,
            Camera {
                position: [20, 40, 60],
                ..camera
            },
            0,
        )?;
        let object = game.objects.get_mut(&window).unwrap();
        object.camera = camera;
        object.pending_camera = Some(pending);
        game.message(message)?;
        assert_eq!(game.frame(), expected);
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 32,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    Instruction {
                        index: 0,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmSync".into(),
                            schema: 17,
                        },
                        address: 0,
                        next_address: 24,
                        flags: 0,
                        arguments: vec![
                            Argument::Reference(sequence),
                            Argument::Number(16),
                            Argument::Number(0),
                        ],
                    },
                    Instruction {
                        index: 1,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmRet".into(),
                            schema: 17,
                        },
                        address: 24,
                        next_address: u32::MAX,
                        flags: 0,
                        arguments: vec![],
                    },
                ],
            },
        )?;
        game.vm = Vm::new(Arc::new(program), 1)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert_eq!(game.vm.cursor().unwrap().address, 0);
        assert_eq!(game.step(70650)?, RunState::Waiting);
        assert_ne!(game.frame(), expected);
        assert_eq!(game.step(70649)?, RunState::Waiting);
        assert_eq!(game.step(1)?, RunState::Ended);
        assert_eq!(game.frame(), expected);
        let object = &game.objects[&window];
        assert_eq!(object.camera, camera);
        assert_eq!(object.pending_camera, Some(pending));
        assert!(!object.layers.contains_key(&ROLL_LAYER));
        let Layer::Screen(after) = &object.layers[&0] else {
            bail!("base replaced");
        };
        assert!(Arc::ptr_eq(after, &base));
        assert!(game.pending_sounds().is_empty());
        Ok(())
    }
}
