//! Shared viewport math for presentation and logical input coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
    game_width: u32,
    game_height: u32,
}
impl Viewport {
    pub fn fit(
        window_width: u32,
        window_height: u32,
        game_width: u32,
        game_height: u32,
    ) -> Option<Self> {
        if [window_width, window_height, game_width, game_height].contains(&0) {
            return None;
        }
        let width = u64::from(window_width)
            .min(u64::from(window_height) * u64::from(game_width) / u64::from(game_height))
            .max(1) as u32;
        let height =
            (u64::from(width) * u64::from(game_height) / u64::from(game_width)).max(1) as u32;
        Some(Self {
            left: (window_width - width) / 2,
            top: (window_height - height) / 2,
            width,
            height,
            game_width,
            game_height,
        })
    }
    pub fn point(self, x: f64, y: f64) -> Option<(i32, i32)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let x = x - f64::from(self.left);
        let y = y - f64::from(self.top);
        if x < 0.0 || y < 0.0 || x >= f64::from(self.width) || y >= f64::from(self.height) {
            return None;
        }
        Some((
            (x * f64::from(self.game_width) / f64::from(self.width)) as i32,
            (y * f64::from(self.game_height) / f64::from(self.height)) as i32,
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn letterbox_edges_resize_and_nonfinite_positions() {
        let v = Viewport::fit(1280, 720, 1024, 600).unwrap();
        assert_eq!((v.left, v.top, v.width, v.height), (26, 0, 1228, 719));
        assert_eq!(v.point(26.0, 0.0), Some((0, 0)));
        assert_eq!(v.point(1253.999, 718.999), Some((1023, 599)));
        for (x, y) in [
            (25.0, 0.0),
            (1254.0, 0.0),
            (30.0, 719.0),
            (f64::NAN, 0.0),
            (f64::INFINITY, 0.0),
        ] {
            assert_eq!(v.point(x, y), None);
        }
        assert_eq!(
            Viewport::fit(1, 1, 1024, 600).unwrap().point(0.0, 0.0),
            Some((0, 0))
        );
        assert!(Viewport::fit(0, 1, 1024, 600).is_none());
    }
}
