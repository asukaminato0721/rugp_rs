//! Window effects drawn over a parse screen after its world and actors:
//! Xs raster distortion, OM_SetWeatherRainSnow particles and
//! CLayerSimpleFlush filters; plus the OM_SetSpecifyCmdLighting tint of the
//! actors standing in that window (the background art is already lit).
//!
//! Parameters, their units and their call sites are retail (see the message
//! audit). The CLayerSimpleFlush kernels follow Vm60 (multiply tint and
//! over-exposure); the particle and raster kernels are not ported, so those
//! pixels are portable approximations driven by their parameters. Every
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
    pub(super) fn effect_clock(&self) -> u32 {
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
        eprintln!("WEATHER {:?}", fields.iter().map(|(k, v)| format!("{k}={}", match v { Value::Integer(i) => i.to_string(), Value::Real(r) => r.to_string(), Value::Null => "null".into(), o => format!("{o:?}").chars().take(30).collect() })).collect::<Vec<_>>());
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
    /// CLayerSimpleFlush OM_PlayMedia (Vm60 10057d60 class): the layer
    /// filters the window named by var 0 from now on. Its parameters stay
    /// live on the layer object, so later SetColorData/SetVarData packets
    /// (retail SC10 tint transitions) keep steering it.
    pub(super) fn play_flush(&mut self, layer: u64) -> Result<()> {
        let Some(window) = flush_window(self.objects.get(&layer).map(|o| &o.properties)) else {
            return Ok(());
        };
        let start = self.effect_start(window);
        let properties = &mut self.objects.entry(window).or_default().properties;
        properties.retain(|key, _| !key.starts_with("render.flush."));
        properties.insert("render.flush.layer".into(), Value::Integer(layer as i32));
        properties.insert("render.flush.start".into(), Value::Integer(start));
        Ok(())
    }
    /// SetColorData 11/12 and SetVarData 36 capture the value shown now as
    /// the start of a transition that SetVarData 10 times (Vm60 10059496,
    /// 100594be, 1005951d, 10059452); a direct color/amplitude write ends a
    /// finished transition.
    pub(super) fn flush_parameter(&mut self, layer: u64, color: bool, key: i32) {
        let clock = flush_window(self.objects.get(&layer).map(|o| &o.properties))
            .and_then(|w| self.objects.get(&w))
            .map_or(0, ObjectState::effect_clock);
        let Some(object) = self.objects.get_mut(&layer) else {
            return;
        };
        let state = FlushState::at(&object.properties, clock);
        let p = &mut object.properties;
        if state.tween_done {
            p.insert("render.color.0".into(), Value::Integer(dword(state.target)));
            p.insert("render.color.1".into(), Value::Integer(dword(state.base)));
            p.insert("render.var.32".into(), Value::Integer(state.amplitude));
            p.retain(|k, _| {
                !k.starts_with("render.flush_tween.") && !k.starts_with("render.flush_from.")
            });
        }
        match (color, key) {
            (true, 11) => {
                p.insert(
                    "render.flush_from.11".into(),
                    Value::Integer(dword(state.target)),
                );
            }
            (true, 12) => {
                p.insert(
                    "render.flush_from.12".into(),
                    Value::Integer(dword(state.base)),
                );
            }
            (false, 36) => {
                p.insert(
                    "render.flush_from.36".into(),
                    Value::Integer(state.amplitude),
                );
            }
            (false, 10) => {
                p.insert(
                    "render.flush_tween.start".into(),
                    Value::Integer(clock as i32),
                );
            }
            _ => {}
        }
    }
    /// Advance window effect clocks, retire finished flashes and stopped
    /// weather, and redraw while something animates.
    pub(super) fn advance_effects(&mut self, dt_ms: u32) -> Result<()> {
        let mut animate = false;
        // A layer stops itself (vtable +0xc) once its run or its return to
        // the identity tint completes.
        let finished: Vec<u64> = self
            .objects
            .iter()
            .filter_map(|(&id, window)| {
                let layer = flush_layer(&window.properties, &self.objects)?;
                let clock = window.effect_clock().saturating_add(dt_ms);
                let start = get(&window.properties, "render.flush.start").max(0) as u32;
                FlushState::at(layer, clock)
                    .finished(layer, clock.saturating_sub(start))
                    .then_some(id)
            })
            .collect();
        for (&id, object) in self.objects.iter_mut().filter(|(_, o)| o.has_effects()) {
            let clock = object
                .effect_clock()
                .saturating_add(dt_ms)
                .min(i32::MAX as u32);
            object
                .properties
                .insert(CLOCK.into(), Value::Integer(clock as i32));
            if finished.contains(&id) {
                object
                    .properties
                    .retain(|key, _| !key.starts_with("render.flush."));
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
    flush(object, objects, frame, rect, clock);
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
    let knob = |k: &str, d: f64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let count = (strength * if rain { 260.0 * knob("RAIN_COUNT", 1.0) } else { 60.0 }).min(8000.0) as u64;
    let opacity = opacity * if independent.is_some() { knob("RAIN_IND_ALPHA", 1.0) } else { knob("RAIN_ALPHA", 1.0) };
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

/// The window a CLayerSimpleFlush filters (var 0).
fn flush_window(layer: Option<&BTreeMap<String, Value>>) -> Option<u64> {
    match layer?.get("render.var.0") {
        Some(Value::Reference(window)) => window.offset,
        _ => None,
    }
}

fn flush_layer<'a>(
    window: &BTreeMap<String, Value>,
    objects: &'a BTreeMap<u64, ObjectState>,
) -> Option<&'a BTreeMap<String, Value>> {
    let Some(Value::Integer(layer)) = window.get("render.flush.layer") else {
        return None;
    };
    objects.get(&(*layer as u32 as u64)).map(|o| &o.properties)
}

fn dword(rgba: [u8; 4]) -> i32 {
    i32::from_le_bytes([rgba[2], rgba[1], rgba[0], rgba[3]])
}

/// UnivUI ordinal 515: a random value below `n` (none for `n <= 0`). A hash
/// of the effect time keeps replays reproducible.
fn native_random(n: i32, seed: u32) -> i32 {
    if n <= 0 {
        return 0;
    }
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    ((x >> 1) % n as u32) as i32
}

/// Multiply colours (0x80 per channel is identity) and amplitude, including
/// a transition in progress (Vm60 10058300: per-channel lerp over var 10 ms).
#[derive(Debug, Clone, Copy, PartialEq)]
struct FlushState {
    target: [u8; 4],
    base: [u8; 4],
    amplitude: i32,
    tween_done: bool,
}

impl FlushState {
    fn at(p: &BTreeMap<String, Value>, clock: u32) -> Self {
        // Constructor defaults: color 0 = 0xffffffff, color 1 = 0xff808080.
        let mut state = Self {
            target: color(p.get("render.color.0")).unwrap_or([255; 4]),
            base: color(p.get("render.color.1")).unwrap_or([128, 128, 128, 255]),
            amplitude: get(p, "render.var.32"),
            tween_done: false,
        };
        if let Some(start) = p.get("render.flush_tween.start") {
            let duration = get(p, "render.var.10");
            let elapsed = i64::from(clock) - i64::from(start.integer().unwrap_or(0));
            let progress = if duration <= 0 {
                0x10000
            } else {
                (elapsed * 0x10000 / i64::from(duration)).clamp(0, 0x10000)
            };
            let lerp = |from: i64, to: i64| from + (((to - from) * progress) >> 16);
            let channels =
                |from: Option<[u8; 4]>, to: Option<[u8; 4]>, current: [u8; 4]| match (from, to) {
                    (Some(from), Some(to)) => {
                        std::array::from_fn(|c| lerp(i64::from(from[c]), i64::from(to[c])) as u8)
                    }
                    _ => current,
                };
            state.target = channels(
                color(p.get("render.flush_from.11")),
                color(p.get("render.color.11")),
                state.target,
            );
            state.base = channels(
                color(p.get("render.flush_from.12")),
                color(p.get("render.color.12")),
                state.base,
            );
            if p.contains_key("render.flush_from.36") {
                state.amplitude = lerp(
                    i64::from(get(p, "render.flush_from.36")),
                    i64::from(get(p, "render.var.36")),
                ) as i32;
            }
            state.tween_done = progress >= 0x10000;
        }
        state
    }
    /// Whether the layer has stopped `elapsed` ms after OM_PlayMedia.
    fn finished(&self, p: &BTreeMap<String, Value>, elapsed: u32) -> bool {
        let identity = |c: [u8; 4]| c[..3] == [128; 3];
        // 1005845d: a finished transition back to identity with no
        // amplitude stops the layer.
        if self.tween_done && self.amplitude == 0 && identity(self.target) && identity(self.base) {
            return true;
        }
        match get(p, "render.var.15") & 0xff {
            0 => flicker(p, elapsed).is_none(),
            1 => {
                let period = get(p, "render.var.16");
                get(p, "render.var.64") == 1 && i64::from(elapsed) > i64::from(period)
            }
            _ => false,
        }
    }
}

/// Mode 0 timing (Vm60 100584b0): each cycle lasts var 1 + var 2 + a random
/// extension and exposes for its first var 1 + rnd(var 1 / 2) ms; var 3 > 0
/// limits the cycle count. Returns whether the image is exposed now, or
/// `None` once the run is over.
fn flicker(p: &BTreeMap<String, Value>, elapsed: u32) -> Option<bool> {
    let (on, off, count) = (
        get(p, "render.var.1").max(0),
        get(p, "render.var.2").max(0),
        get(p, "render.var.3"),
    );
    let (mut lead, mut extra) = (0, 0);
    let mut t = i64::from(elapsed);
    let mut cycles = 0;
    loop {
        let length = i64::from(on + off + extra);
        if length <= 0 || t < length {
            break;
        }
        t -= length;
        cycles += 1;
        lead = native_random(on / 2, cycles as u32);
        extra = native_random(off, cycles as u32 ^ 0x5555) + lead;
        if cycles > 100_000 {
            break;
        }
    }
    if count > 0 && cycles >= count {
        return None;
    }
    Some(t < i64::from(lead + on))
}

fn flush(
    object: &ObjectState,
    objects: &BTreeMap<u64, ObjectState>,
    frame: &mut Framebuffer,
    rect: [usize; 4],
    clock: u32,
) {
    let Some(layer) = flush_layer(&object.properties, objects) else {
        return;
    };
    let elapsed = clock.saturating_sub(get(&object.properties, "render.flush.start").max(0) as u32);
    let stride = frame.width as usize;
    let mut each = |f: &dyn Fn(usize, u8) -> u8| {
        for y in rect[1]..rect[3] {
            for x in rect[0]..rect[2] {
                let o = (y * stride + x) * 4;
                for c in 0..3 {
                    frame.rgba[o + c] = f(c, frame.rgba[o + c]);
                }
            }
        }
    };
    match get(layer, "render.var.15") & 0xff {
        0 => {
            // 1005891c: square each channel, then shift by
            // rnd(var 4) + 8 - var 4 (saturating): a strong over-exposure.
            if flicker(layer, elapsed) != Some(true) {
                return;
            }
            let strength = get(layer, "render.var.4");
            let shift = (native_random(strength, clock) + 8 - strength).clamp(0, 15) as u32;
            each(&|_, v| ((u32::from(v) * u32::from(v)) >> shift).min(255) as u8);
        }
        1 => {
            // 10058794: the multiply colour swings from color 1 to color 0
            // and back every var 16 ms; amplitude scales it by 1 + a/256.
            let period = get(layer, "render.var.16");
            if period <= 0 {
                return;
            }
            let state = FlushState::at(layer, clock);
            let wave = -(TAU * f64::from(elapsed) / f64::from(period)).cos();
            let weight = (wave + 1.0) * 0.5;
            let alpha = (f64::from(state.amplitude) * wave).round() as i32;
            let scale = (alpha + 256).clamp(0, 512) as u32;
            let multiply: [u32; 3] = std::array::from_fn(|c| {
                let base = f64::from(state.base[c]);
                let mixed = (f64::from(state.target[c]) - base) * weight + base;
                (mixed.round().clamp(0.0, 255.0) as u32 * scale) >> 7
            });
            each(&|c, v| ((u32::from(v) * multiply[c]) >> 8).min(255) as u8);
        }
        _ => {}
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
    }
    fn flush_scene(
        layer: &[(&str, Value)],
        clock: i32,
    ) -> (ObjectState, BTreeMap<u64, ObjectState>) {
        let window = object(&[
            ("render.flush.layer", Value::Integer(7)),
            ("render.flush.start", Value::Integer(0)),
            (CLOCK, Value::Integer(clock)),
        ]);
        (window, BTreeMap::from([(7, object(layer))]))
    }
    fn filtered(layer: &[(&str, Value)], clock: i32, pixel: u8) -> u8 {
        let (window, objects) = flush_scene(layer, clock);
        let mut frame = Framebuffer::new(32, 32).unwrap();
        frame.clear([pixel; 3]);
        render_effects(&window, &objects, &mut frame);
        frame.rgba[0]
    }
    #[test]
    fn simple_flush_mode_1_multiplies_by_the_swinging_tint() {
        // Retail SC3: black, then a steady 0x2f tint (x 0.37), period 100 ms.
        let tint = |c: u32| Value::Integer((0x8000_0000 | c * 0x01_0101) as i32);
        let layer = |c| {
            vec![
                ("render.var.15", Value::Integer(1)),
                ("render.var.16", Value::Integer(100)),
                ("render.var.32", Value::Integer(0)),
                ("render.color.0", tint(c)),
                ("render.color.1", tint(c)),
            ]
        };
        assert_eq!(filtered(&layer(0), 30, 200), 0);
        assert_eq!(filtered(&layer(0x2f), 30, 200), (200 * 94 / 256) as u8);
        assert_eq!(filtered(&layer(0x80), 30, 200), 200);
        // Color 1 at the period start, color 0 half a period later.
        let swing = [
            ("render.var.15", Value::Integer(1)),
            ("render.var.16", Value::Integer(100)),
            ("render.color.0", tint(0)),
            ("render.color.1", tint(0x80)),
        ];
        assert_eq!(filtered(&swing, 0, 100), 100);
        assert_eq!(filtered(&swing, 50, 100), 0);
        // Without a period nothing is drawn.
        assert_eq!(filtered(&swing[1..], 50, 100), 100);
    }
    #[test]
    fn simple_flush_transitions_back_to_identity_and_stops() {
        let mut layer: BTreeMap<String, Value> = [
            ("render.var.15", Value::Integer(1)),
            ("render.var.16", Value::Integer(100)),
            ("render.color.0", Value::Integer(0x802f_2f2fu32 as i32)),
            ("render.color.1", Value::Integer(0x802f_2f2fu32 as i32)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let before = FlushState::at(&layer, 1000);
        layer.insert(
            "render.flush_from.11".into(),
            Value::Integer(dword(before.target)),
        );
        layer.insert(
            "render.flush_from.12".into(),
            Value::Integer(dword(before.base)),
        );
        layer.insert(
            "render.color.11".into(),
            Value::Integer(0x8080_8080u32 as i32),
        );
        layer.insert(
            "render.color.12".into(),
            Value::Integer(0x8080_8080u32 as i32),
        );
        layer.insert("render.var.10".into(), Value::Integer(800));
        layer.insert("render.flush_tween.start".into(), Value::Integer(1000));
        let half = FlushState::at(&layer, 1400);
        assert_eq!(half.target[0], 0x2f + (0x80 - 0x2f) / 2);
        assert!(!half.finished(&layer, 1400));
        let end = FlushState::at(&layer, 1800);
        assert_eq!(end.target[..3], [0x80; 3]);
        assert!(end.finished(&layer, 1800));
    }
    #[test]
    fn simple_flush_mode_0_overexposes_once_for_its_on_time() {
        // Retail SC15: on 50 ms, off 50 ms, one cycle, strength 2.
        let layer = [
            ("render.var.1", Value::Integer(50)),
            ("render.var.2", Value::Integer(50)),
            ("render.var.3", Value::Integer(1)),
            ("render.var.4", Value::Integer(2)),
        ];
        let bright = filtered(&layer, 10, 128);
        assert!(bright == 255 || bright == 128, "{bright}");
        assert_eq!(filtered(&layer, 10, 6), 0, "dark tones crush");
        assert_eq!(filtered(&layer, 70, 128), 128, "off phase");
        let p: BTreeMap<String, Value> = layer
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        assert!(!FlushState::at(&p, 70).finished(&p, 70));
        assert!(FlushState::at(&p, 120).finished(&p, 120));
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
