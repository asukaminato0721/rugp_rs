//! ParseScreen-driven opacity, separate from framebuffer CFade effects.
use super::*;
use rugp_assets::mime::MimeCurve;

pub(super) struct ParseTransition {
    pub(super) scene: BTreeMap<u64, ObjectState>,
    curves: BTreeMap<u64, MimeCurve>,
    elapsed: u32,
    duration: u32,
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
    pub(super) fn start_parse_draw_ratios(&mut self, duration: u32) -> Result<bool> {
        ensure!(
            self.parse_transition.is_none(),
            "overlapping ParseScreen transitions are not implemented yet"
        );
        if !self
            .objects
            .values()
            .any(|o| o.pending_draw_ratio.is_some())
        {
            return Ok(false);
        }
        let curves: BTreeMap<_, _> = self
            .objects
            .iter_mut()
            .filter_map(|(&id, o)| o.pending_draw_ratio.take().map(|c| (id, c)))
            .collect();
        self.parse_transition = Some(ParseTransition {
            scene: self.objects.clone(),
            curves,
            elapsed: 0,
            duration,
        });
        self.transition = None;
        self.advance_parse_draw_ratios(0)?;
        Ok(true)
    }
    pub(super) fn advance_parse_draw_ratios(&mut self, dt_ms: u32) -> Result<()> {
        let size = self.size();
        let Some(parse) = &mut self.parse_transition else {
            return Ok(());
        };
        parse.elapsed = parse.elapsed.saturating_add(dt_ms).min(parse.duration);
        let progress = progress_q16(parse.elapsed, parse.duration);
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
        let next = Self::render_objects(&parse.scene, size)?;
        let finished = parse.elapsed == parse.duration;
        self.frame = next;
        // Native 10047170 releases synchronized curve state, not opacity.
        if finished {
            for id in parse.scene.keys() {
                if let Some(object) = self.objects.get_mut(id) {
                    object.pending_draw_ratio = None;
                }
            }
            self.parse_transition = None;
        }
        Ok(())
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
    fn trigger(duration: i32) -> Message {
        message(
            212259,
            "OM_ParseScreenTrigger",
            BTreeMap::from([("総処理時間基".into(), Value::Integer(duration))]),
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
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_draw_ratio_host_clock_curves_staging_and_reset() -> Result<()> {
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
}
