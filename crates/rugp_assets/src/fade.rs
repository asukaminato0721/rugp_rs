//! Common CFade#12 parameters, after ObjectReader has resolved its three references.
use crate::binary::Reader;
use anyhow::{Result, ensure};

#[derive(Debug, Clone)]
pub struct FadeCurve(Vec<(u16, i16)>);
impl FadeCurve {
    /// Native interpolation uses a 0..256 time axis and signed 1/32 alpha units.
    pub fn alpha(&self, elapsed: u32, duration: u32) -> u16 {
        let x = if duration == 0 {
            256
        } else {
            (u64::from(elapsed.min(duration)) * 256 / u64::from(duration)) as i32
        };
        let Some(&(first_x, first_y)) = self.0.first() else {
            return x as u16;
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
        (y >> 5).clamp(0, 256) as u16
    }
}

#[derive(Debug, Clone)]
pub struct Fade {
    pub mode: u16,
    pub rectangles: [[i16; 4]; 3],
    pub curve: FadeCurve,
    /// Native COLORREF (B,G,R in the high-to-low three bytes).
    pub color: Option<[u8; 3]>,
}
impl Fade {
    /// `data` is the complete inline body already bounded by ObjectReader.
    /// The variable-sized reference encodings precede a fixed-size tail; they
    /// must be resolved by that reader's shared object cache, not reparsed here.
    pub fn from_inline(class: &str, data: &[u8]) -> Result<Self> {
        ensure!(
            matches!(
                class,
                "CFadeNormal" | "CFadeMergeBlack" | "CFadeMergeWhite" | "CFadeMergeColor"
            ),
            "unimplemented fade class {class}"
        );
        let mut r = Reader::new(data);
        let count = r.u16()? as usize;
        let mode = r.u16()?;
        let extra = usize::from(matches!(class, "CFadeMergeWhite" | "CFadeMergeColor")) * 4;
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
        let color = match class {
            "CFadeNormal" => None,
            "CFadeMergeBlack" => Some([0, 0, 0]),
            _ => {
                let c = r.u32()?;
                Some([c as u8, (c >> 8) as u8, (c >> 16) as u8])
            }
        };
        Ok(Self {
            mode,
            rectangles,
            curve: FadeCurve(knots),
            color,
        })
    }
}
