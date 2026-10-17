//! Pixel operators decoded from the retail rvmm.dll.
use rugp_assets::fade::{Fade, Qube, Stretch};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) enum Operator {
    Mosaic,
    Invert,
    /// rvmm 1001e900: CFadeShock never blends. Each frame moves the window
    /// by `rand() % 32 - 16` on both axes; full alpha (0x2000) restores it.
    Shock {
        frame: u32,
    },
    RightCurtain,
    Curtain {
        direction: u8,
    },
    Stretch {
        parameters: Stretch,
        source: [i16; 4],
        history: bool,
    },
    /// rvmm 10020720/10048030: a stretched face tilted in perspective,
    /// blended over the frame it is drawn into (the turn's motion trail).
    Qube {
        parameters: Stretch,
        source: [i16; 4],
        qube: Qube,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SpatialFade {
    pub operator: Operator,
    pub rectangle: [i16; 4],
    pub previous_alpha: Option<u16>,
    pub source_size: (u32, u32),
}

impl SpatialFade {
    pub fn from_class(class: &str, rectangle: [i16; 4]) -> Option<Self> {
        let operator = match class {
            "CFadeMozaik" => Operator::Mosaic,
            "CFadeInvert" => Operator::Invert,
            "CFadeShock" => Operator::Shock { frame: 0 },
            "CFadeRSideCarten" => Operator::RightCurtain,
            _ => return None,
        };
        Some(Self {
            operator,
            rectangle,
            previous_alpha: None,
            source_size: (0, 0),
        })
    }

    pub fn from_fade(class: &str, fade: &Fade, source_size: (u32, u32)) -> Option<Self> {
        let mut effect = if let Some(parameters) = fade
            .curtain
            .filter(|p| p.feather == 128 && p.direction <= 3)
        {
            Self {
                operator: Operator::Curtain {
                    direction: parameters.direction,
                },
                rectangle: fade.rectangles[1],
                previous_alpha: None,
                source_size,
            }
        } else if let Some(parameters) = fade.stretch.filter(|p| p.frames <= 0) {
            let mut rectangle = fade.rectangles[1];
            let mut parameters = parameters;
            if fade.mode & 1 != 0 {
                for (i, value) in rectangle.iter_mut().enumerate().skip(2) {
                    if *value == 0 {
                        *value = fade.rectangles[0][i];
                    }
                    if parameters.destination_end[i] == 0 {
                        parameters.destination_end[i] = fade.rectangles[0][i];
                    }
                }
            }
            let operator = match fade.qube {
                Some(qube) => Operator::Qube {
                    parameters,
                    source: fade.rectangles[0],
                    qube,
                },
                None => Operator::Stretch {
                    parameters,
                    source: fade.rectangles[0],
                    history: false,
                },
            };
            Self {
                operator,
                rectangle,
                previous_alpha: None,
                source_size,
            }
        } else {
            Self::from_class(class, fade.rectangles[1])?
        };
        effect.source_size = source_size;
        Some(effect)
    }

    /// Stretch-based operators sample the unplaced source surface.
    pub fn samples_raw_source(&self) -> bool {
        matches!(
            self.operator,
            Operator::Stretch { .. } | Operator::Qube { .. }
        )
    }

    pub fn source_size(&self, fallback: (u32, u32)) -> (u32, u32) {
        if self.samples_raw_source() {
            self.source_size
        } else {
            fallback
        }
    }

    pub fn keeps_previous_frame(&self) -> bool {
        matches!(
            self.operator,
            Operator::RightCurtain
                | Operator::Curtain { .. }
                | Operator::Stretch { history: true, .. }
                | Operator::Qube { .. }
        ) && self.previous_alpha.is_some()
    }

    #[cfg(test)]
    pub fn render(&mut self, output: &mut [u8], source: &[u8], size: (u32, u32), alpha: u16) {
        self.render_q13(output, source, size, alpha * 32);
    }

    pub fn render_q13(
        &mut self,
        output: &mut [u8],
        source: &[u8],
        size: (u32, u32),
        progress: u16,
    ) {
        if let Operator::Stretch {
            parameters,
            source: start,
            ..
        } = self.operator
        {
            self.render_stretch(output, source, size, start, parameters, progress);
            self.previous_alpha = Some(progress >> 5);
            return;
        }
        if let Operator::Qube {
            parameters,
            source: start,
            qube,
        } = self.operator
        {
            self.render_qube(output, source, size, start, parameters, qube, progress);
            self.previous_alpha = Some(progress >> 5);
            return;
        }
        if let Operator::Shock { frame } = &mut self.operator {
            *frame = frame.wrapping_add(1);
            let offset = if progress >= 8192 {
                (0, 0)
            } else {
                shake_offset(*frame)
            };
            render_shock(output, source, size, offset);
            self.previous_alpha = Some(progress >> 5);
            return;
        }
        let alpha = progress >> 5;
        let [x, y, w, h] = self.rectangle.map(i64::from);
        let width = i64::from(size.0);
        let height = i64::from(size.1);
        let w = if w == 0 { width } else { w };
        let h = if h == 0 { height } else { h };
        let block = i64::from((256 - alpha) / 8 + 1);
        let previous = self.previous_alpha.replace(alpha).unwrap_or(0);
        let direction = match self.operator {
            Operator::Curtain { direction } => direction,
            _ => 0,
        };
        let extent = if direction < 2 { w } else { h };
        let position = i64::from(alpha) * (extent + 128) / 256 - 128;
        let old_position = i64::from(previous) * (extent + 128) / 256 - 128;
        let reverse = direction & 1 != 0;
        let (strip_start, strip_end) = if reverse {
            (
                extent - 128 - position.max(old_position),
                extent - position.min(old_position),
            )
        } else {
            (position.min(old_position), position.max(old_position) + 128)
        };
        for row in y.max(0)..(y + h).min(height) {
            for column in x.max(0)..(x + w).min(width) {
                let target = ((row * width + column) * 4) as usize;
                match self.operator {
                    Operator::Stretch { .. } | Operator::Qube { .. } | Operator::Shock { .. } => {
                        unreachable!()
                    }
                    Operator::Mosaic => {
                        // rvmm 1003b700 samples the upper-left pixel of each
                        // block. The grid starts at the effect rectangle.
                        let sx = x + (column - x) / block * block;
                        let sy = y + (row - y) / block * block;
                        if sx < 0 || sy < 0 {
                            continue;
                        }
                        let sample = ((sy * width + sx) * 4) as usize;
                        for channel in 0..3 {
                            output[target + channel] = ((u32::from(source[sample + channel])
                                * u32::from(alpha))
                                >> 8) as u8;
                        }
                    }
                    Operator::Invert => {
                        // rvmm 10039230 interpolates source to its complement.
                        // Native integer arithmetic truncates each channel.
                        for channel in 0..3 {
                            let value = u32::from(source[target + channel]);
                            output[target + channel] = ((value * u32::from(256 - alpha)
                                + (255 - value) * u32::from(alpha))
                                >> 8) as u8;
                        }
                    }
                    Operator::RightCurtain | Operator::Curtain { .. } => {
                        // rvmm 1003b8b0/1003bb30 updates only the moving
                        // strip. Reverse directions increment the blend
                        // counter before sampling; forward directions decrement.
                        let local = if direction < 2 { column - x } else { row - y };
                        if alpha == previous || local < strip_start || local >= strip_end {
                            continue;
                        }
                        let weight = if reverse {
                            position + 129 - extent + local
                        } else {
                            position + 127 - local
                        }
                        .clamp(0, 128) as u32;
                        for channel in 0..3 {
                            output[target + channel] = ((u32::from(source[target + channel])
                                * weight
                                + u32::from(output[target + channel]) * (128 - weight))
                                >> 7) as u8;
                        }
                    }
                }
                output[target + 3] = 255;
            }
        }
    }

    /// rvmm 1001ff10 interpolates both rectangles in Q13. A zero source end
    /// extent preserves the initial extent. Destination zero remains zero.
    fn stretch_rects(
        &self,
        start: [i16; 4],
        parameters: Stretch,
        progress: u16,
    ) -> ([i64; 4], [i64; 4]) {
        let p = i64::from(progress);
        let src = std::array::from_fn(|i| {
            let end = if i >= 2 && parameters.source_end[i] == 0 {
                start[i]
            } else {
                parameters.source_end[i]
            };
            i64::from(start[i]) * (8192 - p) + i64::from(end) * p
        });
        let dst = std::array::from_fn(|i| {
            i64::from(self.rectangle[i]) * (8192 - p) + i64::from(parameters.destination_end[i]) * p
        });
        (src, dst)
    }

    /// The face maps the destination rectangle onto the source rectangle
    /// about their centres. rvmm 10048030 adds second-order perspective
    /// terms by forward differencing; this evaluates the same polynomial
    /// directly, with bilinear taps that are black past the image edge.
    #[allow(clippy::too_many_arguments)]
    fn render_qube(
        &self,
        output: &mut [u8],
        source: &[u8],
        size: (u32, u32),
        start: [i16; 4],
        parameters: Stretch,
        qube: Qube,
        progress: u16,
    ) {
        let (src, dst) = self.stretch_rects(start, parameters, progress);
        let (sw, sh) = self.source_size;
        if src[2] <= 0 || src[3] <= 0 || dst[2] <= 0 || dst[3] <= 0 || sw == 0 || sh == 0 {
            return;
        }
        let p = i32::from(progress);
        // 10020720: weights round in Q13, angles truncate toward zero.
        let weight = |[a, b]: [i16; 2]| {
            let a = i32::from(a);
            a + (((i32::from(b) - a) * p + 0x1000) >> 13)
        };
        let source_weight = weight(qube.source_weight);
        let destination_weight = weight(qube.destination_weight);
        let angle = |axis: usize| {
            let a = i32::from(qube.angles[0][axis]);
            a + (i32::from(qube.angles[1][axis]) - a) * p / 8192
        };
        let ratio = dst[2] as f64 / src[2] as f64;
        let tilt = |axis| ratio * (f64::from(angle(axis)) * std::f64::consts::TAU / 4096.0).sin();
        let (tilt_x, tilt_y) = (tilt(0), tilt(1));
        let q13 = |v: i64| v as f64 / 8192.0;
        let destination_center = [q13(dst[0] + dst[2] / 2), q13(dst[1] + dst[3] / 2)];
        let source_center = [q13(src[0] + src[2] / 2), q13(src[1] + src[3] / 2)];
        let scale = [src[2] as f64 / dst[2] as f64, src[3] as f64 / dst[3] as f64];
        let pivot_y = if qube.pivot_from_rect != 0 {
            destination_center[1]
        } else {
            f64::from(qube.center[1])
        };
        // Native forward-difference constants: 15 * 2^16 for the squared
        // terms, 8.75 * 2^16 and -683 for the cross terms (rvmm 10083750).
        let square = 15.0 / 16384.0;
        let cross_u = 573_440.0 / f64::from(1u32 << 29);
        let cross_v = 683.0 / f64::from(1u32 << 19);
        let plain = source_weight == 256 && destination_weight == 0;
        let width = i64::from(size.0);
        let left = (dst[0] >> 13).max(0);
        let top = (dst[1] >> 13).max(0);
        let right = ((dst[0] + dst[2]) >> 13).min(width);
        let bottom = ((dst[1] + dst[3]) >> 13).min(i64::from(size.1));
        let texel = |x: i64, y: i64, channel: usize| -> i32 {
            if x < i64::from(sw) && y < i64::from(sh) {
                i32::from(source[((y * i64::from(sw) + x) * 4) as usize + channel])
            } else {
                0
            }
        };
        for row in top..bottom {
            let y = row as f64 - pivot_y;
            let base_y = row as f64 - destination_center[1];
            for column in left..right {
                let x = column as f64 - destination_center[0];
                let u = source_center[0]
                    + x * scale[0]
                    + x * x * square * tilt_x
                    + x * y * cross_u * tilt_y;
                let v = source_center[1]
                    + base_y * scale[1]
                    + y * y * square * tilt_y
                    + x * y * cross_v * tilt_x;
                let target = ((row * width + column) * 4) as usize;
                let pixel = &mut output[target..target + 4];
                if u < 0.0 || v < 0.0 {
                    pixel.copy_from_slice(&[0, 0, 0, 255]);
                    continue;
                }
                let (ui, vi) = (u.floor() as i64, v.floor() as i64);
                if ui >= i64::from(sw) || vi >= i64::from(sh) {
                    if plain {
                        pixel.copy_from_slice(&[0, 0, 0, 255]);
                    } else if parameters.options == 2 {
                        for value in &mut pixel[..3] {
                            *value = ((i32::from(*value) * destination_weight) >> 8) as u8;
                        }
                    }
                    continue;
                }
                // 14-bit fractions, as the MMX loop keeps them.
                let fx = ((u - ui as f64) * 16384.0) as i32;
                let fy = ((v - vi as f64) * 16384.0) as i32;
                for (channel, value) in pixel.iter_mut().take(3).enumerate() {
                    let lerp = |a: i32, b: i32, f: i32| a + (((b - a) * 4 * f) >> 16);
                    let top = lerp(texel(ui, vi, channel), texel(ui + 1, vi, channel), fx);
                    let bottom = lerp(
                        texel(ui, vi + 1, channel),
                        texel(ui + 1, vi + 1, channel),
                        fx,
                    );
                    let sample = lerp(top, bottom, fy).clamp(0, 255);
                    *value = if plain {
                        sample as u8
                    } else {
                        (((sample * source_weight) >> 8)
                            + ((i32::from(*value) * destination_weight) >> 8))
                            .clamp(0, 255) as u8
                    };
                }
                pixel[3] = 255;
            }
        }
    }

    fn render_stretch(
        &self,
        output: &mut [u8],
        source: &[u8],
        size: (u32, u32),
        start: [i16; 4],
        parameters: Stretch,
        progress: u16,
    ) {
        let p = i64::from(progress);
        let (src, dst) = self.stretch_rects(start, parameters, progress);
        if src[2] <= 0 || src[3] <= 0 || dst[2] <= 0 || dst[3] <= 0 {
            return;
        }
        let opacity = ((i64::from(parameters.opacity[0]) * 8192
            + p * (i64::from(parameters.opacity[1]) - i64::from(parameters.opacity[0]))
            + 4096)
            >> 13)
            .clamp(0, 8192);
        let (sw, sh) = self.source_size;
        for y in (dst[1] >> 13).max(0)..((dst[1] + dst[3]) >> 13).min(i64::from(size.1)) {
            let sy = src[1] + ((y * 8192 - dst[1]) * src[3]) / dst[3];
            if sy < 0 || sy >= i64::from(sh) * 8192 {
                continue;
            }
            for x in (dst[0] >> 13).max(0)..((dst[0] + dst[2]) >> 13).min(i64::from(size.0)) {
                let sx = src[0] + ((x * 8192 - dst[0]) * src[2]) / dst[2];
                if sx < 0 || sx >= i64::from(sw) * 8192 {
                    continue;
                }
                let ix = (sx >> 13) as u32;
                let iy = (sy >> 13) as u32;
                let fx = sx & 8191;
                let fy = sy & 8191;
                let out = ((y as u32 * size.0 + x as u32) * 4) as usize;
                for channel in 0..3 {
                    let sample =
                        |x: u32, y: u32| i64::from(source[((y * sw + x) * 4) as usize + channel]);
                    let x1 = (ix + 1).min(sw - 1);
                    let y1 = (iy + 1).min(sh - 1);
                    let top = sample(ix, iy) * (8192 - fx) + sample(x1, iy) * fx;
                    let bottom = sample(ix, y1) * (8192 - fx) + sample(x1, y1) * fx;
                    let pixel = (top * (8192 - fy) + bottom * fy) >> 26;
                    output[out + channel] = ((pixel * opacity
                        + i64::from(output[out + channel]) * (8192 - opacity))
                        >> 13) as u8;
                }
                output[out + 3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shock_moves_the_whole_image_and_settles_at_full_alpha() {
        let mut fade = SpatialFade::from_class("CFadeShock", [0; 4]).unwrap();
        let source: Vec<u8> = (0..64u8).flat_map(|i| [i * 4, 0, 0, 255]).collect();
        let mut output = vec![9; source.len()];
        let mut moved = false;
        for _ in 0..8 {
            fade.render_q13(&mut output, &source, (8, 8), 4096);
            let (dx, dy) = match fade.operator {
                Operator::Shock { frame } => shake_offset(frame),
                _ => unreachable!(),
            };
            assert!((-16..16).contains(&dx) && (-16..16).contains(&dy));
            moved |= output != source;
        }
        assert!(moved);
        fade.render_q13(&mut output, &source, (8, 8), 8192);
        assert_eq!(output, source);
    }

    use super::*;
    #[test]
    fn stretch_uses_source_geometry_q13_opacity_and_native_zero_extent() {
        let parameters = Stretch {
            destination_end: [0, 0, 4, 1],
            source_end: [2, 0, 0, 0],
            options: 1,
            frames: 0,
            opacity: [4096, 4096],
        };
        let mut effect = SpatialFade {
            operator: Operator::Stretch {
                parameters,
                source: [0, 0, 2, 1],
                history: true,
            },
            rectangle: [0, 0, 4, 1],
            previous_alpha: None,
            source_size: (4, 1),
        };
        let source = [0, 40, 80, 120]
            .into_iter()
            .flat_map(|v| [v, v, v, 255])
            .collect::<Vec<_>>();
        let mut output = vec![0; 16];
        effect.render_q13(&mut output, &source, (4, 1), 4096);
        assert_eq!(
            output.chunks(4).map(|p| p[0]).collect::<Vec<_>>(),
            [20, 30, 40, 50]
        );
        assert!(effect.keeps_previous_frame());
        // Serialize the moving operator and retain the rendered history.
        let mut restored: SpatialFade =
            serde_json::from_slice(&serde_json::to_vec(&effect).unwrap()).unwrap();
        let mut resumed = output.clone();
        effect.render_q13(&mut output, &source, (4, 1), 8192);
        restored.render_q13(&mut resumed, &source, (4, 1), 8192);
        assert_eq!(resumed, output);
        assert_eq!(
            output.chunks(4).map(|p| p[0]).collect::<Vec<_>>(),
            [50, 65, 80, 85]
        );
    }

    fn qube(
        source: [i16; 4],
        source_end: [i16; 4],
        weights: [[i16; 2]; 2],
        tilt: [i16; 2],
        size: (u32, u32),
    ) -> SpatialFade {
        SpatialFade {
            operator: Operator::Qube {
                parameters: Stretch {
                    destination_end: [0, 0, size.0 as i16, size.1 as i16],
                    source_end,
                    options: 1,
                    frames: 0,
                    opacity: [8192, 8192],
                },
                source,
                qube: Qube {
                    reserved: 0,
                    source_weight: weights[0],
                    destination_weight: weights[1],
                    center: [size.0 as i16 / 2, size.1 as i16 / 2],
                    angles: [[tilt[0], 0, 0], [tilt[1], 0, 0]],
                    pivot_from_rect: 1,
                },
            },
            rectangle: [0, 0, size.0 as i16, size.1 as i16],
            previous_alpha: None,
            source_size: size,
        }
    }

    #[test]
    fn untilted_unweighted_qube_copies_its_source() {
        let size = (16u32, 8u32);
        let source: Vec<u8> = (0..size.0 * size.1)
            .flat_map(|i| [i as u8, (i * 3) as u8, 7, 255])
            .collect();
        let full = [0, 0, 16, 8];
        let mut fade = qube(full, full, [[256, 256], [0, 0]], [0, 0], size);
        let mut output = vec![9; source.len()];
        fade.render_q13(&mut output, &source, size, 4096);
        assert_eq!(output, source);
        assert!(fade.keeps_previous_frame());
    }

    #[test]
    fn qube_blends_over_its_trail_blackens_before_the_image_and_keeps_past_it() {
        let size = (16u32, 4u32);
        let source = [200, 100, 0, 255].repeat(64);
        // The window starts half an image to the left and ends past the
        // right edge: leading columns are black, trailing ones untouched.
        let mut fade = qube(
            [-8, 0, 16, 4],
            [16, 0, 16, 4],
            [[64, 64], [192, 192]],
            [0, 0],
            size,
        );
        let mut output = [40, 40, 40, 255].repeat(64);
        fade.render_q13(&mut output, &source, size, 0);
        assert_eq!(&output[..4], &[0, 0, 0, 255]);
        // 200*64/256 + 40*192/256
        assert_eq!(&output[12 * 4..][..4], &[80, 55, 30, 255]);
        let mut output = [40, 40, 40, 255].repeat(64);
        fade.render_q13(&mut output, &source, size, 8192);
        assert_eq!(output, [40, 40, 40, 255].repeat(64));
    }

    #[test]
    fn qube_tilt_recedes_one_side_in_perspective() {
        let size = (64u32, 32u32);
        let source = [255, 255, 255, 255].repeat(64 * 32);
        let full = [0, 0, 64, 32];
        let mut fade = qube(full, full, [[256, 256], [0, 0]], [-400, -400], size);
        let mut output = [9, 9, 9, 255].repeat(64 * 32);
        fade.render_q13(&mut output, &source, size, 0);
        let lit = |column: usize| {
            (0..32)
                .filter(|row| output[(row * 64 + column) * 4] == 255)
                .count()
        };
        // Columns left of the pivot sample beyond the image rows.
        assert!(lit(2) < lit(32), "{} {}", lit(2), lit(32));
        assert_eq!(lit(32), 32);
    }

    #[test]
    fn mosaic_uses_native_grid_brightness_and_crop() {
        let mut source = [20, 40, 60, 255].repeat(40 * 3);
        source[(40 + 2) * 4..(40 + 2) * 4 + 4].copy_from_slice(&[201, 101, 51, 255]);
        source[(40 + 19) * 4..(40 + 19) * 4 + 4].copy_from_slice(&[81, 41, 21, 255]);
        let mut output = [7, 9, 11, 255].repeat(40 * 3);
        let mut fade = SpatialFade::from_class("CFadeMozaik", [2, 1, 20, 2]).unwrap();
        fade.render(&mut output, &source, (40, 3), 128);
        for y in 1..3 {
            for x in 2..19 {
                assert_eq!(&output[(y * 40 + x) * 4..][..4], &[100, 50, 25, 255]);
            }
            for x in 19..22 {
                assert_eq!(&output[(y * 40 + x) * 4..][..4], &[40, 20, 10, 255]);
            }
        }
        assert_eq!(&output[..4], &[7, 9, 11, 255]);
        assert_eq!(&output[(40 + 22) * 4..][..4], &[7, 9, 11, 255]);
        fade.render(&mut output, &source, (40, 3), 0);
        assert_eq!(&output[(40 + 2) * 4..][..4], &[0, 0, 0, 255]);
        fade.render(&mut output, &source, (40, 3), 256);
        assert_eq!(&output[(40 + 2) * 4..][..4], &[201, 101, 51, 255]);
    }

    #[test]
    fn invert_keeps_source_at_zero_and_inverts_at_full_alpha() {
        let source = [0, 101, 255, 255];
        let mut output = [0; 4];
        let mut fade = SpatialFade::from_class("CFadeInvert", [0; 4]).unwrap();
        for (alpha, expected) in [
            (0, [0, 101, 255, 255]),
            (128, [127, 127, 127, 255]),
            (256, [255, 154, 0, 255]),
        ] {
            fade.render(&mut output, &source, (1, 1), alpha);
            assert_eq!(output, expected);
        }
    }

    #[test]
    fn curtain_moves_a_feathered_strip_and_retains_previous_samples() {
        let source = [255, 0, 0, 255].repeat(256);
        let mut output = [0, 0, 255, 255].repeat(256);
        let mut fade = SpatialFade::from_class("CFadeRSideCarten", [0; 4]).unwrap();
        fade.render(&mut output, &source, (256, 1), 128);
        assert_eq!(&output[..4], &[255, 0, 0, 255]);
        assert_eq!(&output[127 * 4..][..4], &[127, 0, 127, 255]);
        assert_eq!(&output[191 * 4..][..4], &[0, 0, 255, 255]);
        let half = output.clone();
        fade.render(&mut output, &source, (256, 1), 128);
        assert_eq!(output, half);
        fade.render(&mut output, &source, (256, 1), 160);
        assert_eq!(&output[127 * 4..][..4], &[239, 0, 15, 255]);
        assert_eq!(&output[239 * 4..][..4], &[0, 0, 255, 255]);
    }
    #[test]
    fn general_curtains_follow_both_axes_and_reverse_counter_rounding() {
        for direction in 0..4 {
            let size = if direction < 2 { (256, 1) } else { (1, 256) };
            let source = [255, 0, 0, 255].repeat(256);
            let mut output = [0, 0, 255, 255].repeat(256);
            let mut effect = SpatialFade {
                operator: Operator::Curtain { direction },
                rectangle: [0; 4],
                previous_alpha: None,
                source_size: size,
            };
            effect.render(&mut output, &source, size, 128);
            assert_eq!(&output[127 * 4..][..4], &[127, 0, 127, 255]);
            let reverse = direction & 1 != 0;
            assert_eq!(output[0], if reverse { 0 } else { 255 });
            assert_eq!(output[191 * 4], if reverse { 255 } else { 0 });
            let half = output.clone();
            effect.render(&mut output, &source, size, 128);
            assert_eq!(output, half);
            assert!(effect.keeps_previous_frame());
            let mut restored: SpatialFade =
                serde_json::from_slice(&serde_json::to_vec(&effect).unwrap()).unwrap();
            let mut resumed = output.clone();
            restored.render(&mut resumed, &source, size, 160);
            effect.render(&mut output, &source, size, 160);
            assert_eq!(resumed, output);
            effect.render(&mut output, &source, size, 256);
            assert_eq!(output, source);
        }
    }
}

/// Native rand() per frame; a fixed sequence keeps replays reproducible.
fn shake_offset(frame: u32) -> (i64, i64) {
    let mut state = frame.wrapping_mul(0x9E37_79B9) ^ 0x5bd1_e995;
    let mut next = || {
        state = state.wrapping_mul(214_013).wrapping_add(2_531_011);
        i64::from((state >> 16) & 0x7fff) % 32 - 16
    };
    (next(), next())
}

fn render_shock(output: &mut [u8], source: &[u8], size: (u32, u32), (dx, dy): (i64, i64)) {
    let (width, height) = (i64::from(size.0), i64::from(size.1));
    for row in 0..height {
        for column in 0..width {
            let target = ((row * width + column) * 4) as usize;
            let (sx, sy) = (column - dx, row - dy);
            if (0..width).contains(&sx) && (0..height).contains(&sy) {
                let sample = ((sy * width + sx) * 4) as usize;
                output[target..target + 4].copy_from_slice(&source[sample..sample + 4]);
            } else {
                output[target..target + 3].fill(0);
            }
        }
    }
}
