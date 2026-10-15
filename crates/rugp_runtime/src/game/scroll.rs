//! Portable image-area scroll commands; content panning is approximated.
use super::*;

impl Game {
    pub(super) fn retain_mode_one_body_scroll(
        &mut self,
        target: &ObjectRef,
        fields: BTreeMap<String, Value>,
    ) -> Result<()> {
        ensure!(
            target
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CImgAreaBox" && c.schema == 15),
            "unsupported asynchronous body-scroll window"
        );
        let attributes = number(&fields, "同期属性", 0)?;
        let time = number(&fields, "時間基", 0)?;
        let x = number(&fields, "水平距離", 0)?;
        let y = number(&fields, "垂直距離", 0)?;
        let reset = attributes == 0 && time == 0 && x == 0 && y == 0;
        ensure!(
            number(&fields, "モード", 0)? == 1 && ((attributes == 0x8000 && time > 0) || reset),
            "unsupported asynchronous body-scroll mode/attributes"
        );
        let id = target
            .offset
            .context("body-scroll window identity missing")?;
        let object = self
            .objects
            .get_mut(&id)
            .context("body-scroll window is not initialized")?;
        // rvmm 10013740 -> 10013920 registers an independent scroll actor.
        // 10013aea skips the modal wait for attributes 0x8000. There is no
        // script output handle in this packet. Playback-first presentation
        // keeps the whole image, as for mode 0; preserve the request without
        // moving the window/camera or replacing any script/media wait clock.
        // Zero-time/distance reset follows 1001394a..100139ca: settle/stop
        // the old motion rather than starting another independent actor.
        object.properties.insert(
            "render.body_scroll.active".into(),
            Value::Integer(i32::from(!reset)),
        );
        for (name, value) in fields {
            object
                .properties
                .insert(format!("render.body_scroll.{name}"), value);
        }
        Ok(())
    }
}

pub(super) fn apply(
    object: &mut ObjectState,
    class: &str,
    fields: BTreeMap<String, Value>,
) -> Result<()> {
    let x = number(&fields, "水平距離", 0)?;
    let y = number(&fields, "垂直距離", 0)?;
    let duration = number(&fields, "時間基", 0)?;
    ensure!(
        number(&fields, "モード", 0)? == 0 && number(&fields, "同期属性", 0)? == 0,
        "unsupported scroll mode/synchronization attributes"
    );
    ensure!(duration >= 0, "negative scroll duration");
    let prefix = match class {
        "OM_BodyScroll" => {
            // Helper 99 configures/resets an independent content origin before
            // reuse. Retain it, but show the whole portable source surface
            // instead of cropping/panning within its native body viewport.
            "render.body_scroll"
        }
        "OM_BoxScroll" => {
            // Approximate helper 99's absolute repositioning with an instant
            // endpoint. Its separate blocking CVmBlt retains the transition
            // duration; this command does not present or change any VM wait.
            object.rect.0 = x;
            object.rect.1 = y;
            "render.box_scroll"
        }
        _ => bail!("unsupported scroll command {class}"),
    };
    for (key, value) in fields {
        object.properties.insert(format!("{prefix}.{key}"), value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_async_body_pan_retains_its_packet_and_preserves_pixels_and_waits() -> Result<()> {
        use rugp_assets::{object::ClassId, script::Script};
        use rugp_scene_vm::Scenario;
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let scene = Scenario::decode_with_schema(
            &Script::load(&game.archive, 244)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let mut instruction = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x8348)
            .context("ending body-pan packet")?
            .clone();
        let message = game.vm.message(&instruction)?;
        let Value::Reference(target) = &message.target else {
            bail!("body-pan target missing")
        };
        let id = target.offset.unwrap();
        let target = target.clone();
        let fields = message.fields.clone();
        let texture = Arc::new(Texture {
            width: 2,
            height: 2,
            display_scale: 1.0,
            rgba: [255, 0, 0, 255].repeat(4),
        });
        let camera = Camera {
            position: [3, 4, 5],
            ..Camera::default()
        };
        let pending = crate::world::CameraTween::new(camera, Camera::default(), 0)?;
        game.frame = Framebuffer::new(4, 4)?;
        game.objects.insert(
            id,
            ObjectState {
                rect: (1, 1, 2, 2),
                camera,
                pending_camera: Some(pending),
                layers: BTreeMap::from([(0, Layer::Screen(Arc::clone(&texture)))]),
                ..ObjectState::default()
            },
        );
        game.present_scene()?;
        let before = game.frame().to_vec();
        game.wait_ms = Some(500);
        game.message(message)?;
        assert_eq!(game.frame(), before);
        assert_eq!(game.wait_ms, Some(500));
        let object = &game.objects[&id];
        assert_eq!(object.rect, (1, 1, 2, 2));
        assert_eq!(object.camera, camera);
        assert_eq!(object.pending_camera, Some(pending));
        let Layer::Screen(stored) = &object.layers[&0] else {
            bail!("body image replaced")
        };
        assert!(Arc::ptr_eq(stored, &texture));
        for (name, value) in [
            ("水平距離", 2500),
            ("垂直距離", 0),
            ("時間基", 100000),
            ("同期属性", 0x8000),
            ("モード", 1),
        ] {
            assert_eq!(
                number(
                    &object.properties,
                    &format!("render.body_scroll.{name}"),
                    -1
                )?,
                value
            );
        }
        let mut invalid = fields;
        invalid.insert("同期属性".into(), Value::Integer(0));
        assert!(game.retain_mode_one_body_scroll(&target, invalid).is_err());
        // Host acknowledgement must not turn this async request into a wait.
        instruction.address = 0;
        instruction.next_address = 28;
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 40,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    instruction,
                    Instruction {
                        index: 1,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmRet".into(),
                            schema: 17,
                        },
                        address: 28,
                        next_address: u32::MAX,
                        flags: 0,
                        arguments: vec![],
                    },
                ],
            },
        )?;
        game.vm = Vm::new(Arc::new(program), 1)?;
        game.wait_ms = None;
        assert_eq!(game.step(0)?, RunState::Ended);
        assert!(game.pending_sounds().is_empty());
        let reset = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x88ec)
            .context("ending body-pan reset")?;
        let reset = game.vm.message(reset)?;
        game.wait_ms = Some(700);
        game.message(reset)?;
        assert_eq!(game.wait_ms, Some(700));
        let object = &game.objects[&id];
        assert_eq!(
            number(&object.properties, "render.body_scroll.active", -1)?,
            0
        );
        assert_eq!(
            number(&object.properties, "render.body_scroll.時間基", -1)?,
            0
        );
        assert_eq!(object.camera, camera);
        assert_eq!(object.pending_camera, Some(pending));
        assert_eq!(object.rect, (1, 1, 2, 2));
        assert_eq!(game.frame(), before);
        Ok(())
    }

    fn fields(x: i32, y: i32, duration: i32) -> BTreeMap<String, Value> {
        BTreeMap::from([
            ("水平距離".into(), Value::Integer(x)),
            ("垂直距離".into(), Value::Integer(y)),
            ("時間基".into(), Value::Integer(duration)),
        ])
    }
    #[test]
    fn body_reset_and_box_endpoint_are_distinct_and_preserve_dimensions() -> Result<()> {
        let mut object = ObjectState {
            rect: (11, 12, 40, 30),
            ..ObjectState::default()
        };
        apply(&mut object, "OM_BodyScroll", fields(0, 0, 0))?;
        assert_eq!(object.rect, (11, 12, 40, 30));
        apply(&mut object, "OM_BodyScroll", fields(100, -20, 400))?;
        assert_eq!(object.rect, (11, 12, 40, 30));
        assert!(matches!(
            object.properties["render.body_scroll.水平距離"],
            Value::Integer(100)
        ));
        apply(&mut object, "OM_BoxScroll", fields(1024, -50, 1000))?;
        assert_eq!(object.rect, (1024, -50, 40, 30));
        assert!(matches!(
            object.properties["render.box_scroll.時間基"],
            Value::Integer(1000)
        ));
        apply(&mut object, "OM_BoxScroll", fields(0, 0, 0))?;
        assert_eq!(object.rect, (0, 0, 40, 30)); // absolute, not relative
        assert_eq!(object.camera, Camera::default());
        assert!(object.visible && object.layers.is_empty());
        Ok(())
    }
    #[test]
    fn unsupported_scrolls_do_not_partially_move_the_surface() {
        let mut object = ObjectState {
            rect: (11, 12, 40, 30),
            ..ObjectState::default()
        };
        for (class, packet) in [
            ("OM_BodyScroll", fields(1, 0, -1)),
            ("OM_BoxScroll", fields(100, 100, -1)),
            ("other", fields(100, 100, 0)),
            (
                "OM_BoxScroll",
                BTreeMap::from([("水平距離".into(), Value::Null)]),
            ),
            (
                "OM_BoxScroll",
                BTreeMap::from([("モード".into(), Value::Integer(1))]),
            ),
            (
                "OM_BoxScroll",
                BTreeMap::from([("同期属性".into(), Value::Integer(1))]),
            ),
        ] {
            assert!(apply(&mut object, class, packet).is_err());
            assert_eq!(object.rect, (11, 12, 40, 30));
            assert!(object.properties.is_empty());
        }
    }
    #[test]
    fn box_endpoint_moves_real_pixels_and_body_pan_remains_a_documented_approximation() -> Result<()>
    {
        let mut object = ObjectState {
            rect: (0, 0, 2, 2),
            ..ObjectState::default()
        };
        object.layers.insert(
            0,
            Layer::Screen(Arc::new(Texture {
                width: 2,
                height: 2,
                display_scale: 1.0,
                rgba: [255, 0, 0, 255].repeat(4),
            })),
        );
        let render = |object: &ObjectState| -> Result<Framebuffer> {
            let mut frame = Framebuffer::new(4, 4)?;
            object.render(&mut frame)?;
            Ok(frame)
        };
        let before = render(&object)?;
        apply(&mut object, "OM_BodyScroll", fields(100, -20, 400))?;
        assert_eq!(render(&object)?.rgba, before.rgba);
        apply(&mut object, "OM_BoxScroll", fields(2, 2, 400))?;
        let after = render(&object)?;
        assert_eq!(&before.rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(&after.rgba[..4], &[0, 0, 0, 255]);
        assert_eq!(&after.rgba[40..44], &[255, 0, 0, 255]);
        Ok(())
    }
}
