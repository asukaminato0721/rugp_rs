//! Practical character presentation. Keep face/layer packets independently of
//! generic object properties: subsequent packets reuse many of the same names.
use super::*;

#[derive(Clone, Default)]
pub(super) struct CharacterState {
    pub(super) face: BTreeMap<String, Value>,
    pub(super) layers: BTreeMap<i32, [f32; 2]>,
    pub(super) texture: Option<Arc<Texture>>,
    pub(super) position: [i32; 3],
    pub(super) style: i32,
}
impl CharacterState {
    pub(super) fn validate_texture(&self) -> Result<()> {
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
        motion: &Camera,
    ) -> Result<()> {
        let Some(texture) = &self.texture else {
            return Ok(());
        };
        // Readable static standing portraits until native world-sprite
        // projection is ported. Preserve relative horizontal/depth placement.
        let position =
            std::array::from_fn::<_, 3, _>(|i| self.position[i].saturating_add(motion.position[i]));
        let depth = if position[2] > 0 { position[2] } else { 1120 } as f64;
        let height = (f64::from(viewport.3) * 1120.0 / depth * f64::from(motion.focal_length)
            / 720.0)
            .clamp(1.0, 16384.0);
        let width = height * f64::from(texture.width) / f64::from(texture.height);
        let x = f64::from(viewport.0)
            + f64::from(viewport.2) * 0.5
            + f64::from(position[0]) * 720.0 / depth
            - width * 0.5;
        let y = f64::from(viewport.1) + f64::from(viewport.3) - height
            + f64::from(position[1]) * 720.0 / depth;
        frame.blit(
            texture,
            (
                x.round() as i32,
                y.round() as i32,
                width.round().max(1.0) as u32,
                height.round() as u32,
            ),
            opacity,
        )
    }
    pub(super) fn remove(&mut self, style: i32) {
        if style == 0 || style == self.style {
            self.texture = None;
        }
    }
}
impl Game {
    fn character_texture(
        &mut self,
        body: &ObjectRef,
        overlays: &[ObjectRef],
    ) -> Result<Arc<Texture>> {
        let body = self.images.load(body)?;
        let mut canvas = Framebuffer::transparent(body.width, body.height)?;
        canvas.blit(&body, (0, 0, body.width, body.height), 255)?;
        for reference in overlays.iter().filter(|r| r.class.is_some()) {
            let part = self.images.load(reference)?;
            canvas.blit(&part, (0, 0, body.width, body.height), 255)?;
        }
        Ok(Arc::new(Texture {
            width: canvas.width,
            height: canvas.height,
            display_scale: 1.0,
            rgba: canvas.rgba,
        }))
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
        // Static expression fallback: all supplied transparent face canvases.
        // Native core pose/deformation and eye/mouth animation remain deferred.
        let texture = self.character_texture(body, &core.references[body_index + 1..])?;
        self.buffers.remove(&id);
        let object = self.objects.entry(id).or_default();
        object.layers.clear();
        object.menu = None;
        object.character.texture = Some(texture);
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
        let position = [
            number(fields, "３ＤＸ", 0)?,
            number(fields, "３ＤＹ", 0)?,
            number(fields, "３ＤＺ", 0)?,
        ];
        let style = number(fields, "スタイル", 0)?;
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
        let texture = self.character_texture(body, &overlays)?;
        let object = self.objects.entry(id).or_default();
        object.layers.clear();
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
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_complete_core_composes_and_clears_without_resetting_live_placement() -> Result<()> {
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
        let body = game.images.load(&body_ref)?;
        game.frame = Framebuffer::new(16, 16)?;
        let object = game.objects.entry(id).or_default();
        object.rect = (0, 0, 16, 16);
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
        assert_eq!(character.position, [1, 0, 1120]);
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
        assert_eq!(game.objects[&id].character.style, 0);
        assert_eq!(game.objects[&id].character.position, [0, 0, 2240]);
        assert!(Arc::ptr_eq(
            &image,
            game.objects[&id].character.texture.as_ref().unwrap()
        ));
        assert_eq!(game.frame(), pixels);
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
                ("スタイル".into(), Value::Integer(512)),
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
                ("スタイル".into(), Value::Integer(512)),
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
        let ui = ObjectState {
            rect: (0, 12, 16, 4),
            backdrop: Some(crate::message_window::Backdrop {
                color: [0, 0, 255],
                source: 256,
                destination_shift: 0,
            }),
            ..ObjectState::default()
        };
        let mut objects = BTreeMap::from([(1, ui), (2, portrait)]);
        let frame = Game::render_objects(&objects, (16, 16))?;
        assert_eq!(&frame.rgba[(8 * 16 + 8) * 4..][..4], &[255, 0, 0, 255]);
        assert_eq!(&frame.rgba[(14 * 16 + 8) * 4..][..4], &[0, 0, 255, 255]);
        objects.get_mut(&2).unwrap().camera.position[0] = 4;
        let moved = Game::render_objects(&objects, (16, 16))?;
        assert_eq!(&moved.rgba[(8 * 16 + 4) * 4..][..4], &[0, 0, 0, 255]);
        objects.get_mut(&2).unwrap().camera = Camera::default();
        let character = &mut objects.get_mut(&2).unwrap().character;
        character.position[2] = 2240;
        character.remove(1);
        assert!(character.texture.is_some());
        let frame = Game::render_objects(&objects, (16, 16))?;
        assert_eq!(&frame.rgba[(4 * 16 + 8) * 4..][..4], &[0, 0, 0, 255]);
        objects.get_mut(&2).unwrap().character.remove(512);
        assert!(objects[&2].character.texture.is_none());
        Ok(())
    }
}
