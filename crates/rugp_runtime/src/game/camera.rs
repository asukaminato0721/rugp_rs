//! Camera commands prepare absolute targets; mode 8 uses ParseScreen time.
use super::*;
use crate::world::CameraTween;

impl ObjectState {
    pub(super) fn camera_target(&self) -> Result<Camera> {
        self.pending_camera
            .as_ref()
            .map_or(Ok(self.camera), |t| self.prepare_camera(t.target()))
    }
    pub(super) fn prepare_camera(&self, mut camera: Camera) -> Result<Camera> {
        for image in self.layers.values().filter_map(|layer| match layer {
            Layer::World(image) => Some(image),
            Layer::Screen(_) => None,
        }) {
            if let Some(first) = image.planes.first() {
                camera = image.boundary.clamp_camera(
                    camera,
                    self.rect,
                    first.texture.width,
                    self.camera_center_y.get(),
                )?;
            }
        }
        Ok(camera)
    }
}
impl Game {
    pub(super) fn set_camera(
        &mut self,
        id: u64,
        fields: BTreeMap<String, Value>,
        screen: bool,
    ) -> Result<()> {
        ensure!(
            !self
                .parse_transition
                .as_ref()
                .is_some_and(|p| p.scene.contains_key(&id)),
            "camera replacement during an active ParseScreen transition is not implemented yet"
        );
        let style = number(&fields, "スタイル", 0)?;
        let fade = number(&fields, "フェード方式", 0)?;
        ensure!(
            matches!(style, 0..=15 | 0x800) && matches!(fade, 0 | 8),
            "camera style/transition is not implemented yet"
        );
        number(&fields, "総処理時間基", 0)?; // synchronized mode uses the ParseScreen clock
        let object = self.objects.get(&id);
        let mut target = object.map_or(Ok(Camera::default()), ObjectState::camera_target)?;
        if screen {
            ensure!(
                number(&fields, "スクリーン座標Ｘ", 0)? == 0
                    && number(&fields, "スクリーン座標Ｙ", 0)? == 0,
                "translated screen camera is not implemented yet"
            );
            let focal = number(&fields, "ズーム率", 720)?;
            ensure!(
                (0..=32767).contains(&focal),
                "invalid camera focal length {focal}"
            );
            target.focal_length = focal as u16;
        } else {
            for (out, field) in target
                .position
                .iter_mut()
                .zip(["３ＤＸ", "３ＤＹ", "３ＤＺ"])
            {
                *out = number(&fields, field, 0)?;
            }
            for (out, name) in target
                .angles_q4
                .iter_mut()
                .zip(["ロール角", "ピッチ角", "バンク角"])
            {
                *out = number(&fields, name, 0)?
                    .checked_mul(16)
                    .context("camera angle overflow")?;
            }
        }
        if let Some(object) = object {
            target = object.prepare_camera(target)?;
        }
        let pending = if style == 0x800 {
            object
                .and_then(|o| o.pending_camera)
                .map(|t| t.rebase_start(target))
                .transpose()?
        } else if fade == 8 {
            let start = object.map_or(Camera::default(), |o| o.camera_start.unwrap_or(o.camera));
            Some(CameraTween::new(start, target, style as u8)?)
        } else {
            None
        };
        let object = self.objects.entry(id).or_default();
        if style == 0x800 {
            object.camera_start = Some(target);
        } else if fade == 0 {
            object.camera = target;
        }
        object.pending_camera = pending;
        object.properties.extend(fields);
        Ok(())
    }
}
