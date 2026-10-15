use super::*;
use rugp_assets::object::ClassId;

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

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_textured_model_fallback_uses_declared_base_pixels_and_stages_placement() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
    let mut game = Game::open(root.as_ref())?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let scene = rugp_scene_vm::Scenario::decode_with_schema(
        &rugp_assets::script::Script::load(&game.archive, 474)?,
        game.archive.address_shift,
        Some(&native),
    )?;
    let mut models = Vec::new();
    scene.visit_references(&mut |r| {
        if r.class
            .as_ref()
            .is_some_and(|c| c.name == "CDcAgesModelQsT")
        {
            models.push(r.clone());
        }
    });
    assert_eq!(models.len(), 6);
    let target = reference("CParseBgBox", 15, 212259, 118);
    game.objects.insert(
        212259,
        ObjectState {
            rect: (0, 0, 1024, 600),
            ..ObjectState::default()
        },
    );
    let mut drew_pixels = false;
    for model in models {
        let base = model
            .inline
            .as_ref()
            .context("inline model missing")?
            .references
            .last()
            .unwrap();
        let expected = game.images.load_world(base)?;
        let actual = game.images.load_world(&model)?;
        assert_eq!(actual.planes.len(), expected.planes.len());
        for (a, b) in actual.planes.iter().zip(&expected.planes) {
            // Inline CBg2d bitmaps are decoded afresh rather than interned in
            // the stored-image cache, so compare pixels, not allocation identity.
            assert_eq!(
                (a.texture.width, a.texture.height, a.texture.display_scale),
                (b.texture.width, b.texture.height, b.texture.display_scale)
            );
            assert_eq!(a.texture.rgba, b.texture.rgba);
            assert_eq!(
                a.plane.quad(&Camera::default(), (0, 0, 1024, 600))?,
                b.plane.quad(&Camera::default(), (0, 0, 1024, 600))?
            );
        }
        game.frame = Framebuffer::new(1024, 600)?;
        let before = game.frame().to_vec();
        game.message(Message {
            class: "OM_HumanIn2".into(),
            target: Value::Reference(target.clone()),
            fields: BTreeMap::from([
                ("ベース画像".into(), Value::Reference(model)),
                ("３ＤＹ".into(), Value::Integer(-1440)),
                ("３ＤＺ".into(), Value::Integer(10000)),
                ("スタイル".into(), Value::Integer(0)),
            ]),
        })?;
        assert_eq!(game.frame(), before);
        let object = &game.objects[&212259];
        assert_eq!(object.camera, Camera::default());
        assert!(matches!(
            object.properties["render.model.３ＤＹ"],
            Value::Integer(-1440)
        ));
        let mut oracle = ObjectState {
            rect: object.rect,
            ..ObjectState::default()
        };
        oracle.layers.insert(0, Layer::World(expected));
        let mut pixels = Framebuffer::new(1024, 600)?;
        oracle.render(&mut pixels)?;
        game.present_scene()?;
        assert_eq!(game.frame(), pixels.rgba);
        drew_pixels |= game
            .frame()
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[..3] != [0; 3]);
    }
    assert!(
        drew_pixels,
        "the model fallback must draw actual archive pixels"
    );
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_extended_background_host_group_matches_independent_pixel_oracle() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let mut game = Game::open(root.as_ref())?;
    for enabled in [false, true] {
        game.message(Message {
            class: "OM_SetUucSaveSwitch".into(),
            target: Value::Null,
            fields: BTreeMap::from([("フラグ".into(), Value::Integer(i32::from(enabled)))]),
        })?;
        assert_eq!(game.script_save_enabled(), enabled);
    }
    let target = reference("CParseBgBox", 15, 212259, 118);
    let bg = reference("CBg2d", 6, 251819850, 3018612);
    let scene = rugp_assets::background::SceneBackground::parse(
        &game.archive.read_at(251819850, 3018612)?,
        6,
        game.archive.address_shift,
    )?;
    assert!(scene.background.planes.is_empty());
    assert_eq!(scene.layers.len(), 4);
    assert_eq!(
        scene
            .layers
            .iter()
            .map(|l| l.position[2])
            .collect::<Vec<_>>(),
        [18000, 7500, 2600, 1180]
    );
    let call = |name: &str, fields: BTreeMap<String, Value>| Message {
        class: name.into(),
        target: Value::Reference(target.clone()),
        fields,
    };
    game.message(call(
        "OM_SetWorldCameraAsScreen",
        BTreeMap::from([
            ("ズーム率".into(), Value::Integer(1752)),
            ("スタイル".into(), Value::Integer(1)),
        ]),
    ))?;
    let before = game.frame().to_vec();
    let camera_before = game.objects[&212259].camera;
    game.message(call(
        "OM_SetWorldCameraAsScreen",
        BTreeMap::from([
            ("スタイル".into(), Value::Integer(0x800)),
            ("総処理時間基".into(), Value::Integer(400)),
            ("ズーム率".into(), Value::Integer(0)),
        ]),
    ))?;
    assert_eq!(game.objects[&212259].camera, camera_before);
    assert_eq!(game.objects[&212259].camera_start.unwrap().focal_length, 0);
    assert_eq!(game.frame(), before);
    game.message(call(
        "OM_SetWorldCamera",
        BTreeMap::from([
            ("スタイル".into(), Value::Integer(0x800)),
            ("総処理時間基".into(), Value::Integer(400)),
            ("ロール角".into(), Value::Integer(1)),
            ("ピッチ角".into(), Value::Integer(-2)),
            ("バンク角".into(), Value::Integer(3)),
        ]),
    ))?;
    assert_eq!(game.objects[&212259].camera, camera_before);
    let captured = game.objects[&212259].camera_start.unwrap();
    assert_eq!(captured.angles_q4, [16, -32, 48]);
    assert_eq!(game.frame(), before);
    // Unsupported capture/transition combinations fail without replacing
    // either the visible camera or the already staged start.
    let rejected = game.message(call(
        "OM_SetWorldCameraAsScreen",
        BTreeMap::from([
            ("スタイル".into(), Value::Integer(0x800)),
            ("フェード方式".into(), Value::Integer(16)),
            ("ズーム率".into(), Value::Integer(2000)),
        ]),
    ));
    assert!(rejected.is_err());
    assert_eq!(game.objects[&212259].camera, camera_before);
    assert_eq!(game.objects[&212259].camera_start, Some(captured));
    game.message(call(
        "OM_HumanIn2",
        BTreeMap::from([
            ("スタイル".into(), Value::Integer(512)),
            ("ベース画像".into(), Value::Reference(bg.clone())),
        ]),
    ))?;
    assert_eq!(game.frame(), before); // still staged until scene presentation
    let Layer::World(image) = &game.objects[&212259].layers[&512] else {
        panic!("not a world group")
    };
    assert_eq!(image.planes.len(), 4);
    assert_eq!(
        image
            .planes
            .iter()
            .map(|p| p.draw_flags)
            .collect::<Vec<_>>(),
        [17, 19, 19, 19]
    );
    assert!(
        game.images
            .load(&bg)
            .err()
            .expect("extended world image must reject flat loading")
            .to_string()
            .contains("cannot be loaded as a flat texture")
    );
    let decoded: Vec<_> = scene
        .layers
        .iter()
        .map(|l| Texture::from_image(l.image.inline.as_ref().unwrap().decode_bitmap()?))
        .collect::<Result<_>>()?;
    let mut native_initial = None;
    for all_visible in [false, true] {
        let mut untranslated = None;
        // Test the native initial hidden state, then exercise every layer's
        // geometry/compositing by explicitly exposing them in test state.
        // This is not an implementation of a native layer-activation message.
        if all_visible {
            let Layer::World(image) = game
                .objects
                .get_mut(&212259)
                .unwrap()
                .layers
                .get_mut(&512)
                .unwrap()
            else {
                unreachable!()
            };
            for plane in &mut image.planes {
                plane.draw_flags &= !2;
            }
        }
        for (pitch, roll, position) in [(-2, 0, [0; 3]), (2, 1, [0; 3]), (2, 1, [100, -50, 100])] {
            // The small nearest layer does not cover the viewport; camera
            // validation must use the group's background boundary, not each quad.
            game.message(call(
                "OM_SetWorldCamera",
                BTreeMap::from([
                    ("ピッチ角".into(), Value::Integer(pitch)),
                    ("ロール角".into(), Value::Integer(roll)),
                    ("スタイル".into(), Value::Integer(11)),
                    ("３ＤＸ".into(), Value::Integer(position[0])),
                    ("３ＤＹ".into(), Value::Integer(position[1])),
                    ("３ＤＺ".into(), Value::Integer(position[2])),
                ]),
            ))?;
            game.message(call("OM_ParseScreenTrigger", BTreeMap::new()))?;
            let mut expected = vec![0u8; 1024 * 600 * 4];
            for p in expected.as_chunks_mut::<4>().0 {
                p[3] = 255;
            }
            for (layer, texture) in scene.layers.iter().zip(&decoded) {
                if !all_visible && layer.parameters[2] & 2 != 0 {
                    continue;
                }
                // Native edge construction + signed MulDiv Q16 rounding,
                // independently implemented without Plane or the compositor.
                let raw_depth = f64::from(layer.position[2]);
                let factor = raw_depth * f64::from(scene.scale) * 0.0015999999999999999;
                // Only the far retail layer is camera-relative; exposed
                // foreground layers translate at their independent depths.
                let camera_position = if layer.attributes[1] == 1 {
                    [0; 3]
                } else {
                    position
                };
                let z = raw_depth - f64::from(camera_position[2]);
                let start = [
                    f64::from(layer.anchor[0]) - 1400.0,
                    f64::from(layer.anchor[1]) - 460.0,
                ];
                let mut edges = [[0.0; 2]; 2];
                for (axis, axis_edges) in edges.iter_mut().enumerate() {
                    for (edge, out) in axis_edges.iter_mut().enumerate() {
                        let length = if axis == 0 {
                            texture.width
                        } else {
                            texture.height
                        };
                        let vertex = ((start[axis] + f64::from(length) * edge as f64) * factor
                            + f64::from(layer.position[axis]))
                        .trunc()
                            - f64::from(camera_position[axis]);
                        let angle = if axis == 0 { roll } else { pitch };
                        *out = [512.0, 300.0][axis]
                            + ((vertex * 65536.0 - f64::from(angle * 16) * z * 8.0) * 1752.0 / z)
                                .round()
                                / 65536.0;
                    }
                }
                for y in 0..600 {
                    for x in 0..1024 {
                        let u = (x as f64 + 0.5 - edges[0][0]) / (edges[0][1] - edges[0][0]);
                        let v = (y as f64 + 0.5 - edges[1][0]) / (edges[1][1] - edges[1][0]);
                        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                            continue;
                        }
                        let sx = (u * f64::from(texture.width)).floor() as usize;
                        let sy = (v * f64::from(texture.height)).floor() as usize;
                        let si = (sy * texture.width as usize + sx) * 4;
                        let di = (y * 1024 + x) * 4;
                        let alpha = u32::from(texture.rgba[si + 3]);
                        for c in 0..3 {
                            expected[di + c] = ((u32::from(texture.rgba[si + c]) * alpha
                                + u32::from(expected[di + c]) * (255 - alpha)
                                + 127)
                                / 255) as u8;
                        }
                    }
                }
            }
            assert_eq!(game.frame().len(), expected.len());
            assert!(
                game.frame().iter().zip(&expected).all(|(a, b)| a == b),
                "multiplane frame differs at pitch {pitch}, roll {roll}"
            );
            if pitch == 2 {
                if position == [0; 3] {
                    untranslated = Some(game.frame().to_vec());
                } else if all_visible {
                    assert_ne!(game.frame(), untranslated.as_ref().unwrap());
                } else {
                    assert_eq!(game.frame(), untranslated.as_ref().unwrap());
                }
            }
            if pitch == -2 {
                if all_visible {
                    assert_ne!(game.frame(), native_initial.as_ref().unwrap());
                } else {
                    native_initial = Some(game.frame().to_vec());
                }
            }
        }
    }
    // A subsequent native replacement replaces the entire group, preserving
    // per-slot sprite-pass flags and never leaving stale foreground planes.
    game.objects
        .get_mut(&212259)
        .unwrap()
        .world_layers
        .set_sprite_pass_suppressed(3, true);
    game.message(call(
        "OM_HumanIn2",
        BTreeMap::from([
            ("スタイル".into(), Value::Integer(512)),
            (
                "ベース画像".into(),
                Value::Reference(reference("CBg2d", 6, 13570189, 3577)),
            ),
        ]),
    ))?;
    let object = &game.objects[&212259];
    let Layer::World(image) = &object.layers[&512] else {
        panic!("not a world group")
    };
    assert_eq!(image.planes.len(), 1);
    assert!(!object.world_layers.draws_in_sprite_pass(3));
    let mut expected = Framebuffer::new(1024, 600)?;
    object.render(&mut expected)?;
    let destination = reference("CImgAreaBox", 15, 212650, 91);
    game.objects.insert(
        212650,
        ObjectState {
            visible: false,
            ..ObjectState::default()
        },
    );
    let before_build = game.frame().to_vec();
    game.message(call(
        "OM_ParseImageBuild",
        BTreeMap::from([(
            "対象ウインドウ".into(),
            Value::Reference(destination.clone()),
        )]),
    ))?;
    assert_eq!(game.buffers[&212650].rgba, expected.rgba);
    assert_eq!(game.frame(), before_build); // construction alone is not presentation
    game.message(call("OM_UpdateAllView", BTreeMap::new()))?;
    assert_eq!(game.frame(), expected.rgba);
    game.message(call(
        "OM_VramAgesImageIn",
        BTreeMap::from([
            ("画像ソースウインドウ".into(), Value::Reference(destination)),
            ("スタイル".into(), Value::Integer(512)),
        ]),
    ))?;
    assert!(matches!(
        game.objects[&212259].layers[&512],
        Layer::Screen(_)
    ));
    game.message(call("OM_SetWorldCamera", BTreeMap::new()))?;
    game.message(call("OM_ParseScreenTrigger_AllHumanOut", BTreeMap::new()))?;
    game.message(call("OM_UpdateAllView", BTreeMap::new()))?;
    assert_eq!(game.frame(), expected.rgba);
    game.objects.insert(
        900,
        ObjectState {
            parent: Some(212259),
            ..ObjectState::default()
        },
    );
    for visible in [false, true] {
        game.message(call(
            "OM_ChangeAllBoxVisibility",
            BTreeMap::from([("可視".into(), Value::Integer(i32::from(visible)))]),
        ))?;
        assert_eq!(game.objects[&900].visible, visible);
        assert!(game.objects[&212259].visible);
        assert!(!game.objects[&212650].visible);
    }
    Ok(())
}
