//! Pixel operators decoded from the retail rvmm.dll.
use rugp_assets::fade::{Fade, Stretch};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) enum Operator {
    Mosaic,
    Invert,
    RightCurtain,
    Curtain {
        direction: u8,
    },
    Stretch {
        parameters: Stretch,
        source: [i16; 4],
        history: bool,
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
            Self {
                operator: Operator::Stretch {
                    parameters,
                    source: fade.rectangles[0],
                    history: false,
                },
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

    pub fn source_size(&self, fallback: (u32, u32)) -> (u32, u32) {
        if matches!(self.operator, Operator::Stretch { .. }) {
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
                    Operator::Stretch { .. } => unreachable!(),
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
        // rvmm 1001ff10 interpolates in Q13. A zero source end extent
        // preserves the initial extent. Destination zero remains zero.
        let src: [i64; 4] = std::array::from_fn(|i| {
            let end = if i >= 2 && parameters.source_end[i] == 0 {
                start[i]
            } else {
                parameters.source_end[i]
            };
            i64::from(start[i]) * (8192 - p) + i64::from(end) * p
        });
        let dst: [i64; 4] = std::array::from_fn(|i| {
            i64::from(self.rectangle[i]) * (8192 - p) + i64::from(parameters.destination_end[i]) * p
        });
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
