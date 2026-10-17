//! ParseScreen-driven object properties, separate from framebuffer CFade effects.
use super::*;
use rugp_assets::mime::MimeCurve;

/// Vm60 10040c10 is OM_ParseScreenTrigger (dispatch pair 100a4d80).
/// 10040d52..10040d7c enters the window's wait message unless style bit
/// 0x8000 is set. The earlier setup always starts the same render clock.
pub(super) fn validate_trigger(fields: &BTreeMap<String, Value>) -> Result<()> {
    let style = number(fields, "スタイル", 0)?;
    ensure!(
        matches!(style, 0 | 0x8000),
        "ParseScreen trigger style {style:#x} is not implemented yet"
    );
    for name in ["マイムデータ", "その他データ", "非同期ハンドル"] {
        ensure!(
            matches!(fields.get(name), None | Some(Value::Null)),
            "ParseScreen trigger {name} is not implemented yet"
        );
    }
    Ok(())
}

pub(super) struct ParseTransition {
    pub(super) scene: BTreeMap<u64, ObjectState>,
    pub(super) curves: BTreeMap<u64, MimeCurve>,
    pub(super) cameras: BTreeMap<u64, crate::world::CameraTween>,
    pub(super) elapsed: u32,
    pub(super) duration: u32,
    /// Scene cleared by a timed background HumanOut, still swept by the
    /// moving camera while it fades out (retail keeps drawing it).
    pub(super) outgoing: Option<(BTreeMap<u64, ObjectState>, u32)>,
}
impl Game {
    pub(super) fn set_draw_ratio(
        &mut self,
        target: &ObjectRef,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        ensure!(
            target
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CParseBgBox"),
            "draw-ratio target class is not implemented yet"
        );
        let id = target
            .offset
            .context("draw-ratio target identity missing")?;
        let mode = number(fields, "モード", 0)? as u32;
        // In mode 0x100 the ParseScreen coordinator supplies progress;
        // this message's own time field does not start a separate clock.
        number(fields, "時間", 0)?;
        ensure!(
            mode & 0x100 != 0 && mode & !0x1ff == 0,
            "independent-clock draw-ratio mode {mode:#x} is not implemented yet"
        );
        let curve = match fields.get("比重曲線") {
            None | Some(Value::Null) => {
                ensure!(
                    mode & 255 <= 1,
                    "default draw-ratio curve mode {mode:#x} is not implemented yet"
                );
                MimeCurve::alpha_ramp(mode & 255 == 1)
            }
            Some(Value::Reference(reference)) => {
                let class = reference
                    .class
                    .as_ref()
                    .context("draw-ratio curve class missing")?;
                ensure!(
                    class.name == "CMimeFlat16LimitOne" && class.schema == 1,
                    "unsupported draw-ratio curve {}#{}",
                    class.name,
                    class.schema
                );
                if let Some(inline) = &reference.inline {
                    MimeCurve::from_inline(&inline.data, class)?
                } else {
                    ensure!(
                        reference.size == Some(37),
                        "invalid sampled draw-ratio curve extent"
                    );
                    MimeCurve::parse(
                        &self.archive.read_at(
                            reference
                                .offset
                                .context("draw-ratio curve offset missing")?,
                            37,
                        )?,
                        class,
                    )?
                }
            }
            _ => bail!("invalid draw-ratio curve argument"),
        };
        // Native 100447e0 replaces state 2 with pending state 1; the old
        // ParseScreen clock must stop sampling this object's former curve.
        if let Some(parse) = &mut self.parse_transition {
            parse.curves.remove(&id);
        }
        let object = self.objects.entry(id).or_default();
        object.pending_draw_ratio = Some(curve);
        object.properties.extend(fields.clone());
        Ok(())
    }
    /// Native 10042a8a activates mode-0x100 requests. Keep a render snapshot
    /// so subsequent image commands remain staged until explicit presentation.
    pub(super) fn start_parse_transition(&mut self, duration: u32) -> Result<bool> {
        ensure!(
            self.parse_transition.is_none(),
            "overlapping ParseScreen transitions are not implemented yet"
        );
        if !self
            .objects
            .values()
            .any(|o| o.pending_draw_ratio.is_some() || o.pending_camera.is_some())
        {
            return Ok(false);
        }
        let curves: BTreeMap<_, _> = self
            .objects
            .iter_mut()
            .filter_map(|(&id, o)| o.pending_draw_ratio.take().map(|c| (id, c)))
            .collect();
        let cameras: BTreeMap<_, _> = self
            .objects
            .iter_mut()
            .filter_map(|(&id, o)| o.pending_camera.take().map(|t| (id, t)))
            .collect();
        let cameras_present = !cameras.is_empty();
        self.parse_transition = Some(ParseTransition {
            scene: self.objects.clone(),
            curves,
            cameras,
            elapsed: 0,
            duration,
            outgoing: if cameras_present {
                self.outgoing_scene.take()
            } else {
                None
            },
        });
        self.outgoing_scene = None;
        self.transition = None;
        self.advance_parse_transition(0)?;
        self.finish_parse_transition();
        Ok(true)
    }
    pub(super) fn advance_parse_transition(&mut self, dt_ms: u32) -> Result<()> {
        let size = self.size();
        let Some(parse) = &mut self.parse_transition else {
            return Ok(());
        };
        parse.elapsed = parse.elapsed.saturating_add(dt_ms).min(parse.duration);
        let progress = progress_q16(parse.elapsed, parse.duration);
        let afterimage = !parse.cameras.is_empty() && dt_ms > 0 && parse.elapsed < parse.duration;
        for (&id, tween) in &parse.cameras {
            let mut camera = tween.sample(progress)?;
            if let Some(object) = parse.scene.get_mut(&id) {
                camera = object.prepare_camera(camera)?;
                object.camera = camera;
            }
            if let Some(object) = parse.outgoing.as_mut().and_then(|(s, _)| s.get_mut(&id)) {
                object.camera = camera;
            }
            if let Some(object) = self.objects.get_mut(&id) {
                object.camera = camera;
            }
        }
        for (&id, curve) in &parse.curves {
            // Vm60 10044774 shifts Q5 output, then clamps to native u8.
            let opacity = curve_opacity(curve, progress);
            if let Some(object) = parse.scene.get_mut(&id) {
                object.opacity = opacity;
            }
            if let Some(object) = self.objects.get_mut(&id) {
                object.opacity = opacity;
            }
        }
        let mut next = Self::render_objects_mode(&parse.scene, size, self.rasterize_scene)?;
        for (&id, snapshot) in &parse.scene {
            if let Some(object) = self.objects.get(&id) {
                object.camera_center_y.set(snapshot.camera_center_y.get());
            }
        }
        if let Some((scene, fade_ms)) = &parse.outgoing {
            let remaining = 256
                - (u64::from(parse.elapsed) * 256 / u64::from((*fade_ms).max(1))).min(256) as u32;
            if remaining > 0 {
                let old = Self::render_objects_mode(scene, size, self.rasterize_scene)?;
                for (out, &from) in next.rgba.iter_mut().zip(&old.rgba) {
                    *out =
                        ((u32::from(from) * remaining + u32::from(*out) * (256 - remaining) + 128)
                            >> 8) as u8;
                }
            }
        }
        if afterimage && next.rgba.len() == self.frame.rgba.len() {
            blend_afterimage(&mut next.rgba, &self.frame.rgba, dt_ms);
        }
        self.frame = next;
        Ok(())
    }

    pub(super) fn finish_parse_transition(&mut self) {
        let Some(parse) = &self.parse_transition else {
            return;
        };
        // Native 10047170 releases synchronized curve state, not opacity.
        // Retain the captured scene until the final media tick renders it.
        if parse.elapsed == parse.duration {
            for id in parse.scene.keys() {
                if let Some(object) = self.objects.get_mut(id) {
                    object.pending_draw_ratio = None;
                    object.pending_camera = None;
                    object.camera_start = None;
                }
            }
            self.parse_transition = None;
        }
    }
}

/// Retail camera moves under ParseScreen leave trails of earlier frames
/// (rvmm CMimeExclusiveForAfterImage keeps a constant weight until progress
/// reaches 1.0, then yields the exact final frame). Recorded retail pans
/// fade a trail over roughly this many milliseconds.
const AFTERIMAGE_MS: f64 = 80.0;

fn blend_afterimage(next: &mut [u8], previous: &[u8], dt_ms: u32) {
    let keep = ((-f64::from(dt_ms) / AFTERIMAGE_MS).exp() * 256.0).round() as u32;
    for (out, &old) in next.iter_mut().zip(previous) {
        *out = ((u32::from(old) * keep + u32::from(*out) * (256 - keep) + 128) >> 8) as u8;
    }
}

fn progress_q16(elapsed: u32, duration: u32) -> i32 {
    if duration == 0 {
        65536
    } else {
        (u64::from(elapsed.min(duration)) * 65536 / u64::from(duration)) as i32
    }
}
fn curve_opacity(curve: &MimeCurve, progress: i32) -> u8 {
    (curve.evaluate(progress) >> 5).clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugp_assets::object::ClassId;

    #[test]
    fn afterimage_trail_decays_with_elapsed_time() {
        let mut next = [200u8; 4];
        blend_afterimage(&mut next, &[0; 4], 0);
        assert_eq!(next, [0; 4], "no time elapsed keeps the shown frame");
        let mut next = [200u8; 4];
        blend_afterimage(&mut next, &[0; 4], 80);
        // exp(-1) of the old frame remains after one time constant.
        assert_eq!(next, [127; 4]);
        let mut next = [200u8; 4];
        blend_afterimage(&mut next, &[0; 4], 2000);
        assert_eq!(next, [200; 4]);
    }
    #[test]
    fn progress_and_signed_opacity_cover_zero_duration_extremes_and_q5_steps() {
        assert_eq!(progress_q16(0, 0), 65536);
        assert_eq!(progress_q16(0, 400), 0);
        assert_eq!(progress_q16(100, 400), 16384);
        assert_eq!(progress_q16(800, 400), 65536);
        assert_eq!(progress_q16(u32::MAX, u32::MAX), 65536);
        assert_eq!(curve_opacity(&MimeCurve::alpha_ramp(false), 32768), 128);
        for (value, opacity) in [
            (i16::MIN, 0),
            (-1, 0),
            (31, 0),
            (32, 1),
            (1056, 33),
            (8192, 255),
            (i16::MAX, 255),
        ] {
            assert_eq!(
                curve_opacity(
                    &MimeCurve {
                        samples: [value; 17]
                    },
                    12345
                ),
                opacity
            );
        }
    }
    fn image(color: [u8; 4]) -> Layer {
        let texture = Arc::new(Texture {
            width: 4,
            height: 4,
            display_scale: 1.0,
            rgba: color.repeat(16),
        });
        let plane = crate::world::Plane::bitmap(&texture).unwrap();
        Layer::World(WorldImage {
            boundary: plane,
            planes: vec![crate::image_manager::WorldImagePlane {
                texture,
                plane,
                draw_flags: 0,
            }],
        })
    }
    #[test]
    fn projection_cache_updates_on_render_and_snapshot_copies_are_independent() -> Result<()> {
        let mut object = ObjectState {
            rect: (0, 0, 4, 4),
            camera: Camera {
                focal_length: 1440,
                angles_q4: [0, -32, 0],
                ..Camera::default()
            },
            ..ObjectState::default()
        };
        object.layers.insert(512, image([255; 4]));
        let snapshot = object.clone();
        let mut frame = Framebuffer::new(4, 4)?;
        object.render(&mut frame)?;
        assert_eq!(object.camera_center_y.get(), 7);
        assert_eq!(snapshot.camera_center_y.get(), 0);
        snapshot.render(&mut frame)?;
        assert_eq!(snapshot.camera_center_y.get(), 7);
        // Preparing another camera does not overwrite cached geometry.
        object.camera.angles_q4 = [0; 3];
        assert_eq!(object.camera_center_y.get(), 7);
        object.render(&mut frame)?;
        assert_eq!(object.camera_center_y.get(), 2);
        assert_eq!(snapshot.camera_center_y.get(), 7);
        let mut objects = BTreeMap::from([(1, snapshot)]);
        objects.get_mut(&1).unwrap().camera.angles_q4 = [0; 3];
        let normal = Game::render_objects_mode(&objects, (4, 4), true)?;
        let center = objects[&1].camera_center_y.get();
        objects[&1].camera_center_y.set(0);
        let diagnostic = Game::render_objects_mode(&objects, (4, 4), false)?;
        assert_eq!(objects[&1].camera_center_y.get(), center);
        assert_ne!(normal.rgba, diagnostic.rgba);
        assert!(
            diagnostic
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [0, 0, 0, 255])
        );
        objects.get_mut(&1).unwrap().camera.position[2] = 720;
        let diagnostic = Game::render_objects_mode(&objects, (4, 4), false)?;
        let clipped = Game::render_objects_mode(&objects, (4, 4), true)?;
        assert_eq!(clipped.rgba, diagnostic.rgba);
        assert_eq!(objects[&1].camera_center_y.get(), 1);
        Ok(())
    }
    #[test]
    fn object_opacity_is_applied_to_each_world_plane_in_draw_order() -> Result<()> {
        let mut red = image([255, 0, 0, 255]);
        let Layer::World(red_group) = &mut red else {
            unreachable!()
        };
        let Layer::World(mut blue_group) = image([0, 0, 255, 255]) else {
            unreachable!()
        };
        red_group.planes.append(&mut blue_group.planes);
        let mut object = ObjectState {
            rect: (0, 0, 4, 4),
            opacity: 128,
            ..ObjectState::default()
        };
        object.layers.insert(512, red);
        let mut frame = Framebuffer::new(4, 4)?;
        object.render(&mut frame)?;
        assert!(
            frame
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [64, 0, 128, 255])
        );
        let Layer::World(group) = object.layers.get_mut(&512).unwrap() else {
            unreachable!()
        };
        group.planes[1].draw_flags |= 2;
        frame.clear([0, 0, 0]);
        object.render(&mut frame)?;
        assert!(
            frame
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [128, 0, 0, 255])
        );
        Ok(())
    }
    fn reference(name: &str, schema: u16, offset: u64, size: u32) -> ObjectRef {
        ObjectRef {
            class: Some(ClassId {
                name: name.into(),
                schema,
            }),
            cache_index: None,
            flags: 0xc108,
            offset: Some(offset),
            size: Some(size),
            owner: None,
            selector: 0,
            inline: None,
        }
    }
    fn message(id: u64, name: &str, fields: BTreeMap<String, Value>) -> Message {
        Message {
            class: name.into(),
            target: Value::Reference(reference("CParseBgBox", 15, id, 118)),
            fields,
        }
    }
    #[test]
    fn trigger_rejects_unknown_styles_effects_and_output_handles() {
        for style in [0, 0x8000] {
            assert!(
                validate_trigger(&BTreeMap::from([(
                    "スタイル".into(),
                    Value::Integer(style)
                )]))
                .is_ok()
            );
        }
        assert!(
            validate_trigger(&BTreeMap::from([("スタイル".into(), Value::Integer(1))])).is_err()
        );
        for field in ["マイムデータ", "その他データ", "非同期ハンドル"] {
            assert!(
                validate_trigger(&BTreeMap::from([(
                    field.into(),
                    Value::Reference(reference("CVmVarObj", 1, 900, 7))
                )]))
                .is_err()
            );
            assert!(validate_trigger(&BTreeMap::from([(field.into(), Value::Null)])).is_ok());
        }
    }
    fn trigger(duration: i32) -> Message {
        message(
            212259,
            "OM_ParseScreenTrigger",
            BTreeMap::from([
                ("総処理時間基".into(), Value::Integer(duration)),
                ("スタイル".into(), Value::Integer(0x8000)),
            ]),
        )
    }
    fn ratio(id: u64, mode: i32, curve: Value) -> Message {
        message(
            id,
            "OM_SetDrawRatioAsParseWnd",
            BTreeMap::from([
                ("モード".into(), Value::Integer(mode)),
                ("比重曲線".into(), curve),
                ("時間".into(), Value::Integer(12345)), // ignored by synchronized mode
            ]),
        )
    }
    fn pixels(game: &Game, color: [u8; 4]) {
        assert!(
            game.frame().as_chunks::<4>().0.iter().all(|p| *p == color),
            "expected {color:?}, first pixel {:?}",
            &game.frame()[..4]
        );
    }
    fn waiting_game() -> Result<Game> {
        use rugp_scene_vm::{Argument, Scenario};
        let root = std::env::var_os("RUGP_TEST_GAME").context("game directory")?;
        let mut game = Game::open(root.as_ref())?;
        game.frame = Framebuffer::new(4, 4)?;
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 64,
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
                        next_address: 36,
                        flags: 0,
                        arguments: vec![
                            Argument::Reference(
                                rugp_assets::object::ObjectReader::new(&[0; 3]).reference(0)?,
                            ),
                            Argument::Number(0),
                            Argument::Number(1000000),
                        ],
                    },
                    Instruction {
                        index: 1,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmRet".into(),
                            schema: 17,
                        },
                        address: 36,
                        next_address: 48,
                        flags: 0,
                        arguments: vec![],
                    },
                ],
            },
        )?;
        game.vm = Vm::new(Arc::new(program), 1)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        Ok(game)
    }

    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_parse_trigger_waits_for_clock_unless_explicitly_asynchronous() -> Result<()> {
        use rugp_scene_vm::{Argument, Scenario};
        let mut game = waiting_game()?;
        let install = |game: &mut Game, style: u32, duration: u32| -> Result<()> {
            let mut program = Program::default();
            let ins = |address, class: &str, arguments| Instruction {
                index: address / 64,
                stream_offset: 0,
                class: ClassId {
                    name: class.into(),
                    schema: 17,
                },
                address,
                next_address: address + 64,
                flags: 0,
                arguments,
            };
            program.insert(
                0,
                Scenario {
                    object_id: 1,
                    memory_size: 192,
                    null_records: vec![],
                    trailing_bytes: 0,
                    instructions: vec![
                        ins(
                            0,
                            "CVmGenericMsg",
                            vec![
                                Argument::Reference(reference("CParseBgBox", 15, 212259, 118)),
                                Argument::Message {
                                    class: ClassId {
                                        name: "OM_ParseScreenTrigger".into(),
                                        schema: 1,
                                    },
                                    arguments: [("スタイル", style), ("総処理時間基", duration)]
                                        .map(|(name, value)| Argument::Field {
                                            name: name.into(),
                                            offset: 0,
                                            argument: Box::new(Argument::Number(value)),
                                        })
                                        .to_vec(),
                                },
                                Argument::Number(0),
                            ],
                        ),
                        ins(
                            64,
                            "CVmSync",
                            vec![
                                Argument::Reference(
                                    rugp_assets::object::ObjectReader::new(&[0; 3]).reference(0)?,
                                ),
                                Argument::Number(0),
                                Argument::Number(1000000),
                            ],
                        ),
                        ins(128, "CVmRet", vec![]),
                    ],
                },
            )?;
            game.vm = Vm::new(Arc::new(program), 1)?;
            game.wait_ms = None;
            Ok(())
        };
        // Both the legacy framebuffer fallback and live property tracks
        // must hold the actual VM instruction, without reissuing the message.
        for live in [false, true] {
            for style in [0, 0x8000] {
                install(&mut game, style, 400)?;
                let mut object = ObjectState {
                    rect: (0, 0, 4, 4),
                    ..ObjectState::default()
                };
                object.layers.insert(512, image([255; 4]));
                if live {
                    object.pending_draw_ratio = Some(MimeCurve::alpha_ramp(false));
                }
                game.objects.insert(212259, object);
                game.frame.clear([0; 3]);
                assert_eq!(game.step(0)?, RunState::Waiting);
                let expected_pc = if style == 0 { 0 } else { 64 };
                assert_eq!(game.vm.cursor().unwrap().address, expected_pc);
                assert_eq!(game.parse_wait, style == 0);
                pixels(&game, [0, 0, 0, 255]);
                game.step(200)?;
                assert_eq!(game.vm.cursor().unwrap().address, expected_pc);
                pixels(&game, [128, 128, 128, 255]);
                game.step(199)?;
                assert_eq!(game.vm.cursor().unwrap().address, expected_pc);
                game.step(0)?;
                assert_eq!(game.vm.cursor().unwrap().address, expected_pc);
                game.step(1)?;
                assert_eq!(game.vm.cursor().unwrap().address, 64);
                assert!(!game.parse_wait);
                assert!(game.wait_ms.is_some());
                pixels(&game, [255; 4]);
            }
        }
        install(&mut game, 0, 0)?;
        game.step(0)?;
        assert_eq!(game.vm.cursor().unwrap().address, 64);
        assert!(!game.parse_wait);
        Ok(())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_draw_ratio_host_clock_curves_staging_and_reset() -> Result<()> {
        let mut game = waiting_game()?;
        for (id, color) in [(212259, [255, 0, 0, 255]), (212260, [0, 0, 255, 255])] {
            let mut object = ObjectState {
                rect: (0, 0, 4, 4),
                ..ObjectState::default()
            };
            object.layers.insert(512, image(color));
            game.objects.insert(id, object);
        }
        game.present_scene()?;
        pixels(&game, [0, 0, 255, 255]);
        let native_curve = Value::Reference(reference("CMimeFlat16LimitOne", 1, 248008045, 37));
        game.message(ratio(212259, 0x100, Value::Null))?;
        game.message(ratio(212260, 0x110, native_curve))?;
        // Merely queuing, or advancing the VM before a trigger, has no effect.
        game.step(100)?;
        pixels(&game, [0, 0, 255, 255]);
        assert_eq!(game.objects[&212259].opacity, 255);
        let pending = game.objects[&212259].pending_draw_ratio.clone();
        assert!(game.message(ratio(212259, 0, Value::Null)).is_err());
        assert_eq!(game.objects[&212259].pending_draw_ratio, pending);
        game.message(trigger(400))?;
        assert!(game.transition.is_none()); // no second, full-frame crossfade
        pixels(&game, [0, 0, 33, 255]);
        game.step(100)?;
        assert_eq!(game.objects[&212259].opacity, 64);
        pixels(&game, [56, 0, 33, 255]);
        game.step(100)?;
        assert_eq!(game.objects[&212260].opacity, 51);
        pixels(&game, [102, 0, 51, 255]);
        game.step(200)?;
        pixels(&game, [0, 0, 255, 255]);
        assert!(game.parse_transition.is_none());
        assert!(
            game.objects
                .values()
                .all(|o| o.pending_draw_ratio.is_none())
        );
        game.objects.get_mut(&212260).unwrap().visible = false;
        game.message(ratio(212259, 0x100, Value::Null))?;
        game.message(trigger(400))?;
        pixels(&game, [0, 0, 0, 255]);
        assert!(game.message(trigger(400)).is_err()); // no silent restart
        game.message(message(
            212259,
            "OM_HumanIn2",
            BTreeMap::from([
                ("スタイル".into(), Value::Integer(512)),
                (
                    "ベース画像".into(),
                    Value::Reference(reference("CBg2d", 6, 13570189, 3577)),
                ),
            ]),
        ))?;
        game.step(200)?;
        pixels(&game, [128, 0, 0, 255]); // staged white replacement is not exposed
        game.step(200)?;
        pixels(&game, [255, 0, 0, 255]);
        game.message(trigger(0))?;
        pixels(&game, [255, 255, 255, 255]);
        game.message(ratio(212259, 0x101, Value::Null))?;
        game.message(trigger(0))?;
        pixels(&game, [0, 0, 0, 255]);
        assert_eq!(game.objects[&212259].opacity, 0);
        // Replacing an active curve switches to pending, so the previous
        // clock no longer samples it. Native completion clears the pending
        // synchronized request as well, preserving the last draw opacity.
        game.message(ratio(212259, 0x100, Value::Null))?;
        game.message(trigger(400))?;
        game.step(100)?;
        pixels(&game, [64, 64, 64, 255]);
        game.objects.get_mut(&212260).unwrap().visible = true;
        game.present_scene()?;
        pixels(&game, [0, 0, 255, 255]);
        game.step(100)?;
        assert_eq!(game.objects[&212259].opacity, 128);
        game.objects.get_mut(&212260).unwrap().visible = false;
        game.present_scene()?;
        pixels(&game, [128, 128, 128, 255]);
        game.message(ratio(212259, 0x101, Value::Null))?;
        game.message(ratio(212260, 0x100, Value::Null))?;
        game.step(1000)?;
        pixels(&game, [128, 128, 128, 255]);
        assert!(game.objects[&212259].pending_draw_ratio.is_none());
        assert!(game.objects[&212260].pending_draw_ratio.is_none());
        assert!(game.parse_transition.is_none());
        assert!(game.wait_ms.is_some()); // all animation ran during VM waits
        Ok(())
    }

    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_camera_parse_clock_zoom_pan_and_opacity_match_inverse_pixel_oracle() -> Result<()> {
        for mode in [4, 8, 12] {
            camera_parse_clock_pixels(mode, 1)?;
        }
        Ok(())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_style_17_camera_uses_real_parse_clock_pixels_and_capture() -> Result<()> {
        for mode in [4, 8, 12] {
            camera_parse_clock_pixels(mode, 0x11)?;
        }
        Ok(())
    }
    fn camera_parse_clock_pixels(mode: i32, style: i32) -> Result<()> {
        let mut game = waiting_game()?;
        let texture = Arc::new(Texture {
            width: 16,
            height: 16,
            display_scale: 1.0,
            rgba: (0..16u8)
                .flat_map(|y| (0..16u8).flat_map(move |x| [x * 15, y * 15, (x ^ y) * 15, 255]))
                .collect(),
        });
        let plane = crate::world::Plane::bitmap(&texture)?;
        let mut object = ObjectState {
            rect: (0, 0, 4, 4),
            ..ObjectState::default()
        };
        object.layers.insert(
            512,
            Layer::World(WorldImage {
                boundary: plane,
                planes: vec![crate::image_manager::WorldImagePlane {
                    texture: Arc::clone(&texture),
                    plane,
                    draw_flags: 0,
                }],
            }),
        );
        game.objects.insert(212259, object);
        game.present_scene()?;
        let call = |screen: bool, fields: BTreeMap<String, Value>| {
            message(
                212259,
                if screen {
                    "OM_SetWorldCameraAsScreen"
                } else {
                    "OM_SetWorldCamera"
                },
                fields,
            )
        };
        // Mid-transition frames carry the retail afterimage of the previous
        // frame; the camera-only state and the final frame stay exact.
        let verify = |game: &Game, trail: Option<(&[u8], u32)>, camera: Camera, opacity: u8| {
            assert_eq!(game.objects[&212259].camera, camera);
            let mut frame = vec![0u8; 64];
            let depth = 720 - i64::from(camera.position[2]);
            let project = |vertex: i32, axis: usize, center: f64| {
                let numerator = ((i64::from(vertex) - i64::from(camera.position[axis])) * 65536
                    - i64::from(camera.angles_q4[axis]) * depth * 8)
                    * i64::from(camera.focal_length);
                center
                    + (((numerator.abs() + depth / 2) / depth) * numerator.signum()) as f64
                        / 65536.0
            };
            let left = project(-8, 0, 2.0);
            let right = project(8, 0, 2.0);
            let top = project(-8, 1, 2.0);
            let bottom = project(8, 1, 2.0);
            for y in 0..4 {
                for x in 0..4 {
                    let u = (x as f64 + 0.5 - left) / (right - left);
                    let v = (y as f64 + 0.5 - top) / (bottom - top);
                    let mut expected = [0, 0, 0, 255];
                    if camera.focal_length != 0
                        && (0.0..1.0).contains(&u)
                        && (0.0..1.0).contains(&v)
                    {
                        let sx = (u * 16.0).floor() as usize;
                        let sy = (v * 16.0).floor() as usize;
                        for (c, out) in expected[..3].iter_mut().enumerate() {
                            *out = ((u32::from(texture.rgba[(sy * 16 + sx) * 4 + c])
                                * u32::from(opacity)
                                + 127)
                                / 255) as u8;
                        }
                    }
                    frame[(y * 4 + x) * 4..(y * 4 + x + 1) * 4].copy_from_slice(&expected);
                }
            }
            if let Some((previous, dt)) = trail.filter(|_| game.parse_transition.is_some()) {
                blend_afterimage(&mut frame, previous, dt);
            }
            assert_eq!(game.frame(), frame, "camera {camera:?}");
        };
        let initial = game.frame().to_vec();
        game.message(call(
            true,
            BTreeMap::from([
                ("ズーム率".into(), Value::Integer(1440)),
                ("フェード方式".into(), Value::Integer(mode)),
                ("スタイル".into(), Value::Integer(style)),
                ("総処理時間基".into(), Value::Integer(10000)),
            ]),
        ))?;
        game.message(call(
            false,
            BTreeMap::from([
                ("ロール角".into(), Value::Integer(1)),
                ("ピッチ角".into(), Value::Integer(-2)),
                ("バンク角".into(), Value::Integer(3)),
                (
                    "フェード方式".into(),
                    Value::Integer(if mode == 4 { 8 } else { 4 }),
                ),
                ("スタイル".into(), Value::Integer(style)),
            ]),
        ))?;
        // Screen/angle commands combine through their prepared target, but
        // neither the picture nor the actual camera advances before a trigger.
        assert_eq!(game.objects[&212259].camera, Camera::default());
        game.step(12345)?;
        assert_eq!(game.frame(), initial);
        game.message(trigger(400))?;
        assert!(game.transition.is_none());
        verify(&game, None, Camera::default(), 255);
        let previous = game.frame().to_vec();
        game.step(100)?;
        verify(
            &game,
            Some((&previous, 100)),
            Camera {
                focal_length: 900,
                angles_q4: [4, -8, 12],
                position: [0; 3],
            },
            255,
        );
        let before = game.frame().to_vec();
        assert!(
            game.message(call(
                true,
                BTreeMap::from([("ズーム率".into(), Value::Integer(2000))])
            ))
            .is_err()
        );
        assert_eq!(game.frame(), before);
        let previous = game.frame().to_vec();
        game.step(100)?;
        verify(
            &game,
            Some((&previous, 100)),
            Camera {
                focal_length: 1080,
                angles_q4: [8, -16, 24],
                position: [0; 3],
            },
            255,
        );
        let previous = game.frame().to_vec();
        game.step(200)?;
        verify(
            &game,
            Some((&previous, 200)),
            Camera {
                focal_length: 1440,
                angles_q4: [16, -32, 48],
                position: [0; 3],
            },
            255,
        );
        assert_eq!(game.objects[&212259].camera_center_y.get(), 7);
        assert!(game.parse_transition.is_none());
        assert!(game.objects[&212259].pending_camera.is_none());
        assert!(game.objects[&212259].camera_start.is_none());

        game.message(call(false, BTreeMap::new()))?;
        game.message(call(
            true,
            BTreeMap::from([
                ("ズーム率".into(), Value::Integer(0)),
                ("スタイル".into(), Value::Integer(0x800)),
            ]),
        ))?;
        game.message(call(
            true,
            BTreeMap::from([
                ("ズーム率".into(), Value::Integer(720)),
                ("フェード方式".into(), Value::Integer(mode)),
                ("スタイル".into(), Value::Integer(style)),
            ]),
        ))?;
        game.message(ratio(212259, 0x100, Value::Null))?;
        game.message(trigger(400))?;
        verify(
            &game,
            None,
            Camera {
                focal_length: 0,
                angles_q4: [0; 3],
                position: [0; 3],
            },
            0,
        );
        let previous = game.frame().to_vec();
        game.step(100)?;
        verify(
            &game,
            Some((&previous, 100)),
            Camera {
                focal_length: 180,
                angles_q4: [0; 3],
                position: [0; 3],
            },
            64,
        );
        game.step(300)?;
        verify(&game, None, Camera::default(), 255);
        assert!(game.objects[&212259].camera_start.is_none());
        // Capture after preparation overwrites the start while preserving
        // its delta; an angular restage then uses the resulting zoom target.
        game.message(call(
            true,
            BTreeMap::from([
                ("ズーム率".into(), Value::Integer(1000)),
                ("フェード方式".into(), Value::Integer(mode)),
                ("スタイル".into(), Value::Integer(style)),
            ]),
        ))?;
        game.message(call(
            true,
            BTreeMap::from([
                ("ズーム率".into(), Value::Integer(800)),
                ("スタイル".into(), Value::Integer(0x800)),
            ]),
        ))?;
        assert_eq!(
            game.objects[&212259]
                .pending_camera
                .unwrap()
                .target()
                .focal_length,
            1080
        );
        game.message(call(
            false,
            BTreeMap::from([
                ("ロール角".into(), Value::Integer(1)),
                ("フェード方式".into(), Value::Integer(mode)),
                ("スタイル".into(), Value::Integer(style)),
            ]),
        ))?;
        assert_eq!(
            game.objects[&212259]
                .pending_camera
                .unwrap()
                .start()
                .focal_length,
            800
        );
        game.message(trigger(0))?;
        verify(
            &game,
            None,
            Camera {
                focal_length: 1080,
                angles_q4: [16, 0, 0],
                position: [0; 3],
            },
            255,
        );
        game.message(call(false, BTreeMap::new()))?;
        game.message(call(true, BTreeMap::new()))?;
        game.message(call(
            false,
            BTreeMap::from([
                ("３ＤＸ".into(), Value::Integer(3)),
                ("３ＤＹ".into(), Value::Integer(-1)),
                ("３ＤＺ".into(), Value::Integer(40)),
                ("フェード方式".into(), Value::Integer(mode)),
                ("スタイル".into(), Value::Integer(style)),
            ]),
        ))?;
        game.message(call(
            true,
            BTreeMap::from([
                ("ズーム率".into(), Value::Integer(1440)),
                ("フェード方式".into(), Value::Integer(mode)),
                ("スタイル".into(), Value::Integer(style)),
            ]),
        ))?;
        assert_eq!(game.objects[&212259].camera, Camera::default());
        game.message(trigger(400))?;
        let previous = game.frame().to_vec();
        game.step(200)?;
        verify(
            &game,
            Some((&previous, 200)),
            Camera {
                focal_length: 1080,
                position: [1, -1, 20],
                angles_q4: [0; 3],
            },
            255,
        );
        let previous = game.frame().to_vec();
        game.step(200)?;
        verify(
            &game,
            Some((&previous, 200)),
            Camera {
                focal_length: 1440,
                position: [3, -1, 40],
                angles_q4: [0; 3],
            },
            255,
        );
        game.message(call(
            true,
            BTreeMap::from([
                ("ズーム率".into(), Value::Integer(720)),
                ("スタイル".into(), Value::Integer(0x800)),
            ]),
        ))?;
        assert_eq!(
            game.objects[&212259].camera_start.unwrap().position,
            [3, -1, 40]
        );
        // A target whose uncorrected bounds miss the viewport is corrected,
        // not rejected. Native integer angular bounds are not equivalent to
        // forcing the projected rectangle to cover every fractional edge.
        game.message(call(false, BTreeMap::new()))?;
        game.message(call(true, BTreeMap::new()))?;
        game.message(call(
            false,
            BTreeMap::from([
                ("３ＤＸ".into(), Value::Integer(7)),
                ("３ＤＹ".into(), Value::Integer(7)),
            ]),
        ))?;
        assert_eq!(game.objects[&212259].camera.angles_q4, [-11, -11, 0]);
        game.message(trigger(0))?;
        verify(
            &game,
            None,
            Camera {
                focal_length: 720,
                position: [7, 7, 0],
                angles_q4: [-11, -11, 0],
            },
            255,
        );
        assert_eq!(game.objects[&212259].camera_center_y.get(), 2);
        assert!(game.wait_ms.is_some());
        Ok(())
    }
}
