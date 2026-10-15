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
fn retail_extended_background_host_group_matches_independent_pixel_oracle() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let mut game = Game::open(root.as_ref())?;
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
        BTreeMap::from([("ズーム率".into(), Value::Integer(1752))]),
    ))?;
    let before = game.frame().to_vec();
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
        for (pitch, roll) in [(-2, 0), (2, 1)] {
            // The small nearest layer does not cover the viewport; camera
            // validation must use the group's background boundary, not each quad.
            game.message(call(
                "OM_SetWorldCamera",
                BTreeMap::from([
                    ("ピッチ角".into(), Value::Integer(pitch)),
                    ("ロール角".into(), Value::Integer(roll)),
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
                let z = f64::from(layer.position[2]);
                let factor = z * f64::from(scene.scale) * 0.0015999999999999999;
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
                        .trunc();
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
    Ok(())
}
