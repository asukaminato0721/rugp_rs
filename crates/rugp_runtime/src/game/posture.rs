//! CParseBgBox OM_AddPostureMomentum, Vm60 10040fe0/10041bb0.
use super::*;
use crate::{motion::Motion, posture::PostureMotion};
use rugp_assets::{
    motion::{MotionCurve, MotionResource},
    posture::Posture,
};

fn real_field(fields: &BTreeMap<String, Value>, name: &str) -> Result<f32> {
    let value = match fields.get(name) {
        None => 0.0,
        Some(Value::Real(n)) => *n as f32,
        Some(Value::Integer(n)) => f32::from_bits(*n as u32),
        _ => bail!("invalid real posture parameter {name}"),
    };
    ensure!(value.is_finite(), "non-finite posture parameter {name}");
    Ok(value)
}

#[derive(Clone)]
pub(super) struct PostureInstance {
    source: PostureIdentity,
    motion: PostureMotion,
}
pub(super) struct PostureWait {
    source: PostureIdentity,
    snap: bool,
    remaining_ms: Option<u32>,
}
#[derive(Clone, PartialEq, Eq)]
enum PostureIdentity {
    Inline(rugp_assets::object::InlineIdentity),
    Stored(u64),
}
impl PostureIdentity {
    fn of(reference: &ObjectRef) -> Result<Self> {
        Ok(if let Some(inline) = &reference.inline {
            Self::Inline(inline.identity())
        } else {
            Self::Stored(reference.offset.context("posture identity missing")?)
        })
    }
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
            target.class.as_ref().is_some_and(|c| matches!(
                c.name.as_str(),
                "CParseBgBox" | "CCharBox" | "CCharBox2"
            )),
            "posture target class is not implemented yet"
        );
        let id = target.offset.context("posture target identity missing")?;
        let class = media.class.as_ref().context("posture class missing")?;
        let posture = if let Some(inline) = &media.inline {
            Posture::from_inline(inline, class)?
        } else {
            Posture::parse(
                &self.archive.read_at(
                    media.offset.context("posture offset missing")?,
                    media.size.context("posture extent missing")? as usize,
                )?,
                class,
                self.archive.address_shift,
            )?
        };
        let source = PostureIdentity::of(media)?;
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
                // Some external extents append native property data after the
                // complete curve. Preserve it in the asset decoder; playback
                // currently uses the validated curve without those properties.
                MotionResource::parse(&self.archive.read_at(offset, size as usize)?, class)?.curve
            };
            *channel = Some(Motion::new(curve)?);
        }
        let mut motion = PostureMotion::new(&posture.fields, channels)?;
        motion.configure_playback(
            number(fields, "時間オフセット", 0)?,
            real_field(fields, "時間スケール")?,
            [
                real_field(fields, "速度スケール")?,
                real_field(fields, "角度スケール")?,
                real_field(fields, "その他スケール")?,
            ],
        )?;
        let object = self.objects.entry(id).or_default();
        // Portable composition serializes these two presentation operations:
        // settle an unstarted camera tween at its bounded endpoint, then start
        // the posture there. Do not discard the posture, its identity or waits.
        // The later ParseScreen still owns its normal rendering/wait clock.
        let prepared = object.pending_camera.is_some();
        let mut camera = if prepared {
            object.camera_target()?
        } else {
            object.camera
        };
        if motion.instantaneous() {
            camera = object.prepare_camera(motion.apply_end(camera))?;
        }
        if prepared {
            object.pending_camera = None;
            object.camera_start = None;
            object.camera = camera;
        }
        if motion.instantaneous() {
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
        let source = PostureIdentity::of(reference)?;
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
    fn posture_active(&self, source: &PostureIdentity) -> bool {
        self.objects
            .values()
            .any(|o| o.postures.iter().any(|p| p.source == *source))
    }
    fn finish_posture(&mut self, source: &PostureIdentity, snap: bool) -> Result<()> {
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
    #[test]
    fn posture_source_keys_preserve_cached_inline_and_stored_instance_identity() -> Result<()> {
        let first = media(400, 1);
        assert!(PostureIdentity::of(&first)? == PostureIdentity::of(&first.clone())?);
        assert!(PostureIdentity::of(&first)? != PostureIdentity::of(&media(400, 1))?);
        let mut stored = target();
        stored.class.as_mut().unwrap().name = "CPostureMoment".into();
        let mut cached = stored.clone();
        cached.cache_index = Some(99);
        assert!(PostureIdentity::of(&stored)? == PostureIdentity::of(&cached)?);
        cached.offset = Some(stored.offset.unwrap() + 1);
        assert!(PostureIdentity::of(&stored)? != PostureIdentity::of(&cached)?);
        Ok(())
    }
    #[test]
    fn extended_real_parameters_accept_literals_and_raw_field_bits() -> Result<()> {
        let fields = BTreeMap::from([
            ("literal".into(), Value::Real(2.5)),
            ("bits".into(), Value::Integer(1f32.to_bits() as i32)),
            ("invalid".into(), Value::Real(f64::NAN)),
        ]);
        assert_eq!(real_field(&fields, "literal")?, 2.5);
        assert_eq!(real_field(&fields, "bits")?, 1.0);
        assert_eq!(real_field(&fields, "absent")?, 0.0);
        assert!(real_field(&fields, "invalid").is_err());
        Ok(())
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
        // A real helper can prepare a camera and add a posture before its
        // ParseScreen trigger. Use the queued endpoint as the new base, while
        // preserving the posture's full 400 ms inclusive event lifetime.
        let object = game.objects.get_mut(&212259).unwrap();
        object.camera_start = Some(object.camera);
        object.pending_camera = Some(crate::world::CameraTween::new(
            object.camera,
            Camera {
                position: [30, 0, 0],
                ..object.camera
            },
            1,
        )?);
        game.message(add(&first))?;
        let object = &game.objects[&212259];
        assert!(object.pending_camera.is_none() && object.camera_start.is_none());
        assert_eq!(object.camera.position, [30, 0, 0]);
        assert_eq!(object.active_posture_count(), 1);
        install_sync(&mut game, first, 16)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert_eq!(game.step(400)?, RunState::Waiting);
        assert_eq!(game.vm.cursor().unwrap().address, 0);
        game.step(1)?;
        assert_eq!(game.vm.cursor().unwrap().address, 32);
        assert_eq!(game.objects[&212259].active_posture_count(), 0);
        assert_eq!(game.objects[&212259].camera.position, [34, 0, 0]);
        assert!(!game.start_parse_transition(100)?);
        let object = game.objects.get_mut(&212259).unwrap();
        object.pending_camera = Some(crate::world::CameraTween::new(
            object.camera,
            Camera {
                position: [40, 0, 0],
                ..object.camera
            },
            1,
        )?);
        let wait = game.wait_ms;
        game.message(add(&media(0, 1)))?;
        assert_eq!(game.wait_ms, wait);
        assert_eq!(game.objects[&212259].camera.position, [44, 0, 0]);
        assert_eq!(game.objects[&212259].active_posture_count(), 0);
        assert!(game.objects[&212259].pending_camera.is_none());
        Ok(())
    }
}
