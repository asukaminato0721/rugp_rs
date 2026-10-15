//! Portable static stereo approximation of native CAcsPos packets. PCM clocks,
//! envelopes and stop operations remain independent of sound placement.
use anyhow::{Result, ensure};

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StereoGains {
    pub left: f32,
    pub right: f32,
}
impl Default for StereoGains {
    fn default() -> Self {
        Self::UNITY
    }
}
impl StereoGains {
    pub const UNITY: Self = Self {
        left: 1.0,
        right: 1.0,
    };
    pub fn validate(self) -> Result<()> {
        ensure!(
            [self.left, self.right]
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
            "invalid stereo sound gains"
        );
        Ok(())
    }
    /// Vm60 10073717..10073763 reads signed range/X/Y/Z words followed,
    /// without bit 0x10, by three motion words. The archive has 8/14 bytes;
    /// the native in-memory instruction pads the latter to 16 bytes.
    /// This approximation uses initial placement relative to a fixed listener
    /// at the origin, not the native moving listener/cone or motion engine.
    pub fn from_position(flags: u8, bytes: &[u8]) -> Result<Self> {
        ensure!(
            flags & 4 != 0 && flags & 8 == 0,
            "not an inline acoustic position"
        );
        let expected = if flags & 16 != 0 { 8 } else { 14 };
        ensure!(
            bytes.len() == expected,
            "invalid acoustic position packet size"
        );
        let word = |i| f32::from(i16::from_le_bytes([bytes[i], bytes[i + 1]]));
        let radius = word(0);
        let [x, y, z] = [word(2), word(4), word(6)];
        let distance = (x * x + y * y + z * z).sqrt();
        let attenuation = if radius > 0.0 && distance > radius {
            radius / distance
        } else {
            1.0
        };
        let horizontal = x.hypot(z);
        let pan = if horizontal > 0.0 {
            (x / horizontal).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let gains = Self {
            left: attenuation * (1.0 - pan.max(0.0)),
            right: attenuation * (1.0 + pan.min(0.0)),
        };
        gains.validate()?;
        Ok(gains)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_inline_position_balances_and_attenuates_without_invalid_gains() -> Result<()> {
        let packet = |words: [i16; 4]| {
            words
                .into_iter()
                .flat_map(i16::to_le_bytes)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            StereoGains::from_position(0x14, &packet([1000, 0, 0, 0]))?,
            StereoGains::UNITY
        );
        let left = StereoGains::from_position(0x14, &packet([1000, -2000, 0, 2000]))?;
        let right = StereoGains::from_position(0x14, &packet([1000, 2000, 0, 2000]))?;
        assert!(0.0 < left.right && left.right < left.left && left.left < 1.0);
        assert_eq!(left.left, right.right);
        assert_eq!(left.right, right.left);
        for radius in [i16::MIN, 0, 1, i16::MAX] {
            for point in [[i16::MIN; 3], [i16::MAX; 3], [0; 3]] {
                StereoGains::from_position(0x14, &packet([radius, point[0], point[1], point[2]]))?
                    .validate()?;
            }
        }
        let mut full = packet([1000, -2000, 0, 2000]);
        full.extend_from_slice(&[1, 0, 2, 0, 3, 0]);
        assert_eq!(StereoGains::from_position(4, &full)?, left);
        assert!(StereoGains::from_position(4, &full[..8]).is_err());
        assert!(StereoGains::from_position(0x14, &full).is_err());
        assert!(StereoGains::from_position(0x1c, &full).is_err());
        for gains in [
            StereoGains {
                left: f32::NAN,
                right: 1.0,
            },
            StereoGains {
                left: -0.1,
                right: 1.0,
            },
            StereoGains {
                left: 1.0,
                right: 1.1,
            },
        ] {
            assert!(gains.validate().is_err());
        }
        Ok(())
    }
}
