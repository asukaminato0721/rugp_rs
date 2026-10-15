use super::*;

impl Game {
    pub(super) fn embed_alpha(
        &mut self,
        target: &ObjectRef,
        fields: BTreeMap<String, Value>,
    ) -> Result<()> {
        ensure!(
            target
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CParseBgBox" && c.schema == 15),
            "unsupported alpha-embedding target"
        );
        let id = target
            .offset
            .context("alpha-embedding target identity missing")?;
        ensure!(
            number(&fields, "スタイル", 0)? == 0
                && number(&fields, "チャンネルID", 0)? == 0
                && number(&fields, "X座標", 0)? == 0
                && number(&fields, "Y座標", 0)? == 0,
            "unsupported alpha-embedding mode/channel/position"
        );
        let Some(Value::Reference(window)) = fields.get("対象ウインドウ") else {
            bail!("alpha-embedding window missing");
        };
        ensure!(
            window.offset == Some(id)
                && window
                    .class
                    .as_ref()
                    .is_some_and(|c| c.name == "CParseBgBox"),
            "cross-window alpha embedding is not implemented"
        );
        // Vm60 1003d430: style bit 0x100 selects the captured surface;
        // style 0 instead clears the primary native plane's alpha bytes via
        // 1003d4c0. It has no output handle, animation or completion wait.
        // Playback-first approximation: keep the existing RGBA/world planes
        // and record the request, without flattening geometry or changing
        // unrelated camera/posture/transition clocks. Native alpha mutation
        // is intentionally not reproduced by this portable presentation yet.
        for (name, value) in fields {
            self.objects
                .entry(id)
                .or_default()
                .properties
                .insert(format!("render.alpha_embed.{name}"), value);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugp_assets::{object::ClassId, script::Script};
    use rugp_scene_vm::Scenario;
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_clear_alpha_fallback_keeps_world_pixels_cameras_and_waits_and_acknowledges_vm()
    -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        game.frame = Framebuffer::new(4, 4)?;
        let texture = Arc::new(Texture {
            width: 2,
            height: 2,
            display_scale: 1.0,
            rgba: vec![
                255, 0, 0, 0, 0, 255, 0, 64, 0, 0, 255, 128, 200, 150, 100, 255,
            ],
        });
        let plane = crate::world::Plane::bitmap(&texture)?;
        let mut first = None;
        let mut last_target = None;
        for (scene_id, address) in [(235, 0x28d4), (280, 0xb6b0), (330, 0xf09c)] {
            let scene = Scenario::decode_with_schema(
                &Script::load(&game.archive, scene_id)?,
                game.archive.address_shift,
                Some(&native),
            )?;
            let instruction = scene
                .instructions
                .iter()
                .find(|i| i.address == address)
                .context("retail alpha packet")?
                .clone();
            let message = game.vm.message(&instruction)?;
            let Value::Reference(target) = &message.target else {
                bail!("target reference missing");
            };
            let id = target.offset.unwrap();
            last_target = Some(target.clone());
            let object = game.objects.entry(id).or_default();
            object.rect = (0, 0, 4, 4);
            object.layers.insert(
                0,
                Layer::World(WorldImage {
                    planes: vec![crate::image_manager::WorldImagePlane {
                        texture: Arc::clone(&texture),
                        plane,
                        draw_flags: 0,
                    }],
                    boundary: plane,
                }),
            );
            let camera = object.camera;
            let pending = crate::world::CameraTween::new(
                camera,
                Camera {
                    position: [10, 20, 30],
                    ..camera
                },
                0,
            )?;
            object.pending_camera = Some(pending);
            game.present_scene()?;
            let before = game.frame().to_vec();
            game.wait_ms = Some(500);
            game.message(message)?;
            assert_eq!(game.frame(), before);
            assert_eq!(game.wait_ms, Some(500));
            let object = &game.objects[&id];
            assert_eq!(object.camera, camera);
            assert_eq!(object.pending_camera, Some(pending));
            let Layer::World(world) = &object.layers[&0] else {
                bail!("world flattened");
            };
            assert!(Arc::ptr_eq(&world.planes[0].texture, &texture));
            assert_eq!(
                number(&object.properties, "render.alpha_embed.スタイル", -1)?,
                0
            );
            assert!(object.postures.is_empty());
            first.get_or_insert(instruction);
        }
        game.wait_ms = None;
        let mut instruction = first.unwrap();
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
        assert_eq!(game.step(0)?, RunState::Ended);
        assert!(game.pending_sounds().is_empty());
        // No loose fallback for unrecognized style/channel combinations.
        let invalid = BTreeMap::from([("スタイル".into(), Value::Integer(1))]);
        assert!(game.embed_alpha(&last_target.unwrap(), invalid).is_err());
        Ok(())
    }
}
