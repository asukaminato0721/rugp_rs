//! Region blits and bounded offscreen image areas.
use super::{Game, Layer, Transition};
use crate::render::{Framebuffer, Texture};
use anyhow::{Context, Result, bail, ensure};
use rugp_assets::{fade::Fade, object::ObjectRef};
use rugp_scene_vm::{Argument, Instruction, vm::Value};
use std::sync::Arc;

/// Rectangles are x/y/width/height; an empty extent selects the full surface.
/// Crop/scale into a destination snapshot, preserving pixels outside the region.
fn place_region(
    source: &Texture,
    src: [i16; 4],
    destination: &Texture,
    dst: [i16; 4],
) -> Result<Texture> {
    let rect = |raw: [i16; 4], width: u32, height: u32| -> Result<[i64; 4]> {
        ensure!(raw[2] >= 0 && raw[3] >= 0, "negative blit extent");
        Ok([
            i64::from(raw[0]),
            i64::from(raw[1]),
            if raw[2] == 0 {
                i64::from(width)
            } else {
                i64::from(raw[2])
            },
            if raw[3] == 0 {
                i64::from(height)
            } else {
                i64::from(raw[3])
            },
        ])
    };
    let [sx, sy, sw, sh] = rect(src, source.width, source.height)?;
    let [dx, dy, dw, dh] = rect(dst, destination.width, destination.height)?;
    let mut rgba = destination.rgba.clone();
    for y in dy.max(0)..(dy + dh).min(i64::from(destination.height)) {
        let source_y = sy + (y - dy) * sh / dh;
        if !(0..i64::from(source.height)).contains(&source_y) {
            continue;
        }
        for x in dx.max(0)..(dx + dw).min(i64::from(destination.width)) {
            let source_x = sx + (x - dx) * sw / dw;
            if !(0..i64::from(source.width)).contains(&source_x) {
                continue;
            }
            let input = (source_y as usize * source.width as usize + source_x as usize) * 4;
            let output = (y as usize * destination.width as usize + x as usize) * 4;
            rgba[output..output + 4].copy_from_slice(&source.rgba[input..input + 4]);
        }
    }
    Ok(Texture {
        width: destination.width,
        height: destination.height,
        display_scale: 1.0,
        rgba,
    })
}

impl Game {
    pub(super) fn load_vm_image(&mut self, instruction: &Instruction) -> Result<()> {
        ensure!(
            instruction.arguments.len() == 6,
            "invalid image instruction arguments"
        );
        for argument in [
            &instruction.arguments[0],
            &instruction.arguments[1],
            &instruction.arguments[3],
        ] {
            ensure!(
                self.vm.evaluate(argument)?.integer()? == 0,
                "positioned/specialized CVmImage mode is not implemented"
            );
        }
        // Vm60 100708d2 forwards the third field as OM_LoadImage's
        // "スクリーンへすぐに反映？" boolean, not an image composition mode.
        let immediate = self.vm.evaluate(&instruction.arguments[2])?.integer()?;
        ensure!(
            [0, 1].contains(&immediate),
            "invalid image immediate-presentation flag"
        );
        let Value::Reference(destination) = self.vm.resolve_object(&instruction.arguments[5])?
        else {
            bail!("image destination missing")
        };
        let image = match self.vm.evaluate(&instruction.arguments[4])? {
            Value::Reference(source) => self.surface(&source)?,
            Value::Null => {
                let frame = Framebuffer::transparent(self.frame.width, self.frame.height)?;
                Arc::new(Texture {
                    width: frame.width,
                    height: frame.height,
                    display_scale: 1.0,
                    rgba: frame.rgba,
                })
            }
            _ => bail!("image source is not an object"),
        };
        self.write_surface(
            &destination,
            Texture {
                width: image.width,
                height: image.height,
                display_scale: image.display_scale,
                rgba: image.rgba.clone(),
            },
        )?;
        if immediate != 0 && destination.offset != Some(self.primary_buffer) {
            self.present_scene()?;
        }
        Ok(())
    }
    pub(super) fn vram_image_in(
        &mut self,
        id: u64,
        fields: &std::collections::BTreeMap<String, Value>,
    ) -> Result<()> {
        let Some(Value::Reference(source)) = fields.get("画像ソースウインドウ") else {
            bail!("captured image source missing")
        };
        let texture = self.surface(source)?;
        let style = super::number(fields, "スタイル", 512)?;
        self.buffers.remove(&id);
        let object = self.objects.entry(id).or_default();
        object.layers.clear();
        object.layers.insert(style, Layer::Screen(texture));
        object.menu = None;
        object.properties.extend(fields.clone());
        Ok(())
    }
    pub(super) fn build_parse_image(
        &mut self,
        id: u64,
        fields: &std::collections::BTreeMap<String, Value>,
    ) -> Result<()> {
        let Some(Value::Reference(destination)) = fields.get("対象ウインドウ") else {
            bail!("parse image destination missing")
        };
        let mut frame = Framebuffer::new(self.frame.width, self.frame.height)?;
        let source = self
            .objects
            .get(&id)
            .context("parse image source missing")?;
        if self.rasterize_scene {
            source.render(&mut frame)?;
        } else {
            source.validate_projection()?;
        }
        for (&other_id, object) in &self.objects {
            if other_id != id
                && object.parent == Some(id)
                && object.visible
                && object.has_character_image()
            {
                if self.rasterize_scene {
                    object.render(&mut frame)?;
                } else {
                    object.validate_projection()?;
                }
            }
        }
        self.write_surface(
            destination,
            Texture {
                width: frame.width,
                height: frame.height,
                display_scale: 1.0,
                rgba: frame.rgba,
            },
        )
    }
    fn surface(&mut self, reference: &ObjectRef) -> Result<Arc<Texture>> {
        if reference.inline.is_some() {
            return self.images.load(reference);
        }
        let id = reference.offset.context("surface has no bound object")?;
        if id == self.primary_buffer {
            return Ok(Arc::new(Texture {
                width: self.frame.width,
                height: self.frame.height,
                display_scale: 1.0,
                rgba: self.frame.rgba.clone(),
            }));
        }
        if let Some(buffer) = self.buffers.get(&id) {
            return Ok(Arc::clone(buffer));
        }
        if reference
            .class
            .as_ref()
            .is_some_and(|c| ["CImgBox", "CImgAreaBox", "CParseBgBox"].contains(&c.name.as_str()))
        {
            let size = self
                .objects
                .get(&id)
                .map_or((self.frame.width, self.frame.height), |o| {
                    if o.rect.2 > 0 && o.rect.3 > 0 {
                        (o.rect.2, o.rect.3)
                    } else {
                        (self.frame.width, self.frame.height)
                    }
                });
            let mut frame = Framebuffer::new(size.0, size.1)?;
            if let Some(object) = self.objects.get(&id) {
                if self.rasterize_scene {
                    object.render(&mut frame)?;
                } else {
                    object.validate_projection()?;
                }
            }
            return Ok(Arc::new(Texture {
                width: frame.width,
                height: frame.height,
                display_scale: 1.0,
                rgba: frame.rgba,
            }));
        }
        self.images.load(reference)
    }
    fn write_surface(&mut self, reference: &ObjectRef, texture: Texture) -> Result<()> {
        let id = reference
            .offset
            .context("destination surface has no bound object")?;
        if id == self.primary_buffer {
            ensure!(
                texture.width == self.frame.width && texture.height == self.frame.height,
                "primary surface dimensions do not match framebuffer"
            );
            self.frame.rgba = texture.rgba;
            self.transition = None;
        } else {
            ensure!(
                reference.class.as_ref().is_some_and(
                    |c| ["CImgBox", "CImgAreaBox", "CParseBgBox"].contains(&c.name.as_str())
                ),
                "unsupported destination surface"
            );
            let old_size = self.buffers.get(&id).map_or(0, |b| b.rgba.len());
            let used: usize = self.buffers.values().map(|b| b.rgba.len()).sum();
            ensure!(
                used - old_size + texture.rgba.len() <= 128 * 1024 * 1024,
                "offscreen surface budget exceeded"
            );
            let texture = Arc::new(texture);
            self.buffers.insert(id, Arc::clone(&texture));
            let object = self.objects.entry(id).or_default();
            if reference
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CImgBox")
                && matches!(
                    object.properties.get("render.image.fit"),
                    Some(Value::Integer(1))
                )
            {
                object.rect.2 = texture.width;
                object.rect.3 = texture.height;
            }
            object.layers.insert(0, Layer::Screen(texture));
        }
        Ok(())
    }
    /// Returns whether the VM must wait for this effect before acknowledging it.
    pub(super) fn blit(&mut self, instruction: &Instruction) -> Result<bool> {
        ensure!(
            self.parse_transition.is_none(),
            "framebuffer effects during a ParseScreen transition are not implemented yet"
        );
        let [
            Argument::Message { class, arguments },
            control,
            blocking,
            flags,
            duration,
        ] = instruction.arguments.as_slice()
        else {
            bail!("blit has no inline effect");
        };
        ensure!(class.schema == 12, "unsupported fade schema");
        let [Argument::Bytes(data), source, background, destination] = arguments.as_slice() else {
            bail!("invalid fade reference fields");
        };
        let mut fade = Fade::from_inline(&class.name, data)?;
        ensure!(
            matches!(fade.mode, 0 | 0x11),
            "unsupported fade region mode {:#x}",
            fade.mode
        );
        if fade.mode == 0 {
            // Default surface bounds. Specialized stretch records also retain
            // initial strip geometry, which is not their final image crop.
            fade.rectangles = [[0; 4]; 3];
        }
        // The auxiliary word is nonzero in ordinary retail copies too. It
        // does not replace the separately decoded blocking/time parameters.
        let _control = self.vm.evaluate(control)?.integer()?;
        let flags = self.vm.evaluate(flags)?.integer()?;
        ensure!(matches!(flags, 0 | 1), "unsupported blit flags {flags:#x}");
        let blocking = self.vm.evaluate(blocking)?.integer()?;
        ensure!(blocking == 0 || blocking == 1, "invalid blit wait mode");
        let duration = self.vm.evaluate(duration)?.integer()?;
        ensure!(
            duration >= 0 || (flags == 1 && blocking == 0 && duration == -1),
            "unbounded blit duration is not implemented yet"
        );
        let resolve = |arg: &Argument| -> Result<ObjectRef> {
            match self.vm.evaluate(arg)? {
                Value::Reference(reference) => Ok(reference),
                _ => bail!("fade surface is not an object"),
            }
        };
        let source = resolve(source)?;
        let background = resolve(background)?;
        let destination = resolve(destination)?;
        if flags == 1 {
            // Native Xs effects attach to a window, not the primary surface.
            // Keep their configuration without replacing world planes/cameras
            // with a captured screen texture. Distortion itself is omitted;
            // finite blocking requests still retain their explicit wait.
            ensure!(
                destination.class.as_ref().is_some_and(|c| [
                    "CParseBgBox",
                    "CImgAreaBox",
                    "CImgBox"
                ]
                .contains(&c.name.as_str())),
                "unsupported window-effect destination"
            );
            let id = destination
                .offset
                .context("window-effect destination identity missing")?;
            let properties = &mut self.objects.entry(id).or_default().properties;
            for (key, value) in [
                ("class", Value::Text(class.name.clone())),
                ("duration", Value::Integer(duration)),
                ("source", Value::Reference(source)),
                ("background", Value::Reference(background)),
                ("packet", Value::Bytes(data.clone())),
            ] {
                properties.insert(format!("render.xs_fade.{key}"), value);
            }
            if blocking != 0 && duration > 0 {
                self.wait_ms = Some(duration as u32);
                return Ok(true);
            }
            return Ok(false);
        }
        let duration = duration as u32;
        let source_reference = source.clone();
        let background_reference = background.clone();
        let source = self.surface(&source)?;
        let previous = self.surface(&destination)?;
        let background = if let Some(color) = fade.color {
            let mut frame = Framebuffer::new(self.frame.width, self.frame.height)?;
            frame.clear(color);
            Arc::new(Texture {
                width: frame.width,
                height: frame.height,
                display_scale: 1.0,
                rgba: frame.rgba,
            })
        } else {
            self.surface(&background)?
        };
        let source = place_region(&source, fade.rectangles[0], &previous, fade.rectangles[2])?;
        let background = place_region(
            &background,
            fade.rectangles[1],
            &previous,
            fade.rectangles[2],
        )?;
        if duration > 0
            && destination.offset != Some(self.primary_buffer)
            && source.rgba == previous.rgba
            && background.rgba == previous.rgba
        {
            // With identical placed endpoints every sample of the portable
            // blend is identical too. Retain the native effect metadata and
            // exact wait without replacing the image/window's live layers.
            // Specialized spatial deformation remains approximate, as for
            // the primary-surface and flags-1 presentation paths.
            ensure!(
                destination.class.as_ref().is_some_and(|c| [
                    "CImgBox",
                    "CImgAreaBox",
                    "CParseBgBox"
                ]
                .contains(&c.name.as_str())),
                "unsupported unchanged-surface destination"
            );
            let id = destination
                .offset
                .context("unchanged-surface identity missing")?;
            let properties = &mut self.objects.entry(id).or_default().properties;
            for (key, value) in [
                ("class", Value::Text(class.name.clone())),
                ("duration", Value::Integer(duration as i32)),
                ("source", Value::Reference(source_reference)),
                ("background", Value::Reference(background_reference)),
                ("packet", Value::Bytes(data.clone())),
            ] {
                properties.insert(format!("render.surface_fade.{key}"), value);
            }
            if blocking != 0 {
                self.wait_ms = Some(duration);
                return Ok(true);
            }
            return Ok(false);
        }
        if duration == 0 {
            let alpha = u32::from(fade.curve.alpha(0, 0));
            let rgba = source
                .rgba
                .iter()
                .zip(&background.rgba)
                .map(|(s, b)| {
                    ((u32::from(*s) * alpha + u32::from(*b) * (256 - alpha) + 128) / 256) as u8
                })
                .collect();
            self.write_surface(
                &destination,
                Texture {
                    width: source.width,
                    height: source.height,
                    display_scale: 1.0,
                    rgba,
                },
            )?;
        } else {
            ensure!(
                destination.offset == Some(self.primary_buffer),
                "timed offscreen blit is not implemented yet"
            );
            self.transition = Some(Transition {
                from: background.rgba.clone(),
                to: source.rgba.clone(),
                elapsed: 0,
                duration,
                curve: Some(fade.curve),
            });
            self.update_frame();
            if blocking != 0 {
                self.wait_ms = Some(duration);
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_unchanged_offscreen_fade_keeps_live_surface_and_exact_wait() -> Result<()> {
        use crate::game::{ObjectState, RunState};
        use rugp_assets::{native_schema::NativeSchema, object::ClassId, script::Script};
        use rugp_scene_vm::{
            Scenario,
            vm::{Program, Vm},
        };
        use std::collections::BTreeMap;
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
            .find(|i| i.address == 0x841c)
            .context("ending self-surface fade")?
            .clone();
        let Argument::Message { arguments, .. } = &instruction.arguments[0] else {
            bail!("effect missing")
        };
        let Argument::Reference(target) = &arguments[3] else {
            bail!("effect target missing")
        };
        let id = target.offset.unwrap();
        let texture = Arc::new(Texture {
            width: 4,
            height: 4,
            display_scale: 1.0,
            rgba: [255, 0, 0, 255].repeat(16),
        });
        let camera = crate::world::Camera {
            position: [3, 4, 5],
            ..crate::world::Camera::default()
        };
        let pending = crate::world::CameraTween::new(camera, crate::world::Camera::default(), 0)?;
        game.frame = Framebuffer::new(4, 4)?;
        game.objects.insert(
            id,
            ObjectState {
                rect: (0, 0, 4, 4),
                camera,
                pending_camera: Some(pending),
                layers: BTreeMap::from([(0, Layer::Screen(Arc::clone(&texture)))]),
                ..ObjectState::default()
            },
        );
        game.present_scene()?;
        let before = game.frame().to_vec();
        let mut different = instruction.clone();
        instruction.address = 0;
        instruction.next_address = 24;
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
                        address: 24,
                        next_address: u32::MAX,
                        flags: 0,
                        arguments: vec![],
                    },
                ],
            },
        )?;
        game.vm = Vm::new(Arc::new(program), 1)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert_eq!(game.wait_ms, Some(3000));
        assert_eq!(game.step(2999)?, RunState::Waiting);
        assert_eq!(game.wait_ms, Some(1));
        assert_eq!(game.step(1)?, RunState::Ended);
        assert_eq!(game.frame(), before);
        let object = &game.objects[&id];
        assert_eq!(object.camera, camera);
        assert_eq!(object.pending_camera, Some(pending));
        assert_eq!(object.rect, (0, 0, 4, 4));
        let Layer::Screen(stored) = &object.layers[&0] else {
            bail!("surface replaced")
        };
        assert!(Arc::ptr_eq(stored, &texture));
        assert!(matches!(
            object.properties["render.surface_fade.duration"],
            Value::Integer(3000)
        ));
        assert!(!game.buffers.contains_key(&id));
        assert!(game.pending_sounds().is_empty());
        // This identity optimization must not discard a real offscreen change.
        let Argument::Message { arguments, .. } = &mut different.arguments[0] else {
            unreachable!()
        };
        let Argument::Reference(source) = &mut arguments[1] else {
            unreachable!()
        };
        source.offset = Some(213019);
        game.objects.insert(
            213019,
            ObjectState {
                rect: (0, 0, 4, 4),
                layers: BTreeMap::from([(
                    0,
                    Layer::Screen(Arc::new(Texture {
                        width: 4,
                        height: 4,
                        display_scale: 1.0,
                        rgba: [0, 255, 0, 255].repeat(16),
                    })),
                )]),
                ..ObjectState::default()
            },
        );
        assert!(
            game.blit(&different)
                .unwrap_err()
                .to_string()
                .contains("timed offscreen blit")
        );
        assert_eq!(game.frame(), before);
        assert!(game.wait_ms.is_none());
        Ok(())
    }

    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn window_effect_fallback_preserves_world_layers_and_blocking_clocks() -> Result<()> {
        use crate::{
            game::{ObjectState, RunState},
            image_manager::WorldImage,
        };
        use rugp_assets::{native_schema::NativeSchema, object::ClassId, script::Script};
        use rugp_scene_vm::{
            Scenario,
            vm::{Message, Program, Vm},
        };
        use std::collections::BTreeMap;
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let scene = Scenario::decode_with_schema(
            &Script::load(&game.archive, 48)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let original = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x120)
            .context("window effect missing")?;
        let vm = |blocking, flags, duration| -> Result<Vm> {
            let mut instruction = original.clone();
            instruction.address = 0;
            instruction.next_address = 24;
            instruction.arguments[2] = Argument::Number(blocking);
            instruction.arguments[3] = Argument::Number(flags);
            instruction.arguments[4] = Argument::Number(duration);
            let mut program = Program::default();
            program.insert(
                0,
                Scenario {
                    object_id: 1,
                    memory_size: 36,
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
                            address: 24,
                            next_address: u32::MAX,
                            flags: 0,
                            arguments: vec![],
                        },
                    ],
                },
            )?;
            Vm::new(Arc::new(program), 1)
        };
        let texture = Texture {
            width: 2,
            height: 2,
            display_scale: 1.0,
            rgba: [32, 64, 128, 255].repeat(4),
        };
        let plane = crate::world::Plane::bitmap(&texture)?;
        let mut object = ObjectState {
            rect: (0, 0, 4, 4),
            ..ObjectState::default()
        };
        object.layers.insert(
            512,
            Layer::World(WorldImage {
                planes: vec![crate::image_manager::WorldImagePlane {
                    texture: Arc::new(texture),
                    plane,
                    draw_flags: 0,
                }],
                boundary: plane,
            }),
        );
        game.objects.insert(212259, object);
        game.frame = Framebuffer::new(4, 4)?;
        let pixels = game.frame().to_vec();
        game.vm = vm(1, 1, 400)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert!(matches!(
            game.objects[&212259].layers[&512],
            Layer::World(_)
        ));
        assert!(!game.buffers.contains_key(&212259));
        assert_eq!(game.frame(), pixels);
        assert_eq!(game.step(399)?, RunState::Waiting);
        assert_eq!(game.vm.cursor().unwrap().address, 0);
        assert_eq!(game.step(1)?, RunState::Ended);
        game.ended = false;
        game.vm = vm(0, 1, u32::MAX)?; // -1: retained indefinite, non-blocking effect
        assert_eq!(game.step(0)?, RunState::Ended);
        assert!(matches!(
            game.objects[&212259].properties["render.xs_fade.duration"],
            Value::Integer(-1)
        ));
        let target = match &original.arguments[0] {
            Argument::Message { arguments, .. } => match &arguments[3] {
                Argument::Reference(r) => r.clone(),
                _ => bail!("effect destination missing"),
            },
            _ => bail!("effect missing"),
        };
        let stop = |mode| Message {
            class: "OM_StopXsFade".into(),
            target: Value::Reference(target.clone()),
            fields: BTreeMap::from([("モード".into(), Value::Integer(mode))]),
        };
        assert!(game.message(stop(2)).is_err());
        assert!(
            game.objects[&212259]
                .properties
                .contains_key("render.xs_fade.class")
        );
        game.message(stop(1))?;
        game.message(stop(0))?; // repeated stopping is harmless
        assert!(
            !game.objects[&212259]
                .properties
                .keys()
                .any(|key| key.starts_with("render.xs_fade."))
        );
        assert_eq!(game.frame(), pixels);
        for (blocking, flags, duration) in [(0, 2, 400), (1, 1, u32::MAX)] {
            game.ended = false;
            game.vm = vm(blocking, flags, duration)?;
            assert!(game.step(0).is_err());
            assert!(matches!(
                game.objects[&212259].layers[&512],
                Layer::World(_)
            ));
        }
        Ok(())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn vm_image_stages_real_image_box_surfaces_and_null_clears_them() -> Result<()> {
        use crate::game::{ObjectState, RunState};
        use rugp_assets::object::ClassId;
        use rugp_scene_vm::{
            Scenario,
            vm::{Program, Vm},
        };
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        game.frame = Framebuffer::new(4, 4)?;
        let reference = |offset| ObjectRef {
            class: Some(ClassId {
                name: "CImgBox".into(),
                schema: 15,
            }),
            cache_index: None,
            flags: 0xc108,
            offset: Some(offset),
            size: Some(3),
            owner: None,
            selector: 0,
            inline: None,
        };
        let mut null = reference(0);
        null.class = None;
        null.offset = None;
        null.cache_index = Some(0);
        let make_vm = |source: ObjectRef, mode| -> Result<Vm> {
            let instruction = |name: &str, address, next_address, arguments| Instruction {
                index: 0,
                stream_offset: 0,
                class: ClassId {
                    name: name.into(),
                    schema: 17,
                },
                address,
                next_address,
                flags: 0,
                arguments,
            };
            let mut program = Program::default();
            program.insert(
                0,
                Scenario {
                    object_id: 1,
                    memory_size: 80,
                    null_records: vec![],
                    trailing_bytes: 0,
                    instructions: vec![
                        instruction(
                            "CVmImage",
                            0,
                            28,
                            vec![
                                Argument::Number(0),
                                Argument::Number(0),
                                Argument::Number(mode),
                                Argument::Number(0),
                                Argument::Reference(source),
                                Argument::Reference(reference(501)),
                            ],
                        ),
                        instruction(
                            "CVmSync",
                            28,
                            52,
                            vec![
                                Argument::Reference(null.clone()),
                                Argument::Number(0),
                                Argument::Number(1000000),
                            ],
                        ),
                        instruction("CVmRet", 52, u32::MAX, vec![]),
                    ],
                },
            )?;
            Vm::new(Arc::new(program), 1)
        };
        let pixels = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        game.buffers.insert(
            500,
            Arc::new(Texture {
                width: 2,
                height: 2,
                display_scale: 1.0,
                rgba: pixels.clone(),
            }),
        );
        game.objects.insert(
            501,
            ObjectState {
                visible: false,
                rect: (0, 0, 4, 4),
                ..ObjectState::default()
            },
        );
        let before = game.frame().to_vec();
        game.vm = make_vm(reference(500), 0)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert_eq!(game.buffers[&501].rgba, pixels);
        assert_eq!(game.frame(), before);
        game.objects.get_mut(&501).unwrap().visible = true;
        game.present_scene()?;
        assert_eq!(&game.frame()[..4], &[255, 0, 0, 255]);
        assert_eq!(&game.frame()[60..], &[255, 255, 255, 255]);
        game.wait_ms = None;
        game.vm = make_vm(reference(500), 2)?;
        assert!(game.step(0).is_err());
        assert_eq!(game.buffers[&501].rgba, pixels);
        // The retail helper requests fitting followed by an immediate load.
        // Keep its operation packet, but use the tiny deterministic source
        // above so the displayed geometry has an independent pixel oracle.
        let native = super::super::NativeSchema::from_directory(root.as_ref())?;
        let scene = rugp_scene_vm::Scenario::decode_with_schema(
            &rugp_assets::script::Script::load(&game.archive, 149)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let command = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x68)
            .context("retail fit-image packet")?;
        let mut command = game.vm.message(command)?;
        command.target = Value::Reference(reference(501));
        game.objects.get_mut(&501).unwrap().rect = (1, 1, 0, 0);
        let staged = game.frame().to_vec();
        game.message(command)?;
        assert_eq!(game.objects[&501].rect, (1, 1, 0, 0));
        assert_eq!(game.frame(), staged);
        game.vm = make_vm(reference(500), 1)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert_eq!(game.objects[&501].rect, (1, 1, 2, 2));
        let mut expected = Framebuffer::new(4, 4)?;
        expected.blit(&game.buffers[&500], (1, 1, 2, 2), 255)?;
        assert_eq!(game.frame(), expected.rgba);
        game.wait_ms = None;
        game.vm = make_vm(null.clone(), 0)?;
        game.step(0)?;
        assert!(game.buffers[&501].rgba.iter().all(|b| *b == 0));
        game.present_scene()?;
        assert_eq!(game.frame(), before);
        // Diagnostic raster suppression must still suspend and release the VM
        // on the effect's real blocking clock.
        game.wait_ms = None;
        game.set_scene_rasterization(false);
        let mut data = Vec::new();
        for word in [2u16, 0, 0, 0, 0] {
            data.extend(word.to_le_bytes());
        }
        data.extend([0; 24]);
        for word in [0u16, 0, 256, 8192] {
            data.extend(word.to_le_bytes());
        }
        let mut primary = reference(game.primary_buffer);
        primary.class.as_mut().unwrap().name = "CFrameBuffer".into();
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 48,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    Instruction {
                        index: 0,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmBlt".into(),
                            schema: 17,
                        },
                        address: 0,
                        next_address: 24,
                        flags: 0,
                        arguments: vec![
                            Argument::Message {
                                class: ClassId {
                                    name: "CFadeNormal".into(),
                                    schema: 12,
                                },
                                arguments: vec![
                                    Argument::Bytes(data),
                                    Argument::Reference(reference(500)),
                                    Argument::Reference(primary.clone()),
                                    Argument::Reference(primary),
                                ],
                            },
                            Argument::Number(53032),
                            Argument::Number(1),
                            Argument::Number(0),
                            Argument::Number(400),
                        ],
                    },
                    Instruction {
                        index: 1,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmRet".into(),
                            schema: 17,
                        },
                        address: 24,
                        next_address: u32::MAX,
                        flags: 0,
                        arguments: vec![],
                    },
                ],
            },
        )?;
        game.vm = Vm::new(Arc::new(program), 1)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert_eq!(game.step(199)?, RunState::Waiting);
        assert_eq!(game.vm.cursor().unwrap().address, 0);
        assert_eq!(game.step(201)?, RunState::Ended);
        assert!(game.transition.is_none());
        assert_eq!(game.frame(), before);
        Ok(())
    }
    #[test]
    fn crop_scale_and_clipped_destination_preserve_other_pixels() -> Result<()> {
        let source = Texture {
            width: 4,
            height: 1,
            display_scale: 1.0,
            rgba: vec![1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255],
        };
        let destination = Texture {
            width: 4,
            height: 2,
            display_scale: 1.0,
            rgba: [9, 0, 0, 255].repeat(8),
        };
        let output = place_region(&source, [1, 0, 2, 1], &destination, [1, 1, 2, 1])?;
        assert_eq!(
            output
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[0])
                .collect::<Vec<_>>(),
            [9, 9, 9, 9, 9, 2, 3, 9]
        );
        let clipped = place_region(&source, [0; 4], &destination, [-2, 0, 4, 1])?;
        assert_eq!(&clipped.rgba[..8], &[3, 0, 0, 255, 4, 0, 0, 255]);
        let full = place_region(&source, [0; 4], &destination, [0; 4])?;
        assert_eq!(&full.rgba[..16], &source.rgba);
        assert_eq!(&full.rgba[16..], &source.rgba);
        assert!(place_region(&source, [0, 0, -1, 1], &destination, [0; 4]).is_err());
        Ok(())
    }
}
