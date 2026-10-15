//! Native class-specific OM_SetVarData dispatch. It is not a key/value store.
use super::*;

impl Game {
    pub(super) fn set_var_data(
        &mut self,
        target: &ObjectRef,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let class = target
            .class
            .as_ref()
            .context("variable-data target class missing")?;
        let key = fields
            .get("識別値")
            .context("variable-data identifier missing")?
            .integer()?;
        if (matches!(class.name.as_str(), "CCharBox" | "CCharBox2") && key == 100)
            || class.name == "CLayerSimpleFlush"
        {
            // Preserve opaque sprite/layer rendering parameters. The current
            // static presentation does not reproduce the native material.
            let data = fields
                .get("データ")
                .context("render variable-data payload missing")?;
            self.objects
                .entry(target.offset.context("render object identity missing")?)
                .or_default()
                .properties
                .insert(format!("render.var.{key}"), data.clone());
            return Ok(());
        }
        ensure!(
            class.name == "CParseBgBox",
            "variable-data target {} is not implemented yet",
            class.name
        );
        ensure!(
            [41, 128, 129].contains(&key),
            "CParseBgBox variable-data identifier {key} is not implemented yet"
        );
        let data = fields
            .get("データ")
            .context("variable-data payload missing")?;
        if key == 41 {
            // Vm60 100485d1 clears native queued layer updates/callbacks for
            // integer zero. The full-scene renderer has no such cached queue;
            // retain the packet without clearing the logical scene/motions.
            self.objects
                .entry(target.offset.context("render object identity missing")?)
                .or_default()
                .properties
                .insert("render.var.41".into(), data.clone());
            return Ok(());
        }
        // Native CBasicValue::GetInteger (UnivUI 100230c0) tests the immediate
        // integer tag only. It does not coerce text/reals or dereference data.
        if let Value::Integer(slot) = data {
            let id = target
                .offset
                .context("variable-data target identity missing")?;
            self.objects
                .entry(id)
                .or_default()
                .world_layers
                .set_sprite_pass_suppressed(*slot, key == 128);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_masks_accumulate_clear_independently_and_do_not_disable_backgrounds() {
        let mut object = ObjectState::default();
        for slot in [1, 2, 3, 7] {
            object.world_layers.set_sprite_pass_suppressed(slot, true);
        }
        for slot in 0..8 {
            assert_eq!(
                object.world_layers.draws_in_sprite_pass(slot),
                ![1, 2, 3, 7].contains(&slot)
            );
        }
        object.world_layers.set_sprite_pass_suppressed(2, false);
        assert!(object.world_layers.draws_in_sprite_pass(2));
        assert!(!object.world_layers.draws_in_sprite_pass(1));
        let before = object.world_layers;
        for invalid in [i32::MIN, -1, 0, 8, i32::MAX] {
            object
                .world_layers
                .set_sprite_pass_suppressed(invalid, true);
            object
                .world_layers
                .set_sprite_pass_suppressed(invalid, false);
        }
        assert_eq!(object.world_layers, before);
        assert!(object.visible);
        // Native static-plane replacement does not clear the suppression bit.
        object.layers.clear();
        assert_eq!(object.world_layers, before);
        assert!(!object.world_layers.draws_in_sprite_pass(255));
    }

    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_var_data_dispatch_preserves_immediate_integer_tags_and_background_pass() -> Result<()>
    {
        use rugp_assets::object::ClassId;
        let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
        let mut game = Game::open(root.as_ref())?;
        let target = ObjectRef {
            class: Some(ClassId {
                name: "CParseBgBox".into(),
                schema: 15,
            }),
            cache_index: None,
            flags: 0xc108,
            offset: Some(212259),
            size: Some(118),
            owner: None,
            selector: 0,
            inline: None,
        };
        let message = |key, data| Message {
            class: "OM_SetVarData".into(),
            target: Value::Reference(target.clone()),
            fields: BTreeMap::from([
                ("識別値".into(), Value::Integer(key)),
                ("データ".into(), data),
            ]),
        };
        let image = Texture {
            width: 4,
            height: 4,
            display_scale: 1.0,
            rgba: [53, 91, 183, 255].repeat(16),
        };
        let plane = crate::world::Plane::bitmap(&image)?;
        let mut object = ObjectState {
            rect: (0, 0, 4, 4),
            ..ObjectState::default()
        };
        object.layers.insert(
            512,
            Layer::World(crate::image_manager::WorldImage {
                planes: vec![crate::image_manager::WorldImagePlane {
                    texture: Arc::new(image),
                    plane,
                    draw_flags: 0,
                }],
                boundary: plane,
            }),
        );
        game.objects.insert(212259, object);
        game.present_scene()?;
        let pixels = game.frame().to_vec();
        for slot in [1, 2, 3] {
            game.message(message(128, Value::Integer(slot)))?;
        }
        assert!(game.objects[&212259].world_layers.draws_in_sprite_pass(0));
        for slot in [1, 2, 3] {
            assert!(
                !game.objects[&212259]
                    .world_layers
                    .draws_in_sprite_pass(slot)
            );
        }
        game.message(message(129, Value::Integer(2)))?;
        assert!(game.objects[&212259].world_layers.draws_in_sprite_pass(2));
        let state = game.objects[&212259].world_layers;
        for data in [
            Value::Null,
            Value::Real(1.0),
            Value::Text("1".into()),
            Value::Bytes(vec![1]),
            Value::Reference(target.clone()),
            Value::Integer(0),
            Value::Integer(-1),
            Value::Integer(8),
        ] {
            game.message(message(128, data))?;
            assert_eq!(game.objects[&212259].world_layers, state);
        }
        game.message(message(41, Value::Integer(0)))?;
        assert_eq!(game.objects[&212259].world_layers, state);
        let unsupported = game.message(message(42, Value::Integer(0))).unwrap_err();
        assert!(format!("{unsupported:#}").contains("identifier 42"));
        let mut wrong_class = message(128, Value::Integer(1));
        if let Value::Reference(r) = &mut wrong_class.target {
            r.class.as_mut().unwrap().name = "CImgAreaBox".into();
        }
        assert!(game.message(wrong_class).is_err());
        assert_eq!(game.objects[&212259].world_layers, state);
        game.present_scene()?;
        assert_eq!(game.frame(), pixels); // native fast background pass is unaffected
        let cursor = game.vm.cursor().cloned();
        let tween = crate::world::CameraTween::new(
            Camera::default(),
            Camera {
                position: [100, 0, 0],
                ..Camera::default()
            },
            1,
        )?;
        game.objects.get_mut(&212259).unwrap().pending_camera = Some(tween);
        for (class, fields) in [
            (
                "OM_StopAutoAnim",
                BTreeMap::from([("停止モード".into(), Value::Integer(0))]),
            ),
            (
                "OM_SetCacheExtra",
                BTreeMap::from([("左辺".into(), Value::Integer(2))]),
            ),
        ] {
            game.message(Message {
                class: class.into(),
                target: Value::Reference(target.clone()),
                fields,
            })?;
        }
        let object = &game.objects[&212259];
        assert_eq!(object.pending_camera, Some(tween));
        assert!(object.auto_update && object.visible);
        assert!(matches!(
            object.properties["render.auto_anim.停止モード"],
            Value::Integer(0)
        ));
        assert!(matches!(
            object.properties["render.cache_extra.左辺"],
            Value::Integer(2)
        ));
        // This test presents only the current camera; the queued tween must not
        // be canceled by a distinct native auto-animation stop/cache hint.
        game.message(Message {
            class: "OM_SetSpecifyCmdLighting".into(),
            target: Value::Reference(target.clone()),
            fields: BTreeMap::from([
                ("モード".into(), Value::Integer(0)),
                ("周期".into(), Value::Integer(100)),
                ("色1".into(), Value::Bytes(vec![0, 1, 2, 3])),
            ]),
        })?;
        assert!(matches!(
            game.objects[&212259].properties["render.lighting.周期"],
            Value::Integer(100)
        ));
        assert!(
            matches!(&game.objects[&212259].properties["render.lighting.色1"], Value::Bytes(bytes) if bytes == &[0, 1, 2, 3])
        );
        assert_eq!(game.vm.cursor(), cursor.as_ref());
        assert!(game.wait_ms.is_none() && game.parse_transition.is_none());
        game.present_scene()?;
        assert_eq!(game.frame(), pixels);
        let weather = BTreeMap::from([
            ("非同期ハンドル".into(), Value::Null),
            ("変化時間".into(), Value::Integer(500)),
            ("対象ウインドウ".into(), Value::Reference(target.clone())),
        ]);
        game.set_weather(212259, weather.clone())?;
        assert!(matches!(
            game.objects[&212259].properties["render.weather.変化時間"],
            Value::Integer(500)
        ));
        let mut invalid = weather;
        invalid.insert("非同期ハンドル".into(), Value::Reference(target.clone()));
        invalid.insert("変化時間".into(), Value::Integer(0));
        assert!(game.set_weather(212259, invalid).is_err());
        assert!(matches!(
            game.objects[&212259].properties["render.weather.変化時間"],
            Value::Integer(500)
        ));
        let field = |name: &str, value| {
            BTreeMap::from([
                ("メンバ名".into(), Value::Text(name.into())),
                ("値".into(), value),
            ])
        };
        for (name, value) in [("X", Value::Integer(123)), ("screen", Value::Real(720.0))] {
            game.set_effect_camera_member(6472019, &field(name, value))?;
        }
        assert!(matches!(
            game.objects[&6472019].properties["member.X"],
            Value::Integer(123)
        ));
        assert!(
            game.set_effect_camera_member(6472019, &field("unknown", Value::Integer(0)))
                .is_err()
        );
        assert!(
            game.set_effect_camera_member(6472019, &field("X", Value::Null))
                .is_err()
        );
        assert!(
            game.set_effect_camera_member(6472019, &field("X", Value::Real(f64::NAN)))
                .is_err()
        );
        assert!(matches!(
            game.objects[&6472019].properties["member.X"],
            Value::Integer(123)
        ));
        assert_eq!(game.vm.cursor(), cursor.as_ref());
        assert!(game.wait_ms.is_none() && game.parse_transition.is_none());
        game.present_scene()?;
        assert_eq!(game.frame(), pixels);
        let mut effect = target.clone();
        effect.class.as_mut().unwrap().name = "CLayerSimpleFlush".into();
        effect.offset = Some(1014184);
        for (key, data) in [
            (0, Value::Reference(target.clone())),
            (1, Value::Integer(12)),
            (15, Value::Integer(0)),
        ] {
            game.message(Message {
                class: "OM_SetVarData".into(),
                target: Value::Reference(effect.clone()),
                fields: BTreeMap::from([
                    ("識別値".into(), Value::Integer(key)),
                    ("データ".into(), data),
                ]),
            })?;
        }
        assert!(
            matches!(&game.objects[&1014184].properties["render.var.0"], Value::Reference(r) if r.offset == target.offset)
        );
        assert_eq!(
            game.objects[&1014184].properties["render.var.1"].integer()?,
            12
        );
        game.message(Message {
            class: "OM_SetColorData".into(),
            target: Value::Reference(effect.clone()),
            fields: BTreeMap::from([
                ("識別値".into(), Value::Integer(0)),
                ("データ".into(), Value::Integer(0x123456)),
            ]),
        })?;
        assert_eq!(
            game.objects[&1014184].properties["render.color.0"].integer()?,
            0x123456
        );
        assert!(matches!(
            game.objects[&1014184].properties["render.var.0"],
            Value::Reference(_)
        ));
        game.message(Message {
            class: "OM_PlayMedia".into(),
            target: Value::Reference(effect.clone()),
            fields: BTreeMap::new(),
        })?;
        use rugp_scene_vm::{Argument, Scenario};
        let sync = |address, reference, mode, timeout| Instruction {
            index: 0,
            stream_offset: 0,
            class: ClassId {
                name: "CVmSync".into(),
                schema: 17,
            },
            address,
            next_address: address + 24,
            flags: 0,
            arguments: vec![
                Argument::Reference(reference),
                Argument::Number(mode),
                Argument::Number(timeout),
            ],
        };
        let null = rugp_assets::object::ObjectReader::new(&[0; 3]).reference(0)?;
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 128,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    sync(0, effect.clone(), 32, 0),
                    sync(24, effect, 16, 0),
                    sync(48, null, 0, 1000000),
                ],
            },
        )?;
        game.vm = Vm::new(Arc::new(program), 1)?;
        game.step(0)?;
        assert_eq!(game.vm.cursor().unwrap().address, 48);
        assert_eq!(game.vm.synchronization_status, Some(true));
        assert_eq!(game.frame(), pixels); // omitted effect has no stray overlay
        Ok(())
    }
}
