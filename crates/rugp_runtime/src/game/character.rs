//! Practical character presentation. Keep face/layer packets independently of
//! generic object properties: subsequent packets reuse many of the same names.
use super::*;

#[derive(Clone, Default)]
pub(super) struct CharacterState {
    face: BTreeMap<String, Value>,
    layers: BTreeMap<i32, [f32; 2]>,
    texture: Option<Arc<Texture>>,
    position: [i32; 3],
    style: i32,
}
impl CharacterState {
    pub(super) fn has_image(&self) -> bool {
        self.texture.is_some()
    }
    pub(super) fn render(
        &self,
        frame: &mut Framebuffer,
        viewport: (i32, i32, u32, u32),
        opacity: u8,
    ) -> Result<()> {
        let Some(texture) = &self.texture else {
            return Ok(());
        };
        // Readable static standing portraits until native world-sprite
        // projection is ported. Preserve relative horizontal/depth placement.
        let depth = if self.position[2] > 0 {
            self.position[2]
        } else {
            1120
        } as f64;
        let height = (f64::from(viewport.3) * 1120.0 / depth).clamp(1.0, 16384.0);
        let width = height * f64::from(texture.width) / f64::from(texture.height);
        let x = f64::from(viewport.0)
            + f64::from(viewport.2) * 0.5
            + f64::from(self.position[0]) * 720.0 / depth
            - width * 0.5;
        let y = f64::from(viewport.1) + f64::from(viewport.3) - height
            + f64::from(self.position[1]) * 720.0 / depth;
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
    pub(super) fn show_character(
        &mut self,
        id: u64,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let Some(Value::Reference(body)) = fields.get("ベース画像") else {
            bail!("character message has no bound body");
        };
        let body = self.images.load(body)?;
        let mut canvas = Framebuffer::transparent(body.width, body.height)?;
        canvas.blit(&body, (0, 0, body.width, body.height), 255)?;
        let face = self
            .objects
            .get(&id)
            .map(|o| o.character.face.clone())
            .unwrap_or_default();
        for name in ["眼ベース", "左眼", "右眼", "眼", "眉", "口", "頬", "その他"] {
            if let Some(Value::Reference(part)) = face.get(name) {
                let part = self.images.load(part)?;
                // Retail CRmt parts share the complete transparent body canvas.
                canvas.blit(&part, (0, 0, body.width, body.height), 255)?;
            }
        }
        if let Some(Value::Reference(expression)) = fields.get("表情画像") {
            let expression = self.images.load(expression)?;
            canvas.blit(&expression, (0, 0, body.width, body.height), 255)?;
        }
        let position = [
            number(fields, "３ＤＸ", 0)?,
            number(fields, "３ＤＹ", 0)?,
            number(fields, "３ＤＺ", 0)?,
        ];
        let style = number(fields, "スタイル", 0)?;
        let object = self.objects.entry(id).or_default();
        object.layers.clear();
        object.menu = None;
        self.buffers.remove(&id);
        let character = &mut object.character;
        character.texture = Some(Arc::new(Texture {
            width: canvas.width,
            height: canvas.height,
            display_scale: 1.0,
            rgba: canvas.rgba,
        }));
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

#[cfg(test)]
mod tests {
    use super::*;
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
