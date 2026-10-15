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
        ensure!(
            class.name == "CParseBgBox",
            "variable-data target {} is not implemented yet",
            class.name
        );
        let key = fields
            .get("識別値")
            .context("variable-data identifier missing")?
            .integer()?;
        ensure!(
            [128, 129].contains(&key),
            "CParseBgBox variable-data identifier {key} is not implemented yet"
        );
        let data = fields
            .get("データ")
            .context("variable-data payload missing")?;
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
        let plane = Plane::bitmap(&image)?;
        let mut object = ObjectState {
            rect: (0, 0, 4, 4),
            ..ObjectState::default()
        };
        object.layers.insert(
            512,
            Layer {
                texture: Arc::new(image),
                plane: Some(plane),
            },
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
        let unsupported = game.message(message(41, Value::Integer(0))).unwrap_err();
        assert!(format!("{unsupported:#}").contains("identifier 41"));
        let mut wrong_class = message(128, Value::Integer(1));
        if let Value::Reference(r) = &mut wrong_class.target {
            r.class.as_mut().unwrap().name = "CImgAreaBox".into();
        }
        assert!(game.message(wrong_class).is_err());
        assert_eq!(game.objects[&212259].world_layers, state);
        game.present_scene()?;
        assert_eq!(game.frame(), pixels); // native fast background pass is unaffected
        Ok(())
    }
}
