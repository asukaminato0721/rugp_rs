//! Camera commands prepare absolute targets; modes 4/8 use ParseScreen time.
//!
//! Vm60 1004046a sets flags 0x20 for mode 4, whereas 10040483 sets
//! 0x21 for mode 8. Both reach 10044f90 via vtable +1cc from
//! 10044b2a..10044b3d with the same ParseScreen progress, then share
//! 10046d3a interpolation. The bit-1 difference selects native window backing
//! caches (1004348e), not a separate camera timer. Our full-scene redraw does
//! not need those incremental window caches; staged logical snapshots remain
//! necessary in both modes.
use super::*;
use crate::world::CameraTween;

/// Bit 0 only keeps requested angles unclamped (`prepare_camera`); the
/// axis-aligned projection of modes 0..3 ignores the low bits otherwise.
pub(super) fn validate_world_projection(mode: u8) -> Result<()> {
    ensure!(
        [0, 1, 2, 3, 17, 19, 21, 25, 27, 29].contains(&mode),
        "unsupported world projection mode {mode}"
    );
    Ok(())
}

impl ObjectState {
    pub(super) fn begin_world_projection(&mut self) {
        // Vm60 10042442 switches only in states 2/3, armed by HumanIn.
        // State 0 preserves the current mode across camera-only transitions.
        // State 1 discards an override that had no following world image.
        if self.world_projection_refresh {
            self.world_projection = self
                .pending_world_projection
                .unwrap_or(self.default_world_projection);
        }
        self.pending_world_projection = None;
        self.world_projection_refresh = false;
    }
    pub(super) fn camera_target(&self) -> Result<Camera> {
        self.pending_camera
            .as_ref()
            .map_or(Ok(self.camera), |t| self.prepare_camera(t.target()))
    }
    pub(super) fn prepare_camera(&self, mut camera: Camera) -> Result<Camera> {
        // Vm60 10040525: exclusive bit 0 preserves all requested angles.
        let mode = if self.world_projection_refresh {
            self.pending_world_projection
                .unwrap_or(self.default_world_projection)
        } else {
            self.world_projection
        };
        if mode & 1 != 0 {
            return Ok(camera);
        }
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
    pub(super) fn set_world_projection(
        &mut self,
        id: u64,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let mode = number(fields, "排他指定", 0)?;
        ensure!(mode & !0x100ff == 0, "invalid world projection mode");
        let projection = mode as u8;
        validate_world_projection(projection)?;
        ensure!(
            matches!(fields.get("独立カメラ"), None | Some(Value::Null)),
            "independent world camera is not implemented"
        );
        let object = self.objects.entry(id).or_default();
        if mode & 0x10000 != 0 {
            object.pending_world_projection = Some(projection);
            object.world_projection_refresh = false;
        } else {
            object.world_projection = projection;
            object.default_world_projection = projection;
        }
        Ok(())
    }

    /// Independent cameras used by weather effects retain their named
    /// scalar settings. They are not the background's live ParseScreen camera.
    pub(super) fn set_effect_camera_member(
        &mut self,
        id: u64,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let Some(Value::Text(name)) = fields.get("メンバ名") else {
            bail!("camera member name is not text");
        };
        ensure!(
            [
                "X",
                "Y",
                "Z",
                "ロール角",
                "ピッチ角",
                "バンク角",
                "screen",
                "プレイヤー身長補正",
                "SX",
                "SY"
            ]
            .contains(&name.as_str()),
            "unsupported independent camera member {name}"
        );
        let value = fields.get("値").context("camera member value missing")?;
        ensure!(
            matches!(value, Value::Integer(_)) || matches!(value, Value::Real(n) if n.is_finite()),
            "camera member value is not a finite number"
        );
        self.objects
            .entry(id)
            .or_default()
            .properties
            .insert(format!("member.{name}"), value.clone());
        Ok(())
    }
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
            matches!(style, 0..=15 | 0x11 | 0x800) && matches!(fade, 0 | 4 | 8 | 12),
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
        } else if matches!(fade, 4 | 8 | 12) {
            // Mode 12 shares the portable absolute interpolation clock. Its
            // native spatial/effect presentation is intentionally approximate.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_override_survives_camera_transitions_until_next_world_image() {
        let mut object = ObjectState {
            world_projection: 17,
            default_world_projection: 17,
            pending_world_projection: Some(21),
            world_projection_refresh: true,
            ..Default::default()
        };
        let previous_scene = object.clone();
        object.begin_world_projection();
        assert_eq!(object.world_projection, 21);
        assert_eq!(previous_scene.world_projection, 17);
        assert_eq!(object.pending_world_projection, None);
        object.begin_world_projection();
        assert_eq!(object.world_projection, 21);
        // A new image without an override restores the default mode.
        object.world_projection_refresh = true;
        object.begin_world_projection();
        assert_eq!(object.world_projection, 17);
        assert_eq!(object.default_world_projection, 17);
    }
}
