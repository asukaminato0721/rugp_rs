//! Portable motion sampling. rvmm 10006df0/10006f20 select a segment;
//! 10004f70 and 10005570..10005abb interpolate it. Native uses x87 extended
//! intermediates; this implementation uses f64 and is not bit-identical at
//! floating-point rounding boundaries. Integer conversion retains _ftol's
//! truncation and low-32-bit behavior, not Rust's saturating i32 cast.
use anyhow::{Result, ensure};
use rugp_assets::motion::{MotionCurve, MotionSegment, TimeCurve};

#[derive(Debug, Clone)]
pub struct Motion {
    curve: MotionCurve,
    duration: i32,
    segment_duration: i32,
}
impl Motion {
    pub fn new(curve: MotionCurve) -> Result<Self> {
        ensure!(
            (-1..=1).contains(&curve.timing_mode),
            "unsupported motion timing mode {}",
            curve.timing_mode
        );
        ensure!(
            curve.flags & !3 == 0,
            "unsupported motion flags {:#x}",
            curve.flags
        );
        let mut segment_duration = 0i32;
        for segment in &curve.segments {
            ensure!(
                segment.duration_ms > 0,
                "nonpositive motion segment duration is not implemented yet"
            );
            ensure!(
                matches!(segment.kind, 2 | 3),
                "unsupported motion segment kind"
            );
            ensure!(
                segment.kind != 2 || segment.controls.is_empty(),
                "line has control points"
            );
            ensure!(
                segment.controls.len() <= 32,
                "high-degree motion Bezier is not implemented yet"
            );
            if let Some(TimeCurve::Bezier(knots)) = &segment.time_curve {
                ensure!(knots.len() >= 2, "time Bezier needs two knots");
                ensure!(
                    knots.iter().flatten().all(|v| v.is_finite()),
                    "non-finite time Bezier"
                );
                ensure!(
                    knots.windows(2).all(|pair| pair[0][0] < pair[1][0]),
                    "unordered time Bezier knots"
                );
            }
            segment_duration = segment_duration
                .checked_add(segment.duration_ms)
                .ok_or_else(|| anyhow::anyhow!("motion duration overflow"))?;
        }
        let duration = if curve.timing_mode == -1 {
            segment_duration
        } else {
            curve.duration_ms
        };
        ensure!(duration >= 0, "negative motion duration");
        Ok(Self {
            curve,
            duration,
            segment_duration,
        })
    }
    pub fn duration_ms(&self) -> i32 {
        self.duration
    }
    pub fn loops(&self) -> bool {
        self.curve.flags & 2 != 0
    }
    pub fn expired(&self, elapsed_ms: i32) -> bool {
        elapsed_ms > self.duration
    }
    pub fn end(&self) -> [i32; 3] {
        self.curve
            .segments
            .last()
            .map_or(self.curve.start, |s| s.end)
    }
    /// The last millisecond belongs to the final segment. Loop wrapping only
    /// happens *after* that inclusive endpoint (10006e45/10006f5d).
    pub fn sample(&self, mut elapsed_ms: i32) -> [i32; 3] {
        if elapsed_ms < 0 {
            return self.curve.start;
        }
        if elapsed_ms > self.duration {
            if self.loops() && self.duration > 0 {
                elapsed_ms %= self.duration;
            } else {
                return if self.curve.flags & 1 != 0 {
                    self.end()
                } else {
                    self.curve.start
                };
            }
        }
        if self.curve.segments.is_empty() {
            return self.curve.start;
        }
        if self.duration == 0 {
            return self.end();
        }
        let mut start = self.curve.start;
        if self.curve.timing_mode == -1 {
            let mut begin = 0;
            for segment in &self.curve.segments {
                let end = begin + segment.duration_ms;
                if (begin..=end).contains(&elapsed_ms) {
                    return sample_segment(segment, start, elapsed_ms - begin);
                }
                start = segment.end;
                begin = end;
            }
        } else {
            let progress = f64::from(elapsed_ms) / f64::from(self.duration);
            let mut begin = 0.0;
            for segment in &self.curve.segments {
                let weight = if self.curve.timing_mode == 0 {
                    1.0 / self.curve.segments.len() as f64
                } else {
                    f64::from(segment.duration_ms) / f64::from(self.segment_duration)
                };
                if progress >= begin && progress <= begin + weight {
                    let local = ftol((progress - begin) / weight * f64::from(segment.duration_ms));
                    return sample_segment(segment, start, local);
                }
                start = segment.end;
                begin += weight;
            }
        }
        // Floating-point interval accumulation can leave 1.0 just outside the
        // final interval. Native 100077af then chooses the configured endpoint.
        if self.curve.flags & 1 != 0 {
            self.end()
        } else {
            self.curve.start
        }
    }
}

pub(crate) fn ftol(value: f64) -> i32 {
    if (-9223372036854775808.0..9223372036854775808.0).contains(&value) {
        value as i64 as i32
    } else {
        0
    }
}
fn sample_segment(segment: &MotionSegment, start: [i32; 3], elapsed: i32) -> [i32; 3] {
    let mut t = f64::from(elapsed) / f64::from(segment.duration_ms);
    if let Some(curve) = &segment.time_curve {
        t = time_sample(curve, t);
    }
    if segment.kind == 2 || segment.controls.is_empty() {
        return std::array::from_fn(|axis| {
            ftol(f64::from(start[axis]) * (1.0 - t) + f64::from(segment.end[axis]) * t)
        });
    }
    // Native rejects out-of-range Bezier t; supported timing objects keep it
    // within [0,1]. No intermediate integer truncation (unlike integer de Casteljau).
    let degree = segment.controls.len() + 1;
    let factorial = |n: usize| (1..=n).rev().fold(1.0, |a, v| a * v as f64);
    let mut result = [0.0; 3];
    for (index, point) in std::iter::once(&start)
        .chain(&segment.controls)
        .chain(std::iter::once(&segment.end))
        .enumerate()
    {
        let coefficient = factorial(degree) / (factorial(index) * factorial(degree - index));
        let weight = coefficient * (1.0 - t).powf((degree - index) as f64) * t.powf(index as f64);
        for axis in 0..3 {
            result[axis] += weight * f64::from(point[axis]);
        }
    }
    result.map(ftol)
}

fn time_sample(curve: &TimeCurve, x: f64) -> f64 {
    match curve {
        TimeCurve::Quadratic(1) => 1.0 - (x - 1.0) * (x - 1.0),
        TimeCurve::Quadratic(_) => x * x,
        TimeCurve::Bezier(knots) => {
            // rvmm 10003e10 picks a span; knot controls are relative offsets.
            let pair = knots
                .windows(2)
                .find(|p| f64::from(p[0][0]) <= x && x <= f64::from(p[1][0]))
                .unwrap_or_else(|| &knots[knots.len() - 2..]);
            let left = pair[0];
            let right = pair[1];
            // Add offsets in f32, as the native span setup stores back to f32.
            let controls = [
                [left[0], left[1]],
                [left[0] + left[4], left[1] + left[5]],
                [right[0] + right[2], right[1] + right[3]],
                [right[0], right[1]],
            ];
            let cubic = |t: f64, axis: usize| {
                let u = 1.0 - t;
                f64::from(controls[0][axis]) * u * u * u
                    + 3.0 * f64::from(controls[1][axis]) * u * u * t
                    + 3.0 * f64::from(controls[2][axis]) * u * t * t
                    + f64::from(controls[3][axis]) * t * t * t
            };
            let mut previous_x = f64::from(left[0]);
            let mut previous_t = 0.0;
            for i in 1..=40 {
                let t = f64::from(i) * 0.025;
                let next_x = cubic(t, 0);
                if x >= previous_x && x <= next_x {
                    let ratio = (x - previous_x) / (next_x - previous_x);
                    return (cubic(previous_t, 1) * (1.0 - ratio) + cubic(t, 1) * ratio)
                        .clamp(0.0, 1.0);
                }
                previous_x = next_x;
                previous_t = t;
            }
            // Native's no-crossing fallback returns the last t, not its Y.
            previous_t.clamp(0.0, 1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn curve(mode: i32) -> MotionCurve {
        MotionCurve {
            start: [0; 3],
            segments: vec![
                MotionSegment {
                    kind: 2,
                    options: 1,
                    duration_ms: 100,
                    end: [100, -100, 0],
                    controls: vec![],
                    time_curve: None,
                },
                MotionSegment {
                    kind: 2,
                    options: 1,
                    duration_ms: 300,
                    end: [300, -300, 0],
                    controls: vec![],
                    time_curve: None,
                },
            ],
            timing_mode: mode,
            duration_ms: 800,
            rate: 1000.0,
            dimensions: 1,
            flags: 1,
        }
    }
    #[test]
    fn native_timing_modes_and_inclusive_expiry() -> Result<()> {
        let sequential = Motion::new(curve(-1))?;
        assert_eq!(sequential.duration_ms(), 400);
        for (t, x) in [
            (-1, 0),
            (0, 0),
            (50, 50),
            (100, 100),
            (250, 200),
            (400, 300),
            (401, 300),
        ] {
            assert_eq!(sequential.sample(t), [x, -x, 0], "{t}");
        }
        assert!(!sequential.expired(400));
        assert!(sequential.expired(401));
        let equal = Motion::new(curve(0))?;
        let weighted = Motion::new(curve(1))?;
        assert_eq!(equal.duration_ms(), 800);
        assert_eq!(equal.sample(200), [50, -50, 0]);
        assert_eq!(weighted.sample(200), [100, -100, 0]);
        assert_eq!(equal.sample(600), [200, -200, 0]);
        assert_eq!(weighted.sample(500), [200, -200, 0]);
        Ok(())
    }
    #[test]
    fn looping_endpoint_flags_empty_and_instant_motion() -> Result<()> {
        for mode in [-1, 0, 1] {
            let mut c = curve(mode);
            c.flags = 3;
            let motion = Motion::new(c.clone())?;
            let duration = motion.duration_ms();
            assert!(motion.loops());
            assert_eq!(motion.sample(duration), motion.end());
            assert_eq!(motion.sample(duration + 1), motion.sample(1));
            assert_eq!(motion.sample(duration * 2), motion.sample(0));
            c.flags = 0;
            let motion = Motion::new(c.clone())?;
            assert_eq!(motion.sample(duration), [300, -300, 0]);
            assert_eq!(motion.sample(duration + 1), [0; 3]);
        }
        let mut c = curve(0);
        c.duration_ms = 0;
        assert_eq!(Motion::new(c.clone())?.sample(0), [300, -300, 0]);
        c.segments.clear();
        c.start = [1, -2, 3];
        assert_eq!(Motion::new(c)?.sample(0), [1, -2, 3]);
        Ok(())
    }
    #[test]
    fn bernstein_sums_truncate_only_after_the_final_sum() {
        let s = MotionSegment {
            kind: 3,
            options: 1,
            duration_ms: 100,
            end: [8, -8, 8],
            controls: vec![[0, 0, 8], [0, 0, 8]],
            time_curve: None,
        };
        assert_eq!(sample_segment(&s, [0; 3], 50), [1, -1, 7]);
        assert_eq!(sample_segment(&s, [0; 3], 25), [0, 0, 4]);
        assert_eq!(sample_segment(&s, [0; 3], 100), [8, -8, 8]);
        let mut quadratic = s;
        quadratic.time_curve = Some(TimeCurve::Quadratic(0));
        assert_eq!(sample_segment(&quadratic, [0; 3], 50), [0, 0, 4]);
        quadratic.time_curve = Some(TimeCurve::Quadratic(1));
        assert_eq!(sample_segment(&quadratic, [0; 3], 50), [3, -3, 7]);
        assert_eq!(ftol(-0.999), 0);
        assert_eq!(ftol(2147483648.0), i32::MIN);
        assert_eq!(ftol(4294967297.9), 1);
        assert_eq!(ftol(f64::INFINITY), 0);
        assert_eq!(ftol(f64::NAN), 0);
    }
    #[test]
    fn time_bezier_uses_forty_linear_crossings_not_exact_inverse() {
        let time = TimeCurve::Bezier(vec![[0.0; 6], [1.0, 1.0, -1.0, 0.0, 0.0, 0.0]]);
        let x = 0.1;
        let (a, b) = (0.45f64, 0.475f64);
        let fraction = (x - a.powi(3)) / (b.powi(3) - a.powi(3));
        let y = |t: f64| 3.0 * t * t - 2.0 * t * t * t;
        let expected = y(a) * (1.0 - fraction) + y(b) * fraction;
        assert!((time_sample(&time, x) - expected).abs() < 1e-14);
        assert!((time_sample(&time, x) - y(x.cbrt())).abs() > 1e-5);
        assert_eq!(time_sample(&time, 0.0), 0.0);
        assert_eq!(time_sample(&time, 1.0), 1.0);
        assert_eq!(time_sample(&TimeCurve::Quadratic(1), 0.25), 0.4375);
        assert_eq!(time_sample(&TimeCurve::Quadratic(-7), 0.25), 0.0625);
    }
    #[test]
    fn invalid_modes_degenerate_segments_and_timing_knots_are_explicit() {
        let mut c = curve(2);
        assert!(Motion::new(c.clone()).is_err());
        c.timing_mode = 0;
        c.flags = 4;
        assert!(Motion::new(c.clone()).is_err());
        c.flags = 1;
        c.segments[0].duration_ms = 0;
        assert!(Motion::new(c.clone()).is_err());
        c.segments[0].duration_ms = 100;
        c.segments[0].time_curve = Some(TimeCurve::Bezier(vec![]));
        assert!(Motion::new(c).is_err());
    }
}
