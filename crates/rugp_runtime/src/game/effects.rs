//! Window effects drawn over a parse screen after its world and actors:
//! Xs raster distortion, OM_SetWeatherRainSnow particles and
//! CLayerSimpleFlush flashes; plus the OM_SetSpecifyCmdLighting tint of the
//! actors standing in that window (the background art is already lit).
//!
//! Parameters, their units and their call sites are retail (see the message
//! audit); the native particle/raster/flash kernels are not ported, so the
//! pixels are portable approximations driven by those parameters. Every
//! effect is a pure function of saved object properties and a per-window
//! effect clock, so snapshots need no extra state.
use super::*;

const CLOCK: &str = "render.effect_clock";
/// Layer holding a window's frozen image while an Xs effect distorts it;
/// below every script layer.
pub(super) const XS_FROZEN_LAYER: i32 = i32::MIN;
const TAU: f64 = std::f64::consts::TAU;

/// Retail colors are 0xAARRGGBB DWORDs (as Setup.ini nWndBaseColor1), seen
/// either as an integer or as their little-endian bytes. Returns RGBA.
fn color(value: Option<&Value>) -> Option<[u8; 4]> {
    match value? {
        Value::Integer(n) => {
            let n = *n as u32;
            Some([(n >> 16) as u8, (n >> 8) as u8, n as u8, (n >> 24) as u8])
        }
        Value::Bytes(b) if b.len() == 4 => Some([b[2], b[1], b[0], b[3]]),
        _ => None,
    }
}

/// Deterministic per-particle randomness in [0, 1).
fn unit(seed: u64) -> f64 {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0xd1b5_4a32_d192_ed03;
    x ^= x >> 31;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 29;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

fn get(properties: &BTreeMap<String, Value>, key: &str) -> i32 {
    number(properties, key, 0).unwrap_or(0)
}

impl ObjectState {
    fn effect_clock(&self) -> u32 {
        get(&self.properties, CLOCK).max(0) as u32
    }
    fn weather_strength(&self) -> f64 {
        let p = &self.properties;
        let target = f64::from(get(p, "render.weather.粒の強さ[.8]")) / 256.0;
        let from = f64::from(get(p, "render.weather_ramp.from")) / 256.0;
        let start = get(p, "render.weather_ramp.start").max(0) as u32;
        let duration = get(p, "render.weather.変化時間");
        let progress = if duration <= 0 {
            1.0
        } else {
            (f64::from(self.effect_clock().saturating_sub(start)) / f64::from(duration)).min(1.0)
        };
        from + (target - from) * progress
    }
    fn weather_kind(&self) -> Option<bool> {
        // Rain (0x10000: the rainy Tanabata of SC481, the shower of SC280)
        // and snow (0x8000), per the retail call sites' dialogue.
        let style = get(&self.properties, "render.weather.スタイル");
        if style & 0x10000 != 0 {
            Some(true)
        } else if style & 0x8000 != 0 {
            Some(false)
        } else {
            None
        }
    }
    /// The window's actor lighting now: an RGB factor in 0..=255 and whether
    /// it multiplies (カラーオペレーション 1) or adds.
    pub(super) fn actor_tint(&self) -> Option<([f64; 3], bool)> {
        let p = &self.properties;
        let first = color(p.get("render.lighting.色1"))?;
        let second = color(p.get("render.lighting.色2"))?;
        // 周期 is in 10 ms units; 振幅 shimmers each channel around the mix.
        let period = f64::from(get(p, "render.lighting.周期").max(1)) * 10.0;
        let wave = (TAU * f64::from(self.effect_clock()) / period).sin();
        let amplitude = f64::from(get(p, "render.lighting.振幅"));
        let tint = std::array::from_fn(|i| {
            let mixed = f64::from(first[i])
                + (f64::from(second[i]) - f64::from(first[i])) * (1.0 + wave) / 2.0;
            (mixed + amplitude * wave).clamp(0.0, 255.0)
        });
        Some((tint, get(p, "render.lighting.カラーオペレーション") != 2))
    }
    fn lighting_animates(&self) -> bool {
        self.properties.contains_key("render.lighting.色1")
            && get(&self.properties, "render.lighting.振幅") != 0
    }
    fn effects_animate(&self) -> bool {
        (self.weather_kind().is_some() && self.weather_strength() > 0.0)
            || self.lighting_animates()
            || self.properties.contains_key("render.xs_fade.class")
            || self.properties.contains_key("render.flush.start")
    }
    fn has_effects(&self) -> bool {
        self.effects_animate() || self.properties.contains_key("render.lighting.色1")
    }
    /// Replace the weather, ramping from the strength shown now.
    fn set_weather(&mut self, fields: BTreeMap<String, Value>) {
        let from = if self.weather_kind().is_some() {
            (self.weather_strength() * 256.0).round() as i32
        } else {
            0
        };
        let start = self.effect_clock() as i32;
        self.properties
            .retain(|key, _| !key.starts_with("render.weather."));
        for (key, value) in fields {
            self.properties
                .insert(format!("render.weather.{key}"), value);
        }
        self.properties
            .insert("render.weather_ramp.from".into(), Value::Integer(from));
        self.properties
            .insert("render.weather_ramp.start".into(), Value::Integer(start));
    }
}

impl Game {
    /// Effect parameters start from the current state; the clock is the
    /// window's own effect time.
    pub(super) fn effect_start(&self, id: u64) -> i32 {
        self.objects.get(&id).map_or(0, |o| o.effect_clock() as i32)
    }
    pub(super) fn set_weather(&mut self, id: u64, fields: BTreeMap<String, Value>) -> Result<()> {
        // The packet does not block its caller; explicit VM timer waits still
        // execute. Callback handles need a completion implementation.
        ensure!(
            matches!(fields.get("非同期ハンドル"), None | Some(Value::Null)),
            "weather completion handles are not implemented"
        );
        self.objects.entry(id).or_default().set_weather(fields);
        Ok(())
    }
    /// OM_SetSpecifyCmdLighting configures the next background install of
    /// its window ("specify command"); see [`Self::install_lighting`].
    pub(super) fn set_lighting(&mut self, id: u64, fields: BTreeMap<String, Value>) {
        let properties = &mut self.objects.entry(id).or_default().properties;
        properties.retain(|key, _| !key.starts_with("render.lighting_pending."));
        for (key, value) in fields {
            properties.insert(format!("render.lighting_pending.{key}"), value);
        }
    }
    /// A background install takes the pending lighting, or drops the
    /// previous background's lighting.
    pub(super) fn install_lighting(&mut self, id: u64) {
        let Some(object) = self.objects.get_mut(&id) else {
            return;
        };
        let pending: Vec<_> = object
            .properties
            .iter()
            .filter_map(|(key, value)| {
                key.strip_prefix("render.lighting_pending.")
                    .map(|name| (format!("render.lighting.{name}"), value.clone()))
            })
            .collect();
        object.properties.retain(|key, _| {
            !key.starts_with("render.lighting.") && !key.starts_with("render.lighting_pending.")
        });
        object.properties.extend(pending);
    }
    /// CLayerSimpleFlush OM_PlayMedia: flash the window named by var 0 with
    /// color 0, rising over var 1 and falling over var 2 (retail macros pass
    /// 50/50 ms, or a single total in var 16).
    pub(super) fn play_flush(&mut self, layer: u64) -> Result<()> {
        let Some(properties) = self.objects.get(&layer).map(|o| o.properties.clone()) else {
            return Ok(());
        };
        let Some(Value::Reference(window)) = properties.get("render.var.0") else {
            return Ok(());
        };
        let Some(window) = window.offset else {
            return Ok(());
        };
        let Some(color) = color(properties.get("render.color.0")) else {
            return Ok(());
        };
        let total = get(&properties, "render.var.16").clamp(0, 10_000);
        let rise = properties
            .get("render.var.1")
            .map_or(total / 2, |_| get(&properties, "render.var.1"))
            .clamp(0, 10_000);
        let fall = properties
            .get("render.var.2")
            .map_or(total - total / 2, |_| get(&properties, "render.var.2"))
            .clamp(0, 10_000);
        let start = self.effect_start(window);
        let target = self.objects.entry(window).or_default();
        for (key, value) in [
            (
                "color",
                i32::from_le_bytes([color[2], color[1], color[0], color[3]]),
            ),
            ("rise", rise),
            ("fall", fall.max(1)),
            ("start", start),
        ] {
            target
                .properties
                .insert(format!("render.flush.{key}"), Value::Integer(value));
        }
        Ok(())
    }
    /// Advance window effect clocks, retire finished flashes and stopped
    /// weather, and redraw while something animates.
    pub(super) fn advance_effects(&mut self, dt_ms: u32) -> Result<()> {
        let mut animate = false;
        for object in self.objects.values_mut().filter(|o| o.has_effects()) {
            let clock = object
                .effect_clock()
                .saturating_add(dt_ms)
                .min(i32::MAX as u32);
            object
                .properties
                .insert(CLOCK.into(), Value::Integer(clock as i32));
            let p = &object.properties;
            if p.contains_key("render.flush.start") {
                let elapsed = clock.saturating_sub(get(p, "render.flush.start").max(0) as u32);
                let length = (get(p, "render.flush.rise") + get(p, "render.flush.fall")) as u32;
                if elapsed > length {
                    object
                        .properties
                        .retain(|key, _| !key.starts_with("render.flush."));
                }
            }
            if object.weather_kind().is_some() && object.weather_strength() <= 0.0 {
                let start = get(&object.properties, "render.weather_ramp.start").max(0) as u32;
                if clock.saturating_sub(start)
                    >= get(&object.properties, "render.weather.変化時間").max(0) as u32
                {
                    object.properties.retain(|key, _| {
                        !key.starts_with("render.weather.")
                            && !key.starts_with("render.weather_ramp.")
                    });
                }
            }
            animate |= object.visible && object.effects_animate();
        }
        let video = self.video.as_ref().is_some_and(|(_, v)| !v.finished());
        if animate && self.rasterize_scene && !video {
            self.present_scene()?;
        }
        Ok(())
    }
}

/// Draw a window's effects over its already composed scene.
pub(super) fn render_effects(
    object: &ObjectState,
    objects: &BTreeMap<u64, ObjectState>,
    frame: &mut Framebuffer,
) {
    if !object.has_effects() {
        return;
    }
    let clock = object.effect_clock();
    let rect = clip_rect(object.rect, frame);
    let Some(rect) = rect else {
        return;
    };
    distortion(object, frame, rect, clock);
    weather(object, objects, frame, clock);
    flush(object, frame, rect, clock);
}

/// The window rectangle intersected with the frame, as [x0, y0, x1, y1).
fn clip_rect((x, y, w, h): (i32, i32, u32, u32), frame: &Framebuffer) -> Option<[usize; 4]> {
    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = (i64::from(x) + i64::from(w)).clamp(0, i64::from(frame.width)) as usize;
    let y1 = (i64::from(y) + i64::from(h)).clamp(0, i64::from(frame.height)) as usize;
    (x0 < x1 && y0 < y1).then_some([x0, y0, x1, y1])
}

/// OM_SetShading on a character box: blend the actor toward 色 by
/// シェード濃度 (8.8). The retail macro (SC33) passes 0xffff as its time,
/// applied at once; other durations are also applied at once.
pub(super) fn actor_shade(object: &ObjectState) -> Option<([f64; 3], f64)> {
    let density = f64::from(get(&object.properties, "シェード濃度[.8]")) / 256.0;
    if density <= 0.0 {
        return None;
    }
    let rgba = color(object.properties.get("色")).unwrap_or([0; 4]);
    Some((
        [f64::from(rgba[0]), f64::from(rgba[1]), f64::from(rgba[2])],
        density.min(1.0),
    ))
}

/// Blend an actor layer toward a shading colour.
pub(super) fn shade_layer(layer: &mut Framebuffer, (rgb, density): ([f64; 3], f64)) {
    for pixel in layer
        .rgba
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .filter(|p| p[3] != 0)
    {
        for (c, &target) in rgb.iter().enumerate() {
            let v = f64::from(pixel[c]);
            pixel[c] = (v + (target - v) * density).round() as u8;
        }
    }
}

/// Apply an actor tint to a separately rendered actor layer.
pub(super) fn tint_layer(layer: &mut Framebuffer, (tint, multiply): ([f64; 3], bool)) {
    for pixel in layer
        .rgba
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .filter(|p| p[3] != 0)
    {
        for (c, &t) in tint.iter().enumerate() {
            let v = f64::from(pixel[c]);
            pixel[c] = if multiply {
                (v * t / 255.0).round() as u8
            } else {
                (v + t).min(255.0) as u8
            };
        }
    }
}

/// Xs window effects run until OM_StopXsFade; their alpha curve ramps the
/// amplitude in over the requested duration.
fn distortion(object: &ObjectState, frame: &mut Framebuffer, rect: [usize; 4], clock: u32) {
    let p = &object.properties;
    let Some(Value::Text(class)) = p.get("render.xs_fade.class") else {
        return;
    };
    let start = get(p, "render.xs_fade.start").max(0) as u32;
    let duration = get(p, "render.xs_fade.duration").max(1);
    let elapsed = clock.saturating_sub(start);
    let ramp = (f64::from(elapsed) / f64::from(duration)).min(1.0);
    let amplitude = 10.0 * ramp * ramp;
    if amplitude < 0.5 {
        return;
    }
    let t = f64::from(elapsed) / 1000.0;
    let [x0, y0, x1, y1] = rect;
    let (w, h) = (x1 - x0, y1 - y0);
    let stride = frame.width as usize;
    let source: Vec<u8> = (y0..y1)
        .flat_map(|y| frame.rgba[(y * stride + x0) * 4..(y * stride + x1) * 4].to_vec())
        .collect();
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let noise_frame = (t * 20.0) as u64;
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f64, y as f64);
            let (sx, sy) = match class.as_str() {
                "CFadeXsHRasterHOffset" => {
                    (fx + amplitude * (TAU * (fy / 48.0 + t * 1.5)).sin(), fy)
                }
                "CFadeXsSqrRaster_HRasterV_VRasterH" => (
                    fx + amplitude * (TAU * (fy / 64.0 + t)).sin(),
                    fy + amplitude * (TAU * (fx / 64.0 + t)).sin(),
                ),
                "CFadeXsCircleRaster" => {
                    let (dx, dy) = (fx - cx, fy - cy);
                    let r = dx.hypot(dy).max(1.0);
                    let shift = amplitude * (TAU * (r / 40.0 - t * 1.5)).sin();
                    (fx + dx / r * shift, fy + dy / r * shift)
                }
                "CFadeXsRasterNoize" => (
                    fx + amplitude * (unit((y as u64 / 2) ^ noise_frame << 20) * 2.0 - 1.0),
                    fy,
                ),
                _ => return,
            };
            let sx = sx.round().clamp(0.0, (w - 1) as f64) as usize;
            let sy = sy.round().clamp(0.0, (h - 1) as f64) as usize;
            let from = (sy * w + sx) * 4;
            let to = ((y0 + y) * stride + x0 + x) * 4;
            frame.rgba[to..to + 4].copy_from_slice(&source[from..from + 4]);
        }
    }
}

/// 粒の強さ (8.8) particles in a camera-space volume between Zレンジ近/遠,
/// falling along the world vertical: a rotating window camera (projection
/// modes 0x30) turns that into perspective streaks, as the retail sky shots
/// show. 粒の明度 (8.8) is their opacity; 変化時間 ramps the strength.
/// OM_SetWeatherRainSnow's 独立投影カメラ: a CNormalCamera whose members
/// (ONM_SetMenberAsName) give focal length, projection centre and angles.
fn independent_camera(
    object: &ObjectState,
    objects: &BTreeMap<u64, ObjectState>,
) -> Option<(crate::world::Camera, [f64; 2])> {
    let Some(Value::Reference(camera)) = object.properties.get("render.weather.独立投影カメラ")
    else {
        return None;
    };
    let members = &objects.get(&camera.offset?)?.properties;
    let member = |name: &str| get(members, &format!("member.{name}"));
    members.contains_key("member.screen").then(|| {
        // Members hold the native angle units directly (1/65536 turn);
        // SC155's sky shot pitches -12000 (about 66° up), as retail shows.
        (
            crate::world::Camera {
                focal_length: member("screen").clamp(1, 32767) as u16,
                position: [member("X"), member("Y"), member("Z")],
                angles_q4: [member("ロール角"), member("ピッチ角"), member("バンク角")],
            },
            [f64::from(member("SX")), f64::from(member("SY"))],
        )
    })
}

fn weather(
    object: &ObjectState,
    objects: &BTreeMap<u64, ObjectState>,
    frame: &mut Framebuffer,
    clock: u32,
) {
    let Some(rain) = object.weather_kind() else {
        return;
    };
    let strength = object.weather_strength();
    if strength <= 0.0 {
        return;
    }
    let p = &object.properties;
    let brightness = f64::from(get(p, "render.weather.粒の明度[.8]")) / 256.0;
    let opacity = if brightness > 0.0 {
        brightness.min(1.0)
    } else {
        0.5
    };
    let near = f64::from(get(p, "render.weather.Zレンジ近").max(64));
    let mut far = f64::from(get(p, "render.weather.Zレンジ遠"));
    if far <= near {
        far = near + 6000.0;
    }
    let (rx, ry, rw, rh) = object.rect;
    let clip = [rx, ry, rx + rw as i32, ry + rh as i32];
    let independent = independent_camera(object, objects);
    let (camera, rotating, (cx, cy)) = match &independent {
        Some((camera, center)) => (
            camera,
            true,
            (f64::from(rx) + center[0], f64::from(ry) + center[1]),
        ),
        None => (
            &object.camera,
            object.world_projection & 0x30 != 0,
            (
                f64::from(rx) + f64::from(rw) / 2.0,
                f64::from(ry) + f64::from(rh) / 2.0,
            ),
        ),
    };
    let focal = f64::from(camera.focal_length.max(1));
    // World fall velocity (mm/s; +y is down), into camera space.
    let speed = if rain { 7000 } else { 700 };
    let world = [if rain { speed / 12 } else { 0 }, speed, 0];
    let velocity = if rotating {
        camera.rotate(world)
    } else {
        world
    }
    .map(f64::from);
    // A box covering the view at the far plane, wrapped on every axis.
    let half = [
        f64::from(rw) / 2.0 * far / focal * 1.2,
        f64::from(rh) / 2.0 * far / focal * 1.2,
    ];
    let low = [-half[0], -half[1], near];
    let span = [2.0 * half[0], 2.0 * half[1], far - near];
    let count = (strength * if rain { 260.0 } else { 60.0 }).min(8000.0) as u64;
    let t = f64::from(clock) / 1000.0;
    let project = |q: [f64; 3]| {
        (q[2] > near * 0.5).then(|| [cx + q[0] * focal / q[2], cy + q[1] * focal / q[2]])
    };
    for i in 0..count {
        let r = |k: u64| unit(i * 8 + k);
        let mut q: [f64; 3] = std::array::from_fn(|axis| {
            low[axis] + (r(axis as u64) * span[axis] + velocity[axis] * t).rem_euclid(span[axis])
        });
        if !rain {
            q[0] += (t * (0.8 + r(4)) + r(5) * TAU).sin() * 120.0;
        }
        let Some(head) = project(q) else {
            continue;
        };
        let depth = (q[2] - near) / (far - near);
        let alpha = 255.0 * opacity * (1.0 - 0.5 * depth);
        if rain {
            // Motion blur over ~1/20 s, like the retail streaks.
            let tail = std::array::from_fn(|axis| q[axis] - velocity[axis] / 20.0);
            let Some(tail) = project(tail) else {
                continue;
            };
            line(frame, clip, tail, head, [225, 230, 238], alpha * 0.65);
        } else {
            let radius = (12.0 * focal / q[2]).clamp(0.6, 5.0);
            disc(frame, clip, head, radius, alpha);
        }
    }
}

fn disc(frame: &mut Framebuffer, clip: [i32; 4], center: [f64; 2], radius: f64, alpha: f64) {
    let reach = radius.ceil() as i32 + 1;
    let (px, py) = (center[0].floor() as i32, center[1].floor() as i32);
    for y in py - reach..=py + reach {
        for x in px - reach..=px + reach {
            if x < clip[0] || y < clip[1] || x >= clip[2] || y >= clip[3] {
                continue;
            }
            let d = (f64::from(x) + 0.5 - center[0]).hypot(f64::from(y) + 0.5 - center[1]);
            let coverage = (1.0 - d / (radius + 0.5)).clamp(0.0, 1.0);
            let a = (alpha * coverage).round() as u8;
            if a > 0 {
                frame.pixel(x, y, [255, 255, 255, a]);
            }
        }
    }
}

fn line(
    frame: &mut Framebuffer,
    clip: [i32; 4],
    from: [f64; 2],
    to: [f64; 2],
    rgb: [u8; 3],
    alpha: f64,
) {
    let steps = (to[0] - from[0])
        .abs()
        .max((to[1] - from[1]).abs())
        .ceil()
        .clamp(1.0, 400.0) as i32;
    let a = alpha.round().clamp(0.0, 255.0) as u8;
    for s in 0..=steps {
        let k = f64::from(s) / f64::from(steps);
        let x = (from[0] + (to[0] - from[0]) * k).floor() as i32;
        let y = (from[1] + (to[1] - from[1]) * k).floor() as i32;
        if x >= clip[0] && y >= clip[1] && x < clip[2] && y < clip[3] {
            frame.pixel(x, y, [rgb[0], rgb[1], rgb[2], a]);
        }
    }
}

fn flush(object: &ObjectState, frame: &mut Framebuffer, rect: [usize; 4], clock: u32) {
    let p = &object.properties;
    let Some(rgba) = color(p.get("render.flush.color")) else {
        return;
    };
    let elapsed = f64::from(clock.saturating_sub(get(p, "render.flush.start").max(0) as u32));
    let rise = f64::from(get(p, "render.flush.rise"));
    let fall = f64::from(get(p, "render.flush.fall").max(1));
    let envelope = if elapsed < rise {
        elapsed / rise
    } else {
        1.0 - ((elapsed - rise) / fall).min(1.0)
    };
    let a = (f64::from(rgba[3]) * envelope).round() as u8;
    if a == 0 {
        return;
    }
    for y in rect[1]..rect[3] {
        for x in rect[0]..rect[2] {
            frame.pixel(x as i32, y as i32, [rgba[0], rgba[1], rgba[2], a]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn object(properties: &[(&str, Value)]) -> ObjectState {
        let mut object = ObjectState {
            rect: (0, 0, 32, 32),
            ..ObjectState::default()
        };
        for (key, value) in properties {
            object.properties.insert((*key).into(), value.clone());
        }
        object
    }
    #[test]
    fn colors_decode_dwords_and_byte_order() {
        assert_eq!(
            color(Some(&Value::Integer(0x802F_2F2Fu32 as i32))),
            Some([47, 47, 47, 128])
        );
        assert_eq!(
            color(Some(&Value::Bytes(vec![0x4e, 0xed, 0x98, 0xff]))),
            Some([0x98, 0xed, 0x4e, 255])
        );
        assert_eq!(color(Some(&Value::Integer(0))), Some([0; 4]));
        assert_eq!(color(Some(&Value::Bytes(vec![1, 2]))), None);
    }
    #[test]
    fn lighting_tints_actors_only_and_flush_rises_then_falls() {
        let mut frame = Framebuffer::new(32, 32).unwrap();
        frame.clear([200, 100, 50]);
        let lit = object(&[
            (
                "render.lighting.色1",
                Value::Bytes(vec![0x80, 0x80, 0x80, 0xff]),
            ),
            (
                "render.lighting.色2",
                Value::Bytes(vec![0x80, 0x80, 0x80, 0xff]),
            ),
            ("render.lighting.カラーオペレーション", Value::Integer(1)),
        ]);
        render_effects(&lit, &BTreeMap::new(), &mut frame);
        assert_eq!(&frame.rgba[..3], &[200, 100, 50]);
        let mut actor = Framebuffer::new(32, 32).unwrap();
        actor.clear([200, 100, 50]);
        tint_layer(&mut actor, lit.actor_tint().unwrap());
        assert_eq!(&actor.rgba[..3], &[100, 50, 25]);
        let flash = |clock| {
            let mut frame = Framebuffer::new(32, 32).unwrap();
            render_effects(
                &object(&[
                    ("render.flush.color", Value::Integer(0xffff_ffffu32 as i32)),
                    ("render.flush.rise", Value::Integer(50)),
                    ("render.flush.fall", Value::Integer(50)),
                    ("render.flush.start", Value::Integer(0)),
                    (CLOCK, Value::Integer(clock)),
                ]),
                &BTreeMap::new(),
                &mut frame,
            );
            frame.rgba[0]
        };
        assert!(flash(0) == 0 && flash(25) > 100 && flash(50) == 255 && flash(100) == 0);
    }
    #[test]
    fn weather_ramps_from_its_previous_strength_and_draws_particles() {
        let mut weather = object(&[]);
        weather.camera.focal_length = 720;
        let mut fields = BTreeMap::from([
            ("スタイル".to_owned(), Value::Integer(0x10000)),
            ("粒の強さ[.8]".to_owned(), Value::Integer(10 * 256)),
            ("粒の明度[.8]".to_owned(), Value::Integer(256)),
            ("Zレンジ近".to_owned(), Value::Integer(256)),
            ("Zレンジ遠".to_owned(), Value::Integer(6000)),
            ("変化時間".to_owned(), Value::Integer(1000)),
        ]);
        let at = |object: &mut ObjectState, clock| {
            object
                .properties
                .insert(CLOCK.into(), Value::Integer(clock));
            object.weather_strength()
        };
        weather.set_weather(fields.clone());
        assert_eq!(at(&mut weather, 0), 0.0);
        assert_eq!(at(&mut weather, 500), 5.0);
        assert_eq!(at(&mut weather, 2000), 10.0);
        let mut frame = Framebuffer::new(32, 32).unwrap();
        render_effects(&weather, &BTreeMap::new(), &mut frame);
        assert!(frame.rgba.chunks(4).any(|p| p[0] > 0));
        fields.insert("粒の強さ[.8]".into(), Value::Integer(0));
        weather.set_weather(fields);
        assert_eq!(at(&mut weather, 2500), 5.0);
        assert_eq!(at(&mut weather, 3000), 0.0);
    }
}
