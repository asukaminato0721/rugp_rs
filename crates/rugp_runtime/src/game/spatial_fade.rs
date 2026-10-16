//! Pixel operators decoded from the retail rvmm.dll.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) enum Operator {
    Mosaic,
    Invert,
    RightCurtain,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SpatialFade {
    pub operator: Operator,
    pub rectangle: [i16; 4],
    pub previous_alpha: Option<u16>,
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
        })
    }

    pub fn keeps_previous_frame(&self) -> bool {
        matches!(self.operator, Operator::RightCurtain) && self.previous_alpha.is_some()
    }

    pub fn render(&mut self, output: &mut [u8], source: &[u8], size: (u32, u32), alpha: u16) {
        let [x, y, w, h] = self.rectangle.map(i64::from);
        let width = i64::from(size.0);
        let height = i64::from(size.1);
        let w = if w == 0 { width } else { w };
        let h = if h == 0 { height } else { h };
        let block = i64::from((256 - alpha) / 8 + 1);
        let previous = self.previous_alpha.replace(alpha).unwrap_or(0);
        let position = i64::from(alpha) * (w + 128) / 256 - 128;
        let old_position = i64::from(previous) * (w + 128) / 256 - 128;
        let strip_start = position.min(old_position);
        let strip_end = strip_start + (position - old_position).abs() + 128;
        for row in y.max(0)..(y + h).min(height) {
            for column in x.max(0)..(x + w).min(width) {
                let target = ((row * width + column) * 4) as usize;
                match self.operator {
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
                            output[target + channel] =
                                (u32::from(source[sample + channel]) * u32::from(alpha) >> 8) as u8;
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
                    Operator::RightCurtain => {
                        // rvmm 1003b8b0 updates only the moving strip. The
                        // 128-pixel feather blends with the previous frame.
                        let local = column - x;
                        if alpha == previous || local < strip_start || local >= strip_end {
                            continue;
                        }
                        let weight = (position + 127 - local).clamp(0, 128) as u32;
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
