//! AGES-ACS headphone placement. The retail plugin (Plugins/age_acs.rpo)
//! embeds 72 left-ear impulse responses of the Nagoya University (Itakura
//! laboratory) HRTF set: 256 time-aligned taps each, laid out like the MIT
//! KEMAR files (`elev0\L0e%03da.dat`) for azimuths 0..180 and then 355..185
//! degrees, counter-clockwise from the front. The right ear uses the mirrored
//! azimuth; the interaural delay, removed from the table, is added back.
use anyhow::{Context, Result, ensure};
use std::path::Path;

pub const TAPS: usize = 256;
const DIRECTIONS: usize = 72;
/// Virtual address of the table in the plugin image (age_acs 1000c720).
const TABLE: u32 = 0x1001_5468;
/// Woodworth head model: radius (m) and speed of sound (m/s).
const HEAD_RADIUS: f64 = 0.0875;
const SPEED_OF_SOUND: f64 = 343.0;

pub struct Hrtf {
    left: Vec<[f32; TAPS]>,
}

impl Hrtf {
    /// Read the table from the game's own plugin.
    pub fn load(game_dir: &Path) -> Result<Self> {
        let path = game_dir.join("Plugins").join("age_acs.rpo");
        let image = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        Self::from_image(&image)
    }
    pub fn from_image(image: &[u8]) -> Result<Self> {
        let u16_at = |o: usize| -> Result<u16> {
            Ok(u16::from_le_bytes(
                image.get(o..o + 2).context("truncated PE")?.try_into()?,
            ))
        };
        let u32_at = |o: usize| -> Result<u32> {
            Ok(u32::from_le_bytes(
                image.get(o..o + 4).context("truncated PE")?.try_into()?,
            ))
        };
        let pe = u32_at(0x3c)? as usize;
        ensure!(image.get(pe..pe + 4) == Some(b"PE\0\0"), "not a PE image");
        let sections = usize::from(u16_at(pe + 6)?);
        let optional = usize::from(u16_at(pe + 20)?);
        let base = u32_at(pe + 24 + 28)?;
        let rva = TABLE.checked_sub(base).context("plugin image base")?;
        let bytes = DIRECTIONS * TAPS * 4;
        let offset = (0..sections)
            .find_map(|i| {
                let s = pe + 24 + optional + 40 * i;
                let (address, raw_size, raw) = (
                    u32_at(s + 12).ok()?,
                    u32_at(s + 16).ok()?,
                    u32_at(s + 20).ok()?,
                );
                (address <= rva && rva + bytes as u32 <= address + raw_size)
                    .then(|| (raw + rva - address) as usize)
            })
            .context("HRTF table outside the plugin sections")?;
        let data = image
            .get(offset..offset + bytes)
            .context("truncated HRTF table")?;
        let left: Vec<[f32; TAPS]> = data
            .as_chunks::<{ TAPS * 4 }>()
            .0
            .iter()
            .map(|chunk| {
                let (samples, _) = chunk.as_chunks::<4>();
                std::array::from_fn(|i| f32::from_le_bytes(samples[i]))
            })
            .collect();
        // Every response is finite and peaks at its centre tap.
        ensure!(
            left.iter().all(|h| {
                h.iter().all(|v| v.is_finite() && v.abs() < 4.0)
                    && (0..TAPS).max_by(|&a, &b| h[a].abs().total_cmp(&h[b].abs()))
                        == Some(TAPS / 2)
            }),
            "unexpected HRTF table layout"
        );
        Ok(Self { left })
    }
    /// Left-ear response for a counter-clockwise azimuth (degrees).
    fn left_ear(&self, azimuth: f64) -> &[f32; TAPS] {
        let degrees = (azimuth.rem_euclid(360.0) / 5.0).round() as usize * 5 % 360;
        let index = if degrees <= 180 {
            degrees / 5
        } else {
            37 + (355 - degrees) / 5
        };
        &self.left[index]
    }
    /// Pseudo-binaural rendering of a mono signal for a source at `azimuth`
    /// degrees clockwise from the front (positive = right, as the port's
    /// packet coordinates). Output frames keep the input length; a looping
    /// sound is convolved circularly.
    pub fn render(
        &self,
        mono: &[f32],
        azimuth: f64,
        sample_rate: u32,
        looped: bool,
    ) -> Vec<[f32; 2]> {
        let ccw = -azimuth;
        let (left, right) = (self.left_ear(ccw), self.left_ear(-ccw));
        let (delay_left, delay_right) = interaural_delays(azimuth, sample_rate);
        let left = convolve(mono, left, delay_left, looped);
        let right = convolve(mono, right, delay_right, looped);
        left.into_iter().zip(right).map(|(l, r)| [l, r]).collect()
    }
}

/// Far-ear delay in samples (Woodworth), the near ear undelayed.
pub fn interaural_delays(azimuth: f64, sample_rate: u32) -> (usize, usize) {
    let lateral = azimuth.to_radians().sin().clamp(-1.0, 1.0).asin();
    let seconds = HEAD_RADIUS / SPEED_OF_SOUND * (lateral.abs() + lateral.abs().sin());
    let samples = (seconds * f64::from(sample_rate)).round() as usize;
    if lateral > 0.0 {
        (samples, 0)
    } else {
        (0, samples)
    }
}

/// Convolve with a centre-aligned response, then delay.
fn convolve(input: &[f32], response: &[f32; TAPS], delay: usize, looped: bool) -> Vec<f32> {
    let n = input.len();
    let mut out = vec![0.0f32; n];
    if n == 0 {
        return out;
    }
    let centre = (TAPS / 2) as isize;
    for (k, value) in out.iter_mut().enumerate() {
        let mut acc = 0.0f32;
        for (t, &h) in response.iter().enumerate() {
            if h == 0.0 {
                continue;
            }
            let j = k as isize - delay as isize - (t as isize - centre);
            let sample = if looped {
                input[j.rem_euclid(n as isize) as usize]
            } else if (0..n as isize).contains(&j) {
                input[j as usize]
            } else {
                continue;
            };
            acc += sample * h;
        }
        *value = acc;
    }
    out
}

/// Emulation models (no measured filters): interaural delay plus a one-pole
/// head-shadow low-pass on the far ear; `bone` adds the near signal to the
/// far ear (the 回折・骨導音 variant).
pub fn emulate(mono: &[f32], azimuth: f64, sample_rate: u32, bone: bool) -> Vec<[f32; 2]> {
    let (delay_left, delay_right) = interaural_delays(azimuth, sample_rate);
    let lateral = azimuth.to_radians().sin();
    let cutoff = 20_000.0 - 18_500.0 * lateral.abs();
    let a = (-std::f64::consts::TAU * cutoff / f64::from(sample_rate)).exp() as f32;
    let mut state = 0.0f32;
    let delayed = |i: usize, d: usize| if i >= d { mono[i - d] } else { 0.0 };
    (0..mono.len())
        .map(|i| {
            let (near, far_delay) = if lateral >= 0.0 {
                (delayed(i, delay_right), delay_left)
            } else {
                (delayed(i, delay_left), delay_right)
            };
            state = (1.0 - a) * delayed(i, far_delay) + a * state;
            let far = if bone {
                state * 0.8 + near * 0.2
            } else {
                state
            };
            if lateral >= 0.0 {
                [far, near]
            } else {
                [near, far]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn far_ear_is_delayed_and_index_layout_follows_the_plugin_loop() {
        let (l, r) = interaural_delays(90.0, 44_100);
        assert!(l > 25 && l < 32 && r == 0, "{l} {r}");
        assert_eq!(interaural_delays(0.0, 44_100), (0, 0));
        let table = Hrtf {
            left: (0..DIRECTIONS).map(|i| [i as f32; TAPS]).collect(),
        };
        assert_eq!(table.left_ear(0.0)[0], 0.0);
        assert_eq!(table.left_ear(180.0)[0], 36.0);
        assert_eq!(table.left_ear(355.0)[0], 37.0);
        assert_eq!(table.left_ear(-5.0)[0], 37.0);
        assert_eq!(table.left_ear(185.0)[0], 71.0);
    }
    #[test]
    fn retail_hrtf_places_a_right_source_louder_in_the_right_ear() -> Result<()> {
        let Some(root) = std::env::var_os("RUGP_TEST_GAME") else {
            return Ok(());
        };
        let hrtf = Hrtf::load(Path::new(&root))?;
        let noise: Vec<f32> = (0..4096u32)
            .map(|i| ((i.wrapping_mul(2_654_435_761) >> 16) as f32 / 32768.0) - 1.0)
            .collect();
        let energy =
            |frames: &[[f32; 2]], c: usize| frames.iter().map(|f| f[c] * f[c]).sum::<f32>();
        let right = hrtf.render(&noise, 60.0, 44_100, false);
        assert!(energy(&right, 1) > 2.0 * energy(&right, 0));
        let left = hrtf.render(&noise, -60.0, 44_100, false);
        assert!(energy(&left, 0) > 2.0 * energy(&left, 1));
        Ok(())
    }
}
