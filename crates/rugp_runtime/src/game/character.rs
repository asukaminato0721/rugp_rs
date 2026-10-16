//! Practical character presentation. Keep face/layer packets independently of
//! generic object properties: subsequent packets reuse many of the same names.
use super::*;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SpriteGeometry {
    pub(super) size: [i32; 2],
    pub(super) anchor_q16: [i32; 2],
}

#[derive(Clone, Default)]
pub(super) struct CharacterState {
    pub(super) face: BTreeMap<String, Value>,
    pub(super) layers: BTreeMap<i32, [f32; 2]>,
    pub(super) texture: Option<Arc<Texture>>,
    pub(super) geometry: Option<SpriteGeometry>,
    pub(super) position: [i32; 3],
    pub(super) style: i32,
}
#[derive(Clone, Copy)]
pub(super) struct CharacterView<'a> {
    pub camera: &'a Camera,
    pub projection: u8,
    pub player_height: i32,
    pub angles_q4: [i32; 3],
}
impl SpriteGeometry {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.size.iter().all(|v| (0..=32767).contains(v)),
            "invalid sprite dimensions"
        );
        Ok(())
    }
}
impl CharacterState {
    pub(super) fn validate_texture(&self) -> Result<()> {
        if let Some(geometry) = self.geometry {
            geometry.validate()?;
        }
        self.texture.as_deref().map_or(Ok(()), Texture::validate)
    }
    pub(super) fn has_image(&self) -> bool {
        self.texture.is_some()
    }
    pub(super) fn render(
        &self,
        frame: &mut Framebuffer,
        viewport: (i32, i32, u32, u32),
        opacity: u8,
        view: CharacterView<'_>,
    ) -> Result<()> {
        let Some(texture) = &self.texture else {
            return Ok(());
        };
        // Vm60 10035d70: sprite coordinates are relative to the owning
        // world camera. The native player eye height is 1550 world units.
        let geometry = self.geometry.unwrap_or(SpriteGeometry {
            size: [texture.width as i32, texture.height as i32],
            anchor_q16: [
                (texture.width as i32 / 2) << 16,
                (texture.height as i32) << 16,
            ],
        });
        let CharacterView {
            camera: motion,
            projection,
            player_height,
            angles_q4,
        } = view;
        let mut depth = self.position[2].wrapping_sub(motion.position[2]);
        let mut relative = [
            self.position[0].wrapping_sub(motion.position[0]),
            self.position[1]
                .wrapping_sub(motion.position[1])
                .wrapping_add(player_height),
        ];
        let full_view = projection & 0x30 != 0;
        if angles_q4 != [0; 3] || (full_view && projection & 8 != 0) {
            let quad = self.camera_quad(geometry, view);
            return frame.blit_camera_plane(
                texture,
                quad,
                motion.focal_length,
                64.0,
                viewport,
                opacity,
            );
        }
        if full_view {
            let rotated = motion.rotate([relative[0], relative[1], depth]);
            relative = [rotated[0], rotated[1]];
            depth = rotated[2];
        }
        if depth < 1 {
            return Ok(());
        }
        let mut edges = [[0.0; 2]; 2];
        for axis in 0..2 {
            let origin = relative[axis]
                .wrapping_shl(13)
                .wrapping_sub(geometry.anchor_q16[axis] >> 3)
                .wrapping_sub(if full_view {
                    0
                } else {
                    motion.angles_q4[axis].wrapping_mul(depth)
                });
            let center = if axis == 0 {
                viewport.0.wrapping_add(viewport.2 as i32 / 2)
            } else {
                viewport.1.wrapping_add(viewport.3 as i32 / 2)
            };
            for (end, edge) in edges.iter_mut().enumerate() {
                let q13 = origin.wrapping_add((end as i32 * geometry.size[axis]).wrapping_shl(13));
                edge[axis] = f64::from(crate::world::mul_div(
                    q13,
                    i32::from(motion.focal_length),
                    depth,
                )) / 8192.0
                    + f64::from(center);
            }
        }
        let [left, top] = edges[0];
        let [right, bottom] = edges[1];
        frame.blit_projective(
            texture,
            [
                [left, top, f64::from(depth)],
                [right, top, f64::from(depth)],
                [right, bottom, f64::from(depth)],
                [left, bottom, f64::from(depth)],
            ],
            viewport,
            opacity,
        )
    }

    // Vm60 10035d70 rotates the center and the Q16 anchor separately.
    // Billboard mode rotates only the center. Bit 8 also rotates the corners.
    fn camera_quad(&self, geometry: SpriteGeometry, view: CharacterView<'_>) -> [[f64; 3]; 4] {
        use crate::world::rotation::{apply_q12, matrix};
        let camera = view.camera;
        let full = view.projection & 0x30 != 0;
        let mut center = std::array::from_fn(|i| self.position[i].wrapping_sub(camera.position[i]));
        center[1] = center[1].wrapping_add(view.player_height);
        if full {
            center = camera.rotate(center);
        } else {
            for i in 0..2 {
                center[i] =
                    center[i].wrapping_sub(camera.angles_q4[i].wrapping_mul(center[2]) >> 13);
            }
        }
        let mut rotation = matrix(view.angles_q4);
        if full && view.projection & 8 != 0 {
            let parent = camera.rotation();
            rotation = std::array::from_fn(|i| {
                std::array::from_fn(|j| {
                    ((0..3)
                        .map(|k| i32::from(parent[i][k]) * i32::from(rotation[k][j]))
                        .sum::<i32>()
                        >> 12) as i16
                })
            });
        }
        let anchor = apply_q12(
            rotation,
            [geometry.anchor_q16[0], geometry.anchor_q16[1], 0],
        );
        let translation: [i32; 3] =
            std::array::from_fn(|i| center[i].wrapping_sub((anchor[i] >> 28) as i32));
        let [w, h] = geometry.size;
        [[0, 0, 0], [w, 0, 0], [w, h, 0], [0, h, 0]].map(|point| {
            let rotated = apply_q12(rotation, point);
            std::array::from_fn(|i| rotated[i] as f64 / 4096.0 + f64::from(translation[i]))
        })
    }

    pub(super) fn remove(&mut self, style: i32) {
        if style == 0 || style == self.style {
            self.texture = None;
        }
    }
}
impl Game {
    fn character_geometry(
        &self,
        reference: &ObjectRef,
        texture: &Texture,
    ) -> Result<SpriteGeometry> {
        if !reference.class.as_ref().is_some_and(|c| c.name == "CRmt") {
            return Ok(SpriteGeometry {
                size: [texture.width as i32, texture.height as i32],
                anchor_q16: [
                    (texture.width as i32 / 2) << 16,
                    (texture.height as i32) << 16,
                ],
            });
        }
        let sprite = rugp_assets::crmt::Crmt::parse(
            &self.archive.read_at(
                reference.offset.context("sprite extent missing")?,
                reference.size.context("sprite size missing")? as usize,
            )?,
            self.archive.address_shift,
        )?;
        let mut anchor = [
            i32::from(sprite.width) << 15,
            i32::from(sprite.height) << 16,
        ];
        if let (Some(offset), Some(size)) =
            (sprite.image_properties.offset, sprite.image_properties.size)
            && let Some(point) = rugp_assets::image_properties::anchor(
                &self.archive.read_at(offset, size as usize)?,
                [sprite.base_offset.0, sprite.base_offset.1],
            )?
        {
            anchor = point;
        }
        let scale = |value| crate::world::mul_div(value, sprite.scale as i32, 65536);
        Ok(SpriteGeometry {
            size: [
                scale(i32::from(sprite.width)),
                scale(i32::from(sprite.height)),
            ],
            anchor_q16: anchor.map(scale),
        })
    }
    fn character_texture(
        &mut self,
        body: &ObjectRef,
        overlays: &[ObjectRef],
    ) -> Result<(Arc<Texture>, SpriteGeometry)> {
        let geometry;
        let body = {
            let texture = self.images.load(body)?;
            geometry = self.character_geometry(body, &texture)?;
            texture
        };
        let mut canvas = Framebuffer::transparent(body.width, body.height)?;
        canvas.blit(&body, (0, 0, body.width, body.height), 255)?;
        for reference in overlays.iter().filter(|r| r.class.is_some()) {
            let part = self.images.load(reference)?;
            canvas.blit(&part, (0, 0, body.width, body.height), 255)?;
        }
        Ok((
            Arc::new(Texture {
                width: canvas.width,
                height: canvas.height,
                display_scale: body.display_scale,
                rgba: canvas.rgba,
            }),
            geometry,
        ))
    }
    pub(super) fn set_character_core(
        &mut self,
        id: u64,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let key = number(fields, "データ識別値", -1)?;
        ensure!(
            key == 20,
            "unsupported character reference-data identifier {key}"
        );
        let reference = match fields.get("参照") {
            Some(Value::Null) => {
                let object = self.objects.entry(id).or_default();
                object.character.remove(0);
                object.character.face.clear();
                object.clear_character_sources();
                object
                    .properties
                    .insert("character.core".into(), Value::Null);
                return Ok(());
            }
            Some(Value::Reference(reference)) => reference,
            _ => bail!("character core reference missing"),
        };
        let class = reference
            .class
            .as_ref()
            .context("character core class missing")?;
        ensure!(
            class.name == "CChar2CompleteDataCore" && class.schema == 1,
            "unsupported character core class"
        );
        let core = reference
            .inline
            .as_ref()
            .context("external character core is not implemented")?;
        // ObjectReader already bounded and validated this record in its original
        // shared reference context. Do not reparse the copied body independently.
        let version = *core.data.first().context("empty character core")?;
        ensure!(
            (1..=6).contains(&version),
            "unsupported character core version {version}"
        );
        let body_index = if version >= 5 { 2 } else { 1 };
        ensure!(
            core.references.len() == body_index + 14,
            "invalid character core references"
        );
        let body = &core.references[body_index];
        if body.class.is_none() {
            let object = self.objects.entry(id).or_default();
            object.character.remove(0);
            object.character.face.clear();
            object.clear_character_sources();
            object
                .properties
                .insert("character.core".into(), Value::Reference(reference.clone()));
            return Ok(());
        }
        let pose = core
            .character_pose
            .as_deref()
            .copied()
            .context("character core pose missing")?;
        let (texture, geometry) =
            self.character_texture(body, &core.references[body_index + 1..])?;
        self.buffers.remove(&id);
        let object = self.objects.entry(id).or_default();
        object.layers.clear();
        object.menu = None;
        object.character.texture = Some(texture);
        object.character.geometry = Some(geometry);
        object.character.position = [pose[0], pose[1], pose[2]];
        for (name, angle) in ["ロール角量", "ピッチ角量", "バンク角量"]
            .into_iter()
            .zip(&pose[3..])
        {
            object
                .properties
                .insert(name.into(), Value::Integer(*angle));
        }
        object.character.face.clear();
        object.clear_character_sources();
        object
            .properties
            .insert("character.core".into(), Value::Reference(reference.clone()));
        Ok(())
    }
    pub(super) fn show_character(
        &mut self,
        id: u64,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let supplied_position = [
            number(fields, "３ＤＸ", 0)?,
            number(fields, "３ＤＹ", 0)?,
            number(fields, "３ＤＺ", 0)?,
        ];
        let style = number(fields, "スタイル", 0)?;
        // Vm60 100340b0: bits 0..2 replace axes, bits 3..5 add
        // offsets. Unselected axes retain their current position.
        let previous = self
            .objects
            .get(&id)
            .map_or([0; 3], |o| o.character.position);
        let position = std::array::from_fn(|axis| {
            if style & (1 << axis) != 0 {
                supplied_position[axis]
            } else if style & (8 << axis) != 0 {
                previous[axis].wrapping_add(supplied_position[axis])
            } else {
                previous[axis]
            }
        });
        if matches!(fields.get("ベース画像"), None | Some(Value::Null)) {
            ensure!(
                matches!(fields.get("表情画像"), None | Some(Value::Null)),
                "expression-only character update is not implemented"
            );
            // A null image packet can reposition the current actor without
            // replacing or reviving its image (including an empty actor).
            let object = self.objects.entry(id).or_default();
            object.character.position = position;
            object.character.style = style;
            object.prepare_character_angles();
            object.retain_character_placement(fields);
            return Ok(());
        }
        let Some(Value::Reference(body)) = fields.get("ベース画像") else {
            bail!("character message has no bound body");
        };
        let face = self
            .objects
            .get(&id)
            .map(|o| o.character.face.clone())
            .unwrap_or_default();
        let mut overlays = Vec::new();
        for name in ["眼ベース", "左眼", "右眼", "眼", "眉", "口", "頬", "その他"] {
            if let Some(Value::Reference(part)) = face.get(name) {
                overlays.push(part.clone());
            }
        }
        if let Some(Value::Reference(expression)) = fields.get("表情画像") {
            overlays.push(expression.clone());
        }
        let (texture, geometry) = self.character_texture(body, &overlays)?;
        let object = self.objects.entry(id).or_default();
        object.layers.clear();
        object.prepare_character_angles();
        object.menu = None;
        object.clear_character_sources();
        object.properties.remove("character.core");
        object.properties.insert(
            "native.character.body".into(),
            Value::Reference(body.clone()),
        );
        object.properties.insert(
            "native.character.expression".into(),
            fields.get("表情画像").cloned().unwrap_or(Value::Null),
        );
        for (name, value) in face {
            object
                .properties
                .insert(format!("native.character.face.{name}"), value);
        }
        object.retain_character_placement(fields);
        self.buffers.remove(&id);
        let character = &mut object.character;
        character.texture = Some(texture);
        character.geometry = Some(geometry);
        character.position = position;
        character.style = style;
        Ok(())
    }
    pub(super) fn queue_character_face(
        &mut self,
        id: u64,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        for name in ["口", "眼ベース", "左眼", "右眼", "眼", "眉", "頬", "その他"] {
            ensure!(
                matches!(
                    fields.get(name),
                    None | Some(Value::Null | Value::Reference(_))
                ),
                "invalid character face component {name}"
            );
        }
        // Queued parts are applied by the following HumanIn, not presented as
        // independent full-screen objects. Blink/lip animation is deferred.
        self.objects.entry(id).or_default().character.face = fields.clone();
        Ok(())
    }
    pub(super) fn set_character_layer(
        &mut self,
        id: u64,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let layer = number(fields, "指定先", 0)?;
        let vector = [
            f32::from_bits(number(fields, "ベクトル1", 0)? as u32),
            f32::from_bits(number(fields, "ベクトル2", 0)? as u32),
        ];
        ensure!(
            vector.iter().all(|v| v.is_finite()),
            "invalid character layer vector"
        );
        self.objects
            .entry(id)
            .or_default()
            .character
            .layers
            .insert(layer, vector);
        Ok(())
    }
}

impl ObjectState {
    fn prepare_character_angles(&mut self) {
        // Vm60 100340b0/10038920 keeps queued angles until presentation.
        // A helper can issue several HumanIn packets before that trigger.
        if !self.properties.contains_key("character.angles_pending") {
            for name in ["ロール角量", "ピッチ角量", "バンク角量"] {
                self.properties.insert(name.into(), Value::Integer(0));
            }
        }
    }
    /// Keep source identities independently of the composited texture. Native
    /// UUC records restore resources and geometry rather than RGBA screenshots.
    pub(super) fn clear_character_sources(&mut self) {
        self.properties
            .retain(|name, _| !name.starts_with("native.character."));
    }

    fn retain_character_placement(&mut self, fields: &BTreeMap<String, Value>) {
        for name in ["３ＤＸ", "３ＤＹ", "３ＤＺ", "スタイル"] {
            self.properties.insert(
                format!("native.character.{name}"),
                fields.get(name).cloned().unwrap_or(Value::Integer(0)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn entries_retain_queued_angles_until_presentation() -> Result<()> {
        let mut object = ObjectState::default();
        object
            .properties
            .insert("バンク角量".into(), Value::Integer(-27));
        object
            .properties
            .insert("character.angles_pending".into(), Value::Integer(1));
        object.prepare_character_angles();
        assert_eq!(number(&object.properties, "バンク角量", 0)?, -27);
        object.prepare_character_angles();
        assert_eq!(number(&object.properties, "バンク角量", 0)?, -27);
        object.properties.remove("character.angles_pending");
        object.prepare_character_angles();
        for name in ["ロール角量", "ピッチ角量", "バンク角量"] {
            assert_eq!(number(&object.properties, name, -1)?, 0);
        }
        Ok(())
    }

    #[test]
    fn rotated_sprite_vertices_match_original_dll() {
        // Vm60 10035d70 outputs for the SC483:0x9cb8 actor, plus camera
        // corner rotation and mixed actor rotations. These are numeric DLL
        // outputs, independent of this renderer and of proprietary textures.
        let actor = CharacterState {
            position: [960, -440, 600],
            ..Default::default()
        };
        let geometry = SpriteGeometry {
            size: [658, 2035],
            anchor_q16: [19077479, 120172484],
        };
        let camera = Camera {
            focal_length: 1440,
            position: [720, -400, -160],
            angles_q4: [1808, -432, -576],
        };
        for (projection, angles_q4, expected) in [
            (
                21,
                [0, 0, -432],
                [
                    [162, -270, 741],
                    [1439, -323, 741],
                    [1602, 3628, 741],
                    [325, 3681, 741],
                ],
            ),
            (
                29,
                [0; 3],
                [
                    [129, -256, 749],
                    [1259, -235, 863],
                    [1519, 3384, 798],
                    [325, 3966, 683],
                ],
            ),
            (
                29,
                [800, -400, 200],
                [
                    [205, -195, 845],
                    [1263, -196, 908],
                    [1567, 3536, 762],
                    [316, 3878, 698],
                ],
            ),
            (
                0,
                [800, -400, 200],
                [
                    [204, -183, 851],
                    [1364, -194, 800],
                    [1376, 3791, 725],
                    [102, 3543, 775],
                ],
            ),
        ] {
            let quad = actor.camera_quad(
                geometry,
                CharacterView {
                    camera: &camera,
                    projection,
                    player_height: 1550,
                    angles_q4,
                },
            );
            for (point, expected) in quad.into_iter().zip(expected) {
                let projected = [
                    512.0 + point[0] * 1440.0 / point[2],
                    300.0 + point[1] * 1440.0 / point[2],
                    point[2],
                ];
                for (actual, native) in projected.into_iter().zip(expected) {
                    assert!(
                        (actual - f64::from(native)).abs() < 1.0,
                        "mode {projection}, angles {angles_q4:?}: {projected:?} != {expected:?}"
                    );
                }
            }
        }
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_complete_core_composes_restores_pose_and_clears() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let scene = rugp_scene_vm::Scenario::decode_with_schema(
            &rugp_assets::script::Script::load(&game.archive, 483)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let instruction = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x79f8)
            .context("core instruction missing")?;
        let message = game.vm.message(instruction)?;
        let Value::Reference(target) = &message.target else {
            bail!("core target missing")
        };
        let target = target.clone();
        let id = target.offset.context("target identity missing")?;
        let Some(Value::Reference(core)) = message.fields.get("参照") else {
            bail!("core missing")
        };
        let body_ref = core
            .inline
            .as_ref()
            .context("inline core missing")?
            .references[2]
            .clone();
        let pose = core
            .inline
            .as_ref()
            .unwrap()
            .character_pose
            .as_deref()
            .copied()
            .context("core pose")?;
        let body = game.images.load(&body_ref)?;

        game.frame = Framebuffer::new(1024, 600)?;
        let object = game.objects.entry(id).or_default();
        object.rect = (0, 0, 1024, 600);
        object.character.position = [1, 0, 1120];
        object.properties.insert(
            "native.character.body".into(),
            Value::Reference(body_ref.clone()),
        );
        let before = game.frame().to_vec();
        game.message(message)?;
        assert!(
            !game.objects[&id]
                .properties
                .contains_key("native.character.body")
        );
        assert_eq!(game.frame(), before); // application is staged until presentation
        let character = &game.objects[&id].character;
        assert_eq!(character.position, [pose[0], pose[1], pose[2]]);
        for (name, expected) in ["ロール角量", "ピッチ角量", "バンク角量"]
            .into_iter()
            .zip(&pose[3..])
        {
            assert_eq!(number(&game.objects[&id].properties, name, 0)?, *expected);
        }
        let texture = character.texture.as_ref().context("core image missing")?;
        assert_eq!((texture.width, texture.height), (body.width, body.height));
        assert_ne!(texture.rgba, body.rgba); // real transparent face parts were composited
        game.present_scene()?;
        assert_ne!(game.frame(), before);
        let image = Arc::clone(game.objects[&id].character.texture.as_ref().unwrap());
        game.show_character(
            id,
            &BTreeMap::from([
                ("ベース画像".into(), Value::Null),
                ("３ＤＺ".into(), Value::Integer(2240)),
                ("スタイル".into(), Value::Integer(7)),
            ]),
        )?;
        assert!(Arc::ptr_eq(
            &image,
            game.objects[&id].character.texture.as_ref().unwrap()
        ));
        assert_eq!(game.objects[&id].character.position, [0, 0, 2240]);
        let pixels = game.frame().to_vec();
        game.message(Message {
            class: "OM_SetCharMouthAnimeSetting".into(),
            target: Value::Reference(target.clone()),
            fields: BTreeMap::from([
                ("スタイル".into(), Value::Integer(1)),
                ("口パク閾値1".into(), Value::Integer(20)),
            ]),
        })?;
        assert!(matches!(
            game.objects[&id].properties["render.mouth.スタイル"],
            Value::Integer(1)
        ));
        game.message(Message {
            class: "OM_SetCharBreath".into(),
            target: Value::Reference(target.clone()),
            fields: BTreeMap::from([
                ("呼吸モード".into(), Value::Integer(1)),
                ("スタイル".into(), Value::Integer(2)),
            ]),
        })?;
        assert!(matches!(
            game.objects[&id].properties["render.breath.スタイル"],
            Value::Integer(2)
        ));
        assert!(matches!(
            game.objects[&id].properties["render.mouth.スタイル"],
            Value::Integer(1)
        ));
        assert_eq!(game.objects[&id].character.style, 7);
        assert_eq!(game.objects[&id].character.position, [0, 0, 2240]);
        assert!(Arc::ptr_eq(
            &image,
            game.objects[&id].character.texture.as_ref().unwrap()
        ));
        assert_eq!(game.frame(), pixels);
        // Style selects replacement axes, relative axes and retained axes.
        game.show_character(
            id,
            &BTreeMap::from([
                ("ベース画像".into(), Value::Null),
                ("３ＤＸ".into(), Value::Integer(40)),
                ("３ＤＹ".into(), Value::Integer(5)),
                ("３ＤＺ".into(), Value::Integer(999)),
                ("スタイル".into(), Value::Integer(1 | 16)),
            ]),
        )?;
        assert_eq!(game.objects[&id].character.position, [40, 5, 2240]);
        let call = |key| Message {
            class: "OM_SetRefData".into(),
            target: Value::Reference(target.clone()),
            fields: BTreeMap::from([
                ("データ識別値".into(), Value::Integer(key)),
                ("参照".into(), Value::Null),
            ]),
        };
        assert!(game.message(call(21)).is_err());
        assert_eq!(game.frame(), pixels);
        game.message(call(20))?;
        assert!(matches!(
            game.objects[&id].properties["character.core"],
            Value::Null
        ));
        game.present_scene()?;
        assert_eq!(game.frame(), before);
        // A normal body packet replaces the complete core. Future face
        // packets must not change the identity of already applied parts.
        game.queue_character_face(id, &BTreeMap::from([("口".into(), Value::Null)]))?;
        game.show_character(
            id,
            &BTreeMap::from([
                ("ベース画像".into(), Value::Reference(body_ref.clone())),
                ("３ＤＺ".into(), Value::Integer(1120)),
                ("スタイル".into(), Value::Integer(519)),
            ]),
        )?;
        assert!(!game.objects[&id].properties.contains_key("character.core"));
        let Some(Value::Reference(applied)) =
            game.objects[&id].properties.get("native.character.body")
        else {
            bail!("applied body identity missing")
        };
        assert_eq!(applied.offset, body_ref.offset);
        game.queue_character_face(
            id,
            &BTreeMap::from([("口".into(), Value::Reference(body_ref.clone()))]),
        )?;
        assert!(matches!(
            game.objects[&id].properties["native.character.face.口"],
            Value::Null
        ));
        game.show_character(
            id,
            &BTreeMap::from([
                ("ベース画像".into(), Value::Null),
                ("３ＤＺ".into(), Value::Integer(2240)),
                ("スタイル".into(), Value::Integer(519)),
            ]),
        )?;
        assert!(matches!(
            game.objects[&id].properties["native.character.body"],
            Value::Reference(_)
        ));
        assert!(matches!(
            game.objects[&id].properties["native.character.３ＤＺ"],
            Value::Integer(2240)
        ));
        game.message(Message {
            class: "OM_HumanOut".into(),
            target: Value::Reference(target),
            fields: BTreeMap::from([("スタイル".into(), Value::Integer(512))]),
        })?;
        assert!(!game.objects[&id].character.has_image());
        assert!(
            !game.objects[&id]
                .properties
                .keys()
                .any(|name| name.starts_with("native.character."))
        );
        Ok(())
    }
    #[test]
    fn static_portraits_keep_depth_placement_removal_and_ui_order() -> Result<()> {
        let texture = Arc::new(Texture {
            width: 2,
            height: 4,
            display_scale: 1.0,
            rgba: [255, 0, 0, 255].repeat(8),
        });
        let mut portrait = ObjectState {
            rect: (0, 0, 16, 16),
            ..ObjectState::default()
        };
        portrait.character.texture = Some(texture);
        portrait.character.style = 512;
        portrait.character.geometry = Some(SpriteGeometry {
            size: [8, 16],
            anchor_q16: [4 << 16, 16 << 16],
        });
        portrait.character.position = [0, -1542, 720];
        portrait.parent = Some(3);
        // Child-local cameras and viewports must not affect world sprites.
        portrait.camera.position = [1000, 1000, 1000];
        portrait.rect = (100, 100, 2, 2);
        let world = ObjectState {
            rect: (0, 0, 16, 16),
            ..ObjectState::default()
        };
        let ui = ObjectState {
            rect: (0, 12, 16, 4),
            backdrop: Some(crate::message_window::Backdrop {
                color: [0, 0, 255],
                source: 256,
                destination_shift: 0,
            }),
            ..ObjectState::default()
        };
        let mut objects = BTreeMap::from([(1, ui), (2, portrait), (3, world)]);
        let frame = Game::render_objects(&objects, (16, 16))?;
        assert_eq!(&frame.rgba[(8 * 16 + 8) * 4..][..4], &[255, 0, 0, 255]);
        assert_eq!(&frame.rgba[(14 * 16 + 8) * 4..][..4], &[0, 0, 255, 255]);
        objects.get_mut(&3).unwrap().camera.position[0] = 4;
        let moved = Game::render_objects(&objects, (16, 16))?;
        assert_eq!(&moved.rgba[(8 * 16 + 4) * 4..][..4], &[255, 0, 0, 255]);
        assert_eq!(&moved.rgba[(8 * 16 + 9) * 4..][..4], &[0, 0, 0, 255]);
        objects.get_mut(&3).unwrap().camera = Camera::default();
        let character = &mut objects.get_mut(&2).unwrap().character;
        character.position[2] = 1440;
        character.remove(1);
        assert!(character.texture.is_some());
        let frame = Game::render_objects(&objects, (16, 16))?;
        assert_eq!(&frame.rgba[(3 * 16 + 8) * 4..][..4], &[0, 0, 0, 255]);
        objects.get_mut(&2).unwrap().character.remove(512);
        assert!(objects[&2].character.texture.is_none());
        Ok(())
    }
}
