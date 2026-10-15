//! CParseBgBox OM_AddPostureMomentum, Vm60 10040fe0/10041bb0.
use super::*;
use crate::{motion::Motion, posture::PostureMotion};
use rugp_assets::{motion::MotionCurve, posture::Posture};

#[derive(Clone)]
pub(super) struct PostureInstance {
    source: rugp_assets::object::InlineIdentity,
    motion: PostureMotion,
}
pub(super) struct PostureWait {
    source: rugp_assets::object::InlineIdentity,
    snap: bool,
    remaining_ms: Option<u32>,
}

impl Game {
    pub(super) fn add_posture(
        &mut self,
        target: &ObjectRef,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let media = match fields.get("姿勢運動メディア") {
            None | Some(Value::Null) => return Ok(()),
            Some(Value::Reference(media)) if media.class.is_none() => return Ok(()),
            Some(Value::Reference(media)) => media,
            _ => bail!("invalid posture media reference"),
        };
        ensure!(
            target
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CParseBgBox"),
            "posture target class is not implemented yet"
        );
        let id = target.offset.context("posture target identity missing")?;
        ensure!(
            self.parse_transition.is_none() && self.transition.is_none(),
            "posture activation during a screen transition is not implemented yet"
        );
        let posture = Posture::from_inline(
            media
                .inline
                .as_ref()
                .context("external posture is not implemented yet")?,
            media.class.as_ref().context("posture class missing")?,
        )?;
        let source = media.inline.as_ref().unwrap().identity();
        ensure!(
            !self
                .objects
                .values()
                .any(|o| o.postures.iter().any(|p| p.source == source)),
            "cannot activate an already active posture instance"
        );
        ensure!(
            posture.references[0].class.is_none(),
            "serialized posture chains are not implemented yet"
        );
        let mut channels = [None, None, None];
        for (channel, reference) in channels.iter_mut().zip(&posture.references[2..]) {
            let Some(class) = &reference.class else {
                continue;
            };
            let curve = if let Some(inline) = &reference.inline {
                MotionCurve::from_inline(inline, class)?
            } else {
                let offset = reference.offset.context("motion offset missing")?;
                let size = reference.size.context("motion size missing")?;
                MotionCurve::parse(&self.archive.read_at(offset, size as usize)?, class)?
            };
            *channel = Some(Motion::new(curve)?);
        }
        let mut motion = PostureMotion::new(&posture.fields, channels)?;
        let object = self.objects.entry(id).or_default();
        ensure!(
            object.pending_camera.is_none(),
            "posture with a prepared camera transition is not implemented yet"
        );
        if motion.instantaneous() {
            let camera = object.prepare_camera(motion.apply_end(object.camera))?;
            object.camera = camera;
            self.present_scene()?;
        } else {
            // Native inserts at the list head and samples newest first.
            object
                .postures
                .insert(0, PostureInstance { source, motion });
        }
        Ok(())
    }
    pub(super) fn advance_postures(&mut self, dt_ms: u32) -> Result<()> {
        if !self.objects.values().any(|o| !o.postures.is_empty()) {
            return Ok(());
        }
        ensure!(
            self.parse_transition.is_none() && self.transition.is_none(),
            "concurrent posture and screen transition is not implemented yet"
        );
        for object in self.objects.values_mut() {
            let mut remaining = Vec::with_capacity(object.postures.len());
            // Validate the next state before committing either camera/history.
            // Apply bounds after EACH native moment, newest first.
            let mut camera = object.camera;
            for mut instance in object.postures.clone() {
                camera = object.prepare_camera(instance.motion.advance(camera, dt_ms))?;
                if instance.motion.active() {
                    remaining.push(instance);
                }
            }
            object.camera = camera;
            object.postures = remaining;
        }
        self.present_scene()
    }
    pub(super) fn synchronize_posture(
        &mut self,
        reference: &ObjectRef,
        mode: i32,
        timeout: i32,
    ) -> Result<()> {
        // Vm60 10072ba0 queries the event for modes 0x20/0x21. Posture's
        // event status (1001dd70) is ACTIVE, unlike a media completion event.
        // 10072f15 sends event 1 for mode 0x21, removing it without snapping.
        ensure!(
            matches!(mode, 16 | 17 | 32 | 33) && timeout >= 0,
            "unsupported posture synchronization mode/timeout {mode}/{timeout}"
        );
        let source = reference
            .inline
            .as_ref()
            .context("external posture synchronization is not implemented yet")?
            .identity();
        let active = self.posture_active(&source);
        if mode & 0x30 == 0x20 {
            self.finish_posture(&source, mode & 1 == 0)?;
            return self.vm.acknowledge_sync_query(active);
        }
        if active {
            self.posture_wait = Some(PostureWait {
                source,
                snap: mode & 1 == 0,
                remaining_ms: (timeout > 0).then_some(timeout as u32),
            });
            Ok(())
        } else {
            self.vm.acknowledge_host()
        }
    }
    fn posture_active(&self, source: &rugp_assets::object::InlineIdentity) -> bool {
        self.objects
            .values()
            .any(|o| o.postures.iter().any(|p| p.source == *source))
    }
    fn finish_posture(
        &mut self,
        source: &rugp_assets::object::InlineIdentity,
        snap: bool,
    ) -> Result<()> {
        let mut changed = false;
        for object in self.objects.values_mut() {
            if !object.postures.iter().any(|p| p.source == *source) {
                continue;
            }
            if snap {
                let mut camera = object.camera;
                for instance in object.postures.iter_mut().filter(|p| p.source == *source) {
                    camera = instance.motion.apply_end(camera);
                }
                object.camera = object.prepare_camera(camera)?;
                changed = true;
            }
            object.postures.retain(|p| p.source != *source);
        }
        if changed {
            self.present_scene()?;
        }
        Ok(())
    }
    pub(super) fn advance_posture_wait(&mut self, dt_ms: u32) -> Result<bool> {
        let Some(mut wait) = self.posture_wait.take() else {
            return Ok(false);
        };
        if let Some(remaining) = &mut wait.remaining_ms {
            *remaining = remaining.saturating_sub(dt_ms);
        }
        if self.posture_active(&wait.source) && wait.remaining_ms != Some(0) {
            self.posture_wait = Some(wait);
            return Ok(true);
        }
        self.finish_posture(&wait.source, wait.snap)?;
        self.vm.acknowledge_host()?;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugp_assets::object::{ClassId, ObjectReader};
    use rugp_scene_vm::{Argument, Scenario};
    fn words(bytes: &mut Vec<u8>, values: &[i32]) {
        for value in values {
            bytes.extend(value.to_le_bytes());
        }
    }
    fn media(duration: i32, flags: u16) -> ObjectRef {
        let mut bytes = vec![0, 2, 0, 0]; // posture header and first ancestry count
        bytes.extend([0; 4]); // previous/owner null
        bytes.extend(0xffffu16.to_le_bytes());
        bytes.extend(3u16.to_le_bytes());
        bytes.extend(("CMoveAcsOngen".len() as u16).to_le_bytes());
        bytes.extend(b"CMoveAcsOngen");
        bytes.extend(0x40u16.to_le_bytes());
        words(&mut bytes, &[0; 3]);
        bytes.extend([2, 1]);
        words(&mut bytes, &[100, 4, 0, 0]);
        bytes.extend([0; 2]); // null time curve
        bytes.push(0x70);
        words(&mut bytes, &[0, duration]);
        bytes.extend(1000f32.to_le_bytes());
        bytes.push(1);
        bytes.extend(flags.to_le_bytes());
        bytes.extend([0; 4]); // angular/other null
        bytes.extend([0; 58]);
        for _ in 0..3 {
            bytes.extend(1f32.to_le_bytes());
        }
        let class = ClassId {
            name: "CPostureMoment".into(),
            schema: 4,
        };
        let mut reader = ObjectReader::new(&bytes);
        let inline = reader.read_inline(&class, 0).unwrap();
        assert_eq!(reader.input.remaining(), 0);
        ObjectRef {
            class: Some(class),
            cache_index: None,
            flags: 0x40,
            offset: None,
            size: None,
            owner: None,
            selector: 0,
            inline: Some(inline),
        }
    }
    fn target() -> ObjectRef {
        ObjectRef {
            class: Some(ClassId {
                name: "CParseBgBox".into(),
                schema: 15,
            }),
            cache_index: None,
            flags: 0,
            offset: Some(212259),
            size: Some(118),
            owner: None,
            selector: 0,
            inline: None,
        }
    }
    fn install_sync(game: &mut Game, reference: ObjectRef, mode: i32) -> Result<()> {
        install_sync_timeout(game, reference, mode, 0)
    }
    fn install_sync_timeout(
        game: &mut Game,
        reference: ObjectRef,
        mode: i32,
        timeout: u32,
    ) -> Result<()> {
        let null = ObjectReader::new(&[0; 3]).reference(0)?;
        let ins = |address, next_address, args| Instruction {
            index: 0,
            stream_offset: 0,
            class: ClassId {
                name: "CVmSync".into(),
                schema: 17,
            },
            address,
            next_address,
            flags: 0,
            arguments: args,
        };
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 128,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    ins(
                        0,
                        32,
                        vec![
                            Argument::Reference(reference),
                            Argument::Number(mode as u32),
                            Argument::Number(timeout),
                        ],
                    ),
                    ins(
                        32,
                        64,
                        vec![
                            Argument::Reference(null),
                            Argument::Number(0),
                            Argument::Number(1000000),
                        ],
                    ),
                ],
            },
        )?;
        game.vm = Vm::new(Arc::new(program), 1)?;
        game.wait_ms = None;
        game.posture_wait = None;
        Ok(())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_host_posture_clock_pixels_query_cancel_and_instance_identity() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        game.objects.clear();
        game.frame = Framebuffer::new(4, 4)?;
        let texture = Arc::new(Texture {
            width: 16,
            height: 16,
            display_scale: 1.0,
            rgba: (0..16)
                .flat_map(|y| (0..16).flat_map(move |x| [x * 10, y * 10, 30, 255]))
                .collect(),
        });
        let plane = crate::world::Plane::bitmap(&texture)?;
        let boundary = crate::world::Plane::bitmap(&Texture {
            width: 4096,
            height: 4096,
            display_scale: 1.0,
            rgba: vec![],
        })?;
        let mut object = ObjectState {
            rect: (0, 0, 4, 4),
            ..ObjectState::default()
        };
        object.layers.insert(
            512,
            Layer::World(WorldImage {
                boundary,
                planes: vec![crate::image_manager::WorldImagePlane {
                    texture,
                    plane,
                    draw_flags: 0,
                }],
            }),
        );
        game.objects.insert(212259, object);
        game.present_scene()?;
        assert_eq!(&game.frame()[..4], &[60, 60, 30, 255]);
        let first = media(400, 1);
        let second = media(400, 1); // same bytes, different native instance
        let add = |r: &ObjectRef| Message {
            class: "OM_AddPostureMomentum".into(),
            target: Value::Reference(target()),
            fields: BTreeMap::from([("姿勢運動メディア".into(), Value::Reference(r.clone()))]),
        };
        game.message(add(&first))?;
        let null = ObjectReader::new(&[0; 3]).reference(0)?;
        install_sync(&mut game, null, 0)?;
        assert_eq!(game.step(100)?, RunState::Waiting);
        assert_eq!(game.objects[&212259].camera.position, [1, 0, 0]);
        assert_eq!(&game.frame()[..4], &[70, 60, 30, 255]);
        assert!(game.message(add(&first.clone())).is_err());
        assert_eq!(game.objects[&212259].active_posture_count(), 1);
        game.message(add(&second))?;
        game.step(100)?;
        assert_eq!(game.objects[&212259].camera.position, [3, 0, 0]);
        assert_eq!(&game.frame()[..4], &[90, 60, 30, 255]);
        install_sync(&mut game, first.clone(), 33)?;
        game.step(0)?;
        assert_eq!(game.vm.synchronization_status, Some(true));
        assert_eq!(game.objects[&212259].active_posture_count(), 1);
        assert_eq!(game.objects[&212259].camera.position, [3, 0, 0]); // cancel must not snap to end
        install_sync(&mut game, first.clone(), 33)?;
        game.step(0)?;
        assert_eq!(game.vm.synchronization_status, Some(false));
        game.step(100)?;
        assert_eq!(game.objects[&212259].camera.position, [4, 0, 0]);
        install_sync(&mut game, second, 33)?;
        game.step(0)?;
        assert_eq!(game.objects[&212259].active_posture_count(), 0);
        game.message(add(&first))?; // restarted instance resets history
        game.step(400)?;
        assert_eq!(game.objects[&212259].camera.position, [8, 0, 0]);
        assert_eq!(game.objects[&212259].active_posture_count(), 1);
        game.step(1)?;
        assert_eq!(game.objects[&212259].active_posture_count(), 0);
        assert_eq!(game.objects[&212259].camera.position, [8, 0, 0]);
        game.message(add(&first))?;
        install_sync(&mut game, first.clone(), 16)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert_eq!(game.vm.cursor().unwrap().address, 0);
        game.step(400)?;
        assert_eq!(game.vm.cursor().unwrap().address, 0); // inclusive endpoint
        game.step(1)?;
        assert_eq!(game.vm.cursor().unwrap().address, 32);
        assert!(game.posture_wait.is_none());
        assert_eq!(game.objects[&212259].camera.position, [12, 0, 0]);
        game.message(add(&first))?;
        install_sync(&mut game, first.clone(), 32)?;
        game.step(0)?;
        assert_eq!(game.vm.synchronization_status, Some(true));
        assert_eq!(game.objects[&212259].active_posture_count(), 0);
        assert_eq!(game.objects[&212259].camera.position, [16, 0, 0]);
        let looping = media(400, 3);
        game.message(add(&looping))?;
        install_sync_timeout(&mut game, looping, 16, 100)?;
        game.step(0)?;
        assert_eq!(game.vm.cursor().unwrap().address, 0);
        game.step(100)?;
        assert_eq!(game.vm.cursor().unwrap().address, 32);
        assert_eq!(game.objects[&212259].active_posture_count(), 0);
        assert_eq!(game.objects[&212259].camera.position, [20, 0, 0]);
        Ok(())
    }
}
