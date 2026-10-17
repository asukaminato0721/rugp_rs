//! Common CFade#12 parameters, after ObjectReader has resolved its three references.
use crate::binary::Reader;
use anyhow::{Result, ensure};

/// Extra bytes around the common CFade#12 envelope. Shared with ObjectReader
/// so presentation fallbacks consume exactly the same bounded archive record.
pub(crate) fn envelope_layout(class: &str) -> Result<(usize, usize)> {
    let prefix = match class {
        "CFadeCarten" => 6,
        "CFadeSdtCircleRaster" => 15,
        _ => 0,
    };
    let tail = match class {
        "CFadeNormal"
        | "CFadeSdtRatio"
        | "CFadeRSideCarten"
        | "CFadeMozaik"
        | "CFadeMulHorz"
        | "CFadeInvert"
        | "CFadeShock"
        | "CFadeCarten"
        | "CFadeSdtCircleRaster"
        | "CFadeMergeBlack" => 0,
        "CFadeXsRatio" | "CFadeXsMergeBlack" => 6,
        "CFadeStretchAnti"
        | "CFadeStretch"
        | "CFadeXsSqrRaster_HRasterV_VRasterH"
        | "CFadeXsHRasterHOffset" => 17,
        "CFadeOverStretchAnti" => 25,
        "CFadeXsOverStretchAnti" => 30,
        "CFadeQubeStretchAnti" => 44,
        "CFadeXsRasterNoize" => 23,
        "CFadeXsSrcRotate" => 24,
        "CFadeXsCircleRaster" => 21,
        "CFadeMergeWhite" | "CFadeMergeColor" => 4,
        _ => anyhow::bail!("unsupported inline effect {class}"),
    };
    Ok((prefix, tail))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FadeCurve(Vec<(u16, i16)>);
impl<'de> serde::Deserialize<'de> for FadeCurve {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let curve = Self(<Vec<(u16, i16)> as serde::Deserialize>::deserialize(
            deserializer,
        )?);
        curve
            .validate_snapshot()
            .map_err(serde::de::Error::custom)?;
        Ok(curve)
    }
}
impl FadeCurve {
    pub fn validate_snapshot(&self) -> Result<()> {
        ensure!(
            self.0.len() <= 257
                && self
                    .0
                    .iter()
                    .all(|(x, y)| *x <= 256 && (0..=8192).contains(y))
                && self.0.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "invalid fade snapshot curve"
        );
        Ok(())
    }
    /// Native interpolation uses a 0..256 time axis and signed 1/32 alpha units.
    pub fn alpha(&self, elapsed: u32, duration: u32) -> u16 {
        self.value(elapsed, duration) >> 5
    }
    /// Preserve Q13 precision for geometry and OverStretch opacity.
    pub fn value(&self, elapsed: u32, duration: u32) -> u16 {
        let x = if duration == 0 {
            256
        } else {
            (u64::from(elapsed.min(duration)) * 256 / u64::from(duration)) as i32
        };
        let Some(&(first_x, first_y)) = self.0.first() else {
            return (x * 32) as u16;
        };
        let mut y = i32::from(first_y);
        if x > i32::from(first_x) {
            for pair in self.0.windows(2) {
                let [(x0, y0), (x1, y1)] = pair else {
                    unreachable!()
                };
                y = i32::from(*y1);
                if x < i32::from(*x1) {
                    y = i32::from(*y0)
                        + (i32::from(*y1) - i32::from(*y0)) * (x - i32::from(*x0))
                            / i32::from(*x1 - *x0);
                    break;
                }
            }
        }
        y.clamp(0, 8192) as u16
    }
}

#[derive(Debug, Clone)]
pub struct Fade {
    pub mode: u16,
    pub rectangles: [[i16; 4]; 3],
    pub curve: FadeCurve,
    /// Native COLORREF (B,G,R in the high-to-low three bytes).
    pub color: Option<[u8; 3]>,
    pub stretch: Option<Stretch>,
    pub curtain: Option<Curtain>,
    pub qube: Option<Qube>,
}
/// CFadeQubeStretchAnti#12 after its CFadeStretchAnti base (rvmm 10020570).
/// The face warp tilts about the screen axes; the blend weighs the warped
/// source against the destination, both in 1/256 units.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Qube {
    pub reserved: i16,
    /// Source weight, then destination weight, each from start to end.
    pub source_weight: [i16; 2],
    pub destination_weight: [i16; 2],
    /// Unused by the renderer; the vertical pivot when `pivot_from_rect` is 0.
    pub center: [i16; 2],
    /// Tilts about the vertical and horizontal axes (4096 per turn), then
    /// one more unused axis; start triple and end triple.
    pub angles: [[i16; 3]; 2],
    /// Nonzero: pivot at the vertical centre of the interpolated destination.
    pub pivot_from_rect: u8,
}
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Curtain {
    pub feather: i16,
    pub auxiliary: i16,
    /// 0: left to right, 1: right to left, 2: down, 3: up.
    pub direction: u8,
    pub options: u8,
}
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stretch {
    pub destination_end: [i16; 4],
    pub source_end: [i16; 4],
    pub options: u8,
    pub frames: i16,
    pub opacity: [i16; 2],
}
impl Fade {
    /// `data` is the complete inline body already bounded by ObjectReader.
    /// The variable-sized reference encodings precede a fixed-size tail; they
    /// must be resolved by that reader's shared object cache, not reparsed here.
    pub fn from_inline(class: &str, data: &[u8]) -> Result<Self> {
        let (prefix, extra) = envelope_layout(class)?;
        let mut r = Reader::new(data);
        let curtain = if class == "CFadeCarten" {
            Some(Curtain {
                feather: r.u16()? as i16,
                auxiliary: r.u16()? as i16,
                direction: r.u8()?,
                options: r.u8()?,
            })
        } else {
            r.skip(prefix)?;
            None
        };
        let count = r.u16()? as usize;
        let mode = r.u16()?;
        let tail = 24 + count * 4 + extra;
        ensure!(
            r.remaining() >= 6 + tail,
            "truncated fade references/parameters"
        );
        r.skip(r.remaining() - tail)?;
        let mut rectangles = [[0; 4]; 3];
        for rectangle in &mut rectangles {
            for value in rectangle {
                *value = r.u16()? as i16;
            }
        }
        let mut knots = Vec::with_capacity(count);
        for _ in 0..count {
            knots.push((r.u16()?, r.u16()? as i16));
        }
        ensure!(
            knots
                .iter()
                .all(|(x, y)| *x <= 256 && (0..=8192).contains(y)),
            "invalid fade curve range"
        );
        ensure!(
            knots.windows(2).all(|p| p[0].0 < p[1].0),
            "unordered fade curve"
        );
        let parameters = r.position;
        let stretch = if matches!(
            class,
            "CFadeStretch" | "CFadeStretchAnti" | "CFadeOverStretchAnti" | "CFadeQubeStretchAnti"
        ) {
            let mut destination_end = [0; 4];
            let mut source_end = [0; 4];
            for value in destination_end.iter_mut().chain(&mut source_end) {
                *value = r.u16()? as i16;
            }
            let options = r.u8()?;
            let (frames, opacity) = if class == "CFadeOverStretchAnti" {
                let frames = r.u16()? as i16;
                let opacity = [r.u16()? as i16, r.u16()? as i16];
                r.u16()?;
                (frames, opacity)
            } else {
                (0, [8192; 2])
            };
            Some(Stretch {
                destination_end,
                source_end,
                options,
                frames,
                opacity,
            })
        } else {
            None
        };
        let qube = if class == "CFadeQubeStretchAnti" {
            let mut words = [0i16; 13];
            for word in &mut words {
                *word = r.u16()? as i16;
            }
            Some(Qube {
                reserved: words[0],
                source_weight: [words[1], words[2]],
                destination_weight: [words[3], words[4]],
                center: [words[5], words[6]],
                angles: [
                    [words[7], words[8], words[9]],
                    [words[10], words[11], words[12]],
                ],
                pivot_from_rect: r.u8()?,
            })
        } else {
            None
        };
        r.position = parameters;
        let color = match class {
            "CFadeMergeBlack" | "CFadeXsMergeBlack" => Some([0, 0, 0]),
            "CFadeMergeWhite" | "CFadeMergeColor" => {
                let c = r.u32()?;
                Some([c as u8, (c >> 8) as u8, (c >> 16) as u8])
            }
            // Specialized spatial/distortion effects use their common
            // endpoints and curve, without native displacement/deformation.
            _ => None,
        };
        Ok(Self {
            mode,
            rectangles,
            curve: FadeCurve(knots),
            color,
            stretch,
            curtain,
            qube,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn curtain_prefix_precedes_the_common_reference_and_curve_envelope() -> Result<()> {
        let mut body = vec![128, 0, 0, 0, 2, 0, 2, 0, 0x11, 0];
        body.extend_from_slice(&[0; 6]);
        for word in [0u16, 0, 1024, 600].into_iter().chain([0; 8]) {
            body.extend_from_slice(&word.to_le_bytes());
        }
        for word in [0u16, 0, 256, 8192] {
            body.extend_from_slice(&word.to_le_bytes());
        }
        let fade = Fade::from_inline("CFadeCarten", &body)?;
        let curtain = fade.curtain.unwrap();
        assert_eq!(curtain.feather, 128);
        assert_eq!(curtain.direction, 2);
        assert_eq!(fade.mode, 0x11);
        assert_eq!(fade.rectangles[0], [0, 0, 1024, 600]);
        assert_eq!(fade.curve.alpha(128, 256), 128);
        assert_eq!(fade.curve.alpha(256, 256), 256);
        assert!(Fade::from_inline("CFadeCarten", &body[..5]).is_err());
        Ok(())
    }
}
