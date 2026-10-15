use super::*;
use rugp_assets::object::{ClassId, InlineIdentity};

pub(super) struct ImageFade {
    pub(super) handle: ObjectRef,
    pub(super) window: u64,
    pub(super) elapsed: u32,
    pub(super) total: u32,
    pub(super) fade_in: u32,
    pub(super) fade_out: u32,
    pub(super) cancelled: bool,
}
impl ImageFade {
    pub(super) fn identity(&self) -> InlineIdentity {
        self.handle.inline.as_ref().unwrap().identity()
    }
    pub(super) fn finished(&self) -> bool {
        self.cancelled || self.elapsed >= self.total
    }
    fn alpha(&self) -> u8 {
        let elapsed = u64::from(self.elapsed);
        let total = u64::from(self.total);
        if self.elapsed < self.fade_in {
            ((elapsed * 256) / u64::from(self.fade_in)).min(255) as u8
        } else if self.fade_out > 0 && elapsed >= total.saturating_sub(u64::from(self.fade_out)) {
            ((total.saturating_sub(elapsed) * 256) / u64::from(self.fade_out)).min(255) as u8
        } else {
            255
        }
    }
    fn apply(&self, object: &mut ObjectState) {
        if self.finished() {
            // age_classic 10025600, mode bit 1 clear: hide, reset body
            // extent and restore opacity. Keep the image for its next use.
            object.visible = false;
            object.rect.2 = 0;
            object.rect.3 = 0;
            object.opacity = 255;
        } else {
            object.visible = true;
            object.opacity = self.alpha();
        }
    }
}
impl Game {
    pub(super) fn fade_image(
        &mut self,
        target: &ObjectRef,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        ensure!(
            target
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CKiminozoEffectManager" && c.schema == 1),
            "unsupported async image manager"
        );
        ensure!(
            number(fields, "表示方式", 0)? == 0 && number(fields, "モード", 0)? == 1,
            "unsupported async image fade display/mode"
        );
        let Some(Value::Reference(window)) = fields.get("対象VRAMウインドウ") else {
            bail!("async image window missing");
        };
        ensure!(
            window
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CImgBox" && c.schema == 15),
            "unsupported async image window class"
        );
        let window = window
            .offset
            .context("async image window identity missing")?;
        let fade_in = number(fields, "イン速度", 0)?;
        let fade_out = number(fields, "アウト速度", 0)?;
        let total = number(fields, "総時間", 0)?;
        ensure!(
            fade_in >= 0 && fade_out >= 0 && total >= 0,
            "unsupported negative async image fade times"
        );
        let output = match fields.get("非同期ハンドル") {
            None | Some(Value::Null) => None,
            Some(Value::Reference(r))
                if r.class.as_ref().is_some_and(|c| c.name == "CVmVarObj") =>
            {
                Some(r)
            }
            _ => bail!("invalid async image output destination"),
        };
        ensure!(
            self.async_images.len() < 4096,
            "async image handle budget exceeded"
        );
        ensure!(
            !self
                .async_images
                .iter()
                .any(|a| a.window == window && !a.finished()),
            "overlapping fades on one image window are not implemented"
        );
        ensure!(
            self.objects.contains_key(&window),
            "async image window is not initialized"
        );
        // The original exports a CGenericUso event, not the actor or an
        // archive address (age_classic 1002549a..10025503). Keep reference
        // identity through script assignment without inventing RIO offsets.
        let handle = ObjectRef::transient(ClassId {
            name: "CGenericUso".into(),
            schema: 1,
        });
        if let Some(output) = output {
            self.vm
                .write_object(output, Value::Reference(handle.clone()))?;
        }
        let fade = ImageFade {
            handle,
            window,
            elapsed: 0,
            total: total as u32,
            fade_in: fade_in as u32,
            fade_out: fade_out as u32,
            cancelled: false,
        };
        fade.apply(self.objects.get_mut(&window).unwrap());
        self.async_images.push(fade);
        self.present_scene()
    }
    pub(super) fn synchronize_image(
        &mut self,
        reference: &ObjectRef,
        mode: i32,
        timeout: i32,
    ) -> Result<()> {
        ensure!(
            ([0, 16].contains(&mode) && timeout == 0) || [32, 33, 34].contains(&mode),
            "unsupported async image synchronization mode/timeout"
        );
        let identity = reference
            .inline
            .as_ref()
            .context("async image event has no runtime identity")?
            .identity();
        let actor = self
            .async_images
            .iter()
            .position(|a| a.identity() == identity)
            .context("unknown async image event")?;
        let active = !self.async_images[actor].finished();
        if mode & 0x30 == 0x20 {
            // Vm60 10072bb3..10072c70 never reads the timeout word for a
            // query. It samples status first, then 10072f15 sends event 3
            // (mode 32) or 1 (33); low bits 2 (34) leave it alone.
            // CGenericUso forwards both cancellation events to the actor's
            // completion callback, which hides/reset-sizes this mode-1 fade.
            if mode != 34 && active {
                let actor = &mut self.async_images[actor];
                actor.cancelled = true;
                actor.apply(
                    self.objects
                        .get_mut(&actor.window)
                        .context("async image window missing")?,
                );
                self.present_scene()?;
            }
            self.vm.acknowledge_sync_query(active)
        } else if active {
            self.image_wait = Some(identity);
            Ok(())
        } else {
            self.vm.acknowledge_host()
        }
    }
    pub(super) fn advance_async_images(&mut self, dt_ms: u32) -> Result<()> {
        let mut changed = false;
        for actor in self.async_images.iter_mut().filter(|a| !a.finished()) {
            actor.elapsed = actor.elapsed.saturating_add(dt_ms).min(actor.total);
            actor.apply(
                self.objects
                    .get_mut(&actor.window)
                    .context("async image window disappeared")?,
            );
            changed = true;
        }
        if changed {
            self.present_scene()?;
        }
        if let Some(identity) = &self.image_wait {
            let actor = self
                .async_images
                .iter()
                .find(|a| a.identity() == *identity)
                .context("waiting async image event missing")?;
            if actor.finished() {
                self.image_wait = None;
                self.vm.acknowledge_host()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugp_assets::script::Script;
    use rugp_scene_vm::{Argument, Scenario};

    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_async_image_writes_waitable_identity_and_fades_pixels_until_exact_cleanup()
    -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let scene = Scenario::decode_with_schema(
            &Script::load(&game.archive, 149)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let mut instruction = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x19c)
            .context("retail async fade packet")?
            .clone();
        // The original helper substitutes its caller's duration. Keep the
        // real defaults/destination, replacing that override with a test clock.
        instruction.arguments.truncate(3);
        let Argument::Message { arguments, .. } = &mut instruction.arguments[1] else {
            bail!("fade message missing");
        };
        let mut replaced = false;
        for field in arguments {
            if let Argument::Field { name, argument, .. } = field
                && name == "総時間"
            {
                **argument = Argument::Number(5000);
                replaced = true;
            }
        }
        assert!(replaced);
        let message = game.vm.message(&instruction)?;
        let Value::Reference(target) = &message.target else {
            bail!("manager missing")
        };
        let Value::Reference(output) = &message.fields["非同期ハンドル"] else {
            bail!("output missing")
        };
        let Value::Reference(window) = &message.fields["対象VRAMウインドウ"] else {
            bail!("window missing")
        };
        let output = output.clone();
        let id = window.offset.unwrap();
        game.objects.clear();
        game.frame = Framebuffer::new(4, 4)?;
        let blue = Arc::new(Texture {
            width: 1,
            height: 1,
            display_scale: 1.0,
            rgba: vec![0, 0, 255, 255],
        });
        let red = Arc::new(Texture {
            width: 2,
            height: 2,
            display_scale: 1.0,
            rgba: [255, 0, 0, 255].repeat(4),
        });
        game.objects.insert(
            100,
            ObjectState {
                rect: (0, 0, 4, 4),
                layers: BTreeMap::from([(0, Layer::Screen(blue))]),
                ..ObjectState::default()
            },
        );
        game.objects.insert(
            id,
            ObjectState {
                rect: (1, 1, 2, 2),
                layers: BTreeMap::from([(0, Layer::Screen(Arc::clone(&red)))]),
                ..ObjectState::default()
            },
        );
        // Reject unsupported packets before allocating handles or changing pixels.
        for (field, value) in [("モード", 0), ("表示方式", 1), ("イン速度", -1)] {
            let mut invalid = message.fields.clone();
            invalid.insert(field.into(), Value::Integer(value));
            assert!(game.fade_image(target, &invalid).is_err());
            assert!(game.async_images.is_empty());
            assert_eq!(game.objects[&id].opacity, 255);
        }
        instruction.address = 0;
        instruction.next_address = 28;
        let sync = Instruction {
            index: 1,
            stream_offset: 0,
            class: ClassId {
                name: "CVmSync".into(),
                schema: 17,
            },
            address: 28,
            next_address: 52,
            flags: 0,
            arguments: vec![
                Argument::Reference(output.clone()),
                Argument::Number(0),
                Argument::Number(0),
            ],
        };
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 64,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    instruction,
                    sync.clone(),
                    Instruction {
                        index: 2,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmRet".into(),
                            schema: 17,
                        },
                        address: 52,
                        next_address: u32::MAX,
                        flags: 0,
                        arguments: vec![],
                    },
                ],
            },
        )?;
        game.vm = Vm::new(Arc::new(program), 1)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        let Value::Reference(handle) = game.vm.evaluate(&Argument::Reference(output.clone()))?
        else {
            bail!("fade output was not written to script variable");
        };
        assert_eq!(handle.class.as_ref().unwrap().name, "CGenericUso");
        assert!(handle.offset.is_none() && handle.size.is_none());
        assert_eq!(
            handle.inline.as_ref().unwrap().identity(),
            game.async_images[0].identity()
        );
        // Query without disturbing the original blocked VM or fade clock.
        let query = |game: &mut Game, active: bool| -> Result<()> {
            let mut program = Program::default();
            let mut query = sync.clone();
            query.address = 0;
            query.next_address = 0;
            let Value::Reference(current_handle) =
                game.vm.evaluate(&Argument::Reference(output.clone()))?
            else {
                bail!("current fade handle missing")
            };
            query.arguments[0] = Argument::Reference(current_handle);
            query.arguments[1] = Argument::Number(34);
            query.arguments[2] = Argument::Number(4000);
            program.insert(
                0,
                Scenario {
                    object_id: 1,
                    memory_size: 64,
                    null_records: vec![],
                    trailing_bytes: 0,
                    instructions: vec![query],
                },
            )?;
            let original = std::mem::replace(&mut game.vm, Vm::new(Arc::new(program), 1)?);
            let Step::Host { instruction, .. } = game.vm.step()? else {
                bail!("query did not yield")
            };
            game.host(&instruction)?;
            assert_eq!(game.vm.synchronization_status, Some(active));
            game.vm = original;
            Ok(())
        };
        query(&mut game, true)?;
        super::actor_snapshot::test_roundtrip(&mut game)?;
        let Layer::Screen(red) = &game.objects[&id].layers[&0] else {
            bail!("restored image missing")
        };
        let mut red = Arc::clone(red);
        for (dt, alpha) in [(1000, 128), (1000, 255), (2500, 128), (499, 0)] {
            assert_eq!(game.step(dt)?, RunState::Waiting);
            assert_eq!(game.objects[&id].opacity, alpha);
            let mut expected = Framebuffer::new(4, 4)?;
            expected.clear([0, 0, 255]);
            expected.blit(&red, (1, 1, 2, 2), alpha)?;
            assert_eq!(game.frame(), expected.rgba);
            if dt == 1000 {
                super::actor_snapshot::test_roundtrip(&mut game)?;
                let Layer::Screen(current) = &game.objects[&id].layers[&0] else {
                    bail!("restored image missing")
                };
                red = Arc::clone(current);
            }
        }
        assert_eq!(game.step(1)?, RunState::Ended);
        query(&mut game, false)?;
        let object = &game.objects[&id];
        assert!(!object.visible);
        assert_eq!(object.rect, (1, 1, 0, 0));
        assert_eq!(object.opacity, 255);
        let Layer::Screen(texture) = &object.layers[&0] else {
            bail!("image changed")
        };
        assert!(Arc::ptr_eq(texture, &red));
        assert!(game.pending_sounds().is_empty());
        // The ending uses mode 32 with a nonzero, ignored time word. It
        // reports the pre-cancellation state, then cleans up immediately;
        // mode 33 selects the other native cancellation event.
        let ending = Scenario::decode_with_schema(
            &Script::load(&game.archive, 244)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let ending_query = ending
            .instructions
            .iter()
            .find(|i| i.address == 0x3b7c)
            .context("ending fade query")?;
        assert!(matches!(ending_query.arguments[1], Argument::Number(32)));
        assert!(matches!(ending_query.arguments[2], Argument::Number(4000)));
        let Value::Reference(destination) = game.vm.resolve_object(&ending_query.arguments[0])?
        else {
            bail!("ending query destination missing")
        };
        assert_eq!(destination.offset, output.offset);
        for (mode, ignored_time) in [(32, 4000), (33, u32::MAX)] {
            let mut query = ending_query.clone();
            query.address = 0;
            query.next_address = 24;
            query.arguments[1] = Argument::Number(mode);
            query.arguments[2] = Argument::Number(ignored_time);
            let mut program = Program::default();
            program.insert(
                0,
                Scenario {
                    object_id: 1,
                    memory_size: 40,
                    null_records: vec![],
                    trailing_bytes: 0,
                    instructions: vec![
                        query,
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
            let program = Arc::new(program);
            game.vm = Vm::new(Arc::clone(&program), 1)?;
            game.ended = false;
            game.objects.get_mut(&id).unwrap().rect = (1, 1, 2, 2);
            game.fade_image(target, &message.fields)?;
            game.wait_ms = None;
            assert_eq!(game.step(0)?, RunState::Ended);
            assert_eq!(game.vm.synchronization_status, Some(true));
            let actor = game.async_images.last().unwrap();
            assert!(actor.cancelled && actor.finished());
            assert_eq!(actor.elapsed, 0); // query does not advance any clock
            let handle = actor.handle.clone();
            assert!(!game.objects[&id].visible);
            assert_eq!(game.objects[&id].rect, (1, 1, 0, 0));
            assert_eq!(game.objects[&id].opacity, 255);
            assert!(game.wait_ms.is_none() && game.image_wait.is_none());
            game.vm = Vm::new(program, 1)?;
            game.ended = false;
            game.vm.write_object(&output, Value::Reference(handle))?;
            assert_eq!(game.step(0)?, RunState::Ended);
            assert_eq!(game.vm.synchronization_status, Some(false));
        }
        Ok(())
    }

    #[test]
    fn async_image_envelope_keeps_native_q8_edges_and_cleanup() {
        let mut fade = ImageFade {
            handle: ObjectRef::transient(ClassId {
                name: "CGenericUso".into(),
                schema: 1,
            }),
            window: 1,
            elapsed: 0,
            total: 5000,
            fade_in: 2000,
            fade_out: 1000,
            cancelled: false,
        };
        for (elapsed, alpha) in [
            (0, 0),
            (1000, 128),
            (1999, 255),
            (2000, 255),
            (4000, 255),
            (4500, 128),
            (4999, 0),
        ] {
            fade.elapsed = elapsed;
            assert_eq!(fade.alpha(), alpha);
            assert!(!fade.finished());
        }
        let mut object = ObjectState {
            rect: (12, 34, 100, 200),
            ..ObjectState::default()
        };
        fade.elapsed = 5000;
        fade.apply(&mut object);
        assert!(fade.finished());
        assert!(!object.visible);
        assert_eq!(object.rect, (12, 34, 0, 0));
        assert_eq!(object.opacity, 255);
        assert!(object.layers.is_empty());
    }
}
