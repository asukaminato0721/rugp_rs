//! Perspective-correct nearest-neighbor rasterization for world image planes.
//! Screen-space clipping is separate from camera/near-plane clipping.
use super::{Framebuffer, Texture, source_over};
use anyhow::{Result, ensure};

#[derive(Clone, Copy)]
struct Vertex {
    xy: [f64; 2],
    inverse_z: f64,
    uv_over_z: [f64; 2],
}

fn texel_coordinate(uv: f64, extent: u32) -> usize {
    let value = uv * f64::from(extent);
    // Barycentric sums can land a few ulps below an exact texel boundary.
    // Define that tie consistently, independently of the chosen triangle.
    // This is numerical stabilization, not a half-texel sampling offset.
    let nearest = value.round();
    let value = if (value - nearest).abs() <= 8.0 * f64::EPSILON * value.abs().max(1.0) {
        nearest
    } else {
        value
    };
    value.floor().clamp(0.0, f64::from(extent - 1)) as usize
}

/// A directed edge equation. Reversing an edge negates all coefficients
/// exactly, including the shared diagonal: no double alpha blend or cracks.
struct Edge {
    a: f64,
    b: f64,
    c: f64,
    inclusive: bool,
}
impl Edge {
    fn new(p: [f64; 2], q: [f64; 2]) -> Self {
        Self {
            a: p[1] - q[1],
            b: q[0] - p[0],
            c: p[0] * q[1] - p[1] * q[0],
            // Top-left rule in a downwards-positive screen coordinate system.
            inclusive: q[1] < p[1] || (q[1] == p[1] && q[0] > p[0]),
        }
    }
    fn at(&self, p: [f64; 2]) -> f64 {
        self.a * p[0] + self.b * p[1] + self.c
    }
    fn covers(&self, value: f64) -> bool {
        value > 0.0 || (value == 0.0 && self.inclusive)
    }
}

impl Framebuffer {
    /// Draw the entire texture on a convex projected plane. Vertices are in
    /// image order (top-left, top-right, bottom-right, bottom-left), each
    /// `[screen_x, screen_y, positive_camera_depth]`. Either winding is valid.
    /// UVs and reciprocal depth are interpolated, not screen-space UVs.
    ///
    /// Samples are at pixel centers. `clip` is the object's screen viewport;
    /// it is intersected with the framebuffer before any pixel iteration.
    /// Wholly nonpositive-depth planes are culled. The caller must first clip
    /// geometry crossing the camera's near plane.
    /// Native world metadata/projection is intentionally not inferred here.
    pub fn blit_projective(
        &mut self,
        texture: &Texture,
        quad: [[f64; 3]; 4],
        clip: (i32, i32, u32, u32),
        opacity: u8,
    ) -> Result<()> {
        texture.validate()?;
        ensure!(
            quad.iter().all(|p| {
                p.iter().all(|v| v.is_finite())
                    && p[0].abs() <= i32::MAX as f64
                    && p[1].abs() <= i32::MAX as f64
                    && (i32::MIN as f64..=i32::MAX as f64).contains(&p[2])
            }),
            "invalid projected plane coordinates/depth; near-plane clipping required"
        );
        if quad.iter().all(|p| p[2] < 1e-9) {
            return Ok(());
        }
        ensure!(
            quad.iter().all(|p| p[2] >= 1e-9),
            "projected plane crosses the near plane; clipping required"
        );
        let positions = quad.map(|p| [p[0], p[1]]);
        let turns: [f64; 4] = std::array::from_fn(|i| {
            Edge::new(positions[i], positions[(i + 1) % 4]).at(positions[(i + 2) % 4])
        });
        ensure!(
            turns.iter().all(|v| *v >= 0.0) || turns.iter().all(|v| *v <= 0.0),
            "projected plane is not convex"
        );
        let (x, y, w, h) = clip;
        let bounds = [
            i64::from(x).max(0),
            i64::from(y).max(0),
            (i64::from(x) + i64::from(w)).min(i64::from(self.width)),
            (i64::from(y) + i64::from(h)).min(i64::from(self.height)),
        ];
        if opacity == 0 || bounds[0] >= bounds[2] || bounds[1] >= bounds[3] {
            return Ok(());
        }
        if quad.iter().all(|v| v[2] == quad[0][2])
            && positions[0][0] == positions[3][0]
            && positions[1][0] == positions[2][0]
            && positions[0][1] == positions[1][1]
            && positions[2][1] == positions[3][1]
            && positions[1][0] > positions[0][0]
            && positions[3][1] > positions[0][1]
        {
            self.projective_rectangle(texture, [positions[0], positions[2]], bounds, opacity);
            return Ok(());
        }
        let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let vertices: [Vertex; 4] = std::array::from_fn(|i| {
            let inverse_z = 1.0 / quad[i][2];
            Vertex {
                xy: positions[i],
                inverse_z,
                uv_over_z: uv[i].map(|v| v * inverse_z),
            }
        });
        self.projective_triangle(
            texture,
            [vertices[0], vertices[1], vertices[2]],
            bounds,
            opacity,
        );
        self.projective_triangle(
            texture,
            [vertices[0], vertices[2], vertices[3]],
            bounds,
            opacity,
        );
        Ok(())
    }

    // The default native view mode produces axis-aligned quads. Preserve
    // fractional edges and pixel-center sampling, but avoid two triangle
    // coverage/interpolation evaluations for every background pixel.
    fn projective_rectangle(
        &mut self,
        texture: &Texture,
        rect: [[f64; 2]; 2],
        bounds: [i64; 4],
        opacity: u8,
    ) {
        let [[left, top], [right, bottom]] = rect;
        let x0 = bounds[0].max((left - 0.5).ceil() as i64);
        let y0 = bounds[1].max((top - 0.5).ceil() as i64);
        let x1 = bounds[2].min((right - 0.5).ceil() as i64);
        let y1 = bounds[3].min((bottom - 0.5).ceil() as i64);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let columns: Vec<_> = (x0..x1)
            .map(|x| texel_coordinate((x as f64 + 0.5 - left) / (right - left), texture.width))
            .collect();
        for y in y0..y1 {
            let sy = texel_coordinate((y as f64 + 0.5 - top) / (bottom - top), texture.height);
            for (column, sx) in columns.iter().enumerate() {
                let si = (sy * texture.width as usize + sx) * 4;
                let di = (y as usize * self.width as usize + x0 as usize + column) * 4;
                let mut source: [u8; 4] = texture.rgba[si..si + 4].try_into().unwrap();
                source[3] = ((u32::from(source[3]) * u32::from(opacity) + 127) / 255) as u8;
                source_over(&mut self.rgba[di..di + 4], source);
            }
        }
    }

    fn projective_triangle(
        &mut self,
        texture: &Texture,
        mut vertices: [Vertex; 3],
        bounds: [i64; 4],
        opacity: u8,
    ) {
        let area = Edge::new(vertices[0].xy, vertices[1].xy).at(vertices[2].xy);
        if area == 0.0 {
            return;
        }
        if area < 0.0 {
            vertices.swap(1, 2);
        }
        let [v0, v1, v2] = vertices;
        let edges = [
            Edge::new(v1.xy, v2.xy),
            Edge::new(v2.xy, v0.xy),
            Edge::new(v0.xy, v1.xy),
        ];
        let left = bounds[0].max(
            vertices
                .iter()
                .map(|v| v.xy[0].floor() as i64)
                .min()
                .unwrap(),
        );
        let top = bounds[1].max(
            vertices
                .iter()
                .map(|v| v.xy[1].floor() as i64)
                .min()
                .unwrap(),
        );
        let right = bounds[2].min(
            vertices
                .iter()
                .map(|v| v.xy[0].ceil() as i64)
                .max()
                .unwrap(),
        );
        let bottom = bounds[3].min(
            vertices
                .iter()
                .map(|v| v.xy[1].ceil() as i64)
                .max()
                .unwrap(),
        );
        for y in top..bottom {
            for x in left..right {
                let weights = edges
                    .each_ref()
                    .map(|edge| edge.at([x as f64 + 0.5, y as f64 + 0.5]));
                if !edges.iter().zip(weights).all(|(edge, w)| edge.covers(w)) {
                    continue;
                }
                // The common triangle-area divisor cancels in this ratio.
                let inverse_z: f64 = vertices
                    .iter()
                    .zip(weights)
                    .map(|(v, w)| v.inverse_z * w)
                    .sum();
                let uv: [f64; 2] = std::array::from_fn(|c| {
                    vertices
                        .iter()
                        .zip(weights)
                        .map(|(v, w)| v.uv_over_z[c] * w)
                        .sum::<f64>()
                        / inverse_z
                });
                let sx = texel_coordinate(uv[0], texture.width);
                let sy = texel_coordinate(uv[1], texture.height);
                let si = (sy * texture.width as usize + sx) * 4;
                let di = (y as usize * self.width as usize + x as usize) * 4;
                let mut source: [u8; 4] = texture.rgba[si..si + 4].try_into().unwrap();
                source[3] = ((u32::from(source[3]) * u32::from(opacity) + 127) / 255) as u8;
                source_over(&mut self.rgba[di..di + 4], source);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(width: u32, height: u32, alpha: u8) -> Texture {
        Texture {
            width,
            height,
            display_scale: 1.0,
            rgba: (0..height)
                .flat_map(|y| (0..width).flat_map(move |x| [x as u8 * 30, y as u8 * 30, 47, alpha]))
                .collect(),
        }
    }

    #[test]
    fn stabilizes_texel_boundary_ties_without_shifting_samples() {
        assert_eq!(texel_coordinate(0.25 - f64::EPSILON, 4), 1);
        assert_eq!(texel_coordinate(0.25 - 1e-10, 4), 0);
        assert_eq!(texel_coordinate(0.25 + 1e-10, 4), 1);
        assert_eq!(texel_coordinate(1.0, 4), 3);
    }

    #[test]
    fn identity_matches_blit_without_a_diagonal_alpha_seam() -> Result<()> {
        let texture = grid(9, 7, 128);
        let quad = [
            [0.0, 0.0, 720.0],
            [9.0, 0.0, 720.0],
            [9.0, 7.0, 720.0],
            [0.0, 7.0, 720.0],
        ];
        for opaque in [false, true] {
            let new = || {
                if opaque {
                    Framebuffer::new(9, 7)
                } else {
                    Framebuffer::transparent(9, 7)
                }
            };
            let mut expected = new()?;
            let mut actual = new()?;
            expected.blit(&texture, (0, 0, 9, 7), 128)?;
            actual.blit_projective(&texture, quad, (0, 0, 9, 7), 128)?;
            assert_eq!(actual.rgba, expected.rgba);
        }
        Ok(())
    }

    #[test]
    fn fractional_rectangle_fast_path_matches_general_triangles() -> Result<()> {
        let texture = grid(9, 7, 128);
        for [left, top, right, bottom] in [
            [0.0, 0.0, 9.0, 7.0],
            [-1.25, 0.5, 12.75, 7.5],
            [0.5, 1.5, 7.5, 5.5],
            [-20.0, -3.75, 17.0, 23.0],
        ] {
            let quad = [
                [left, top, 720.0],
                [right, top, 720.0],
                [right, bottom, 720.0],
                [left, bottom, 720.0],
            ];
            let mut fast = Framebuffer::transparent(9, 7)?;
            let mut general = Framebuffer::transparent(9, 7)?;
            fast.blit_projective(&texture, quad, (0, 0, 9, 7), 128)?;
            let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
            let v: [Vertex; 4] = std::array::from_fn(|i| Vertex {
                xy: [quad[i][0], quad[i][1]],
                inverse_z: 1.0 / 720.0,
                uv_over_z: uv[i].map(|v| v / 720.0),
            });
            general.projective_triangle(&texture, [v[0], v[1], v[2]], [0, 0, 9, 7], 128);
            general.projective_triangle(&texture, [v[0], v[2], v[3]], [0, 0, 9, 7], 128);
            assert_eq!(fast.rgba, general.rgba, "rectangle {quad:?}");
        }
        Ok(())
    }

    #[test]
    fn tilted_plane_matches_independent_ray_intersections_not_affine_uvs() -> Result<()> {
        let texture = grid(4, 4, 128);
        let mut frame = Framebuffer::transparent(8, 8)?;
        // World plane (X,Y,Z) = (u,v,1+u), u/v in 0..4, focal length 8.
        // It projects partly offscreen, exercising bounding-box clipping too.
        frame.blit_projective(
            &texture,
            [
                [0.0, 0.0, 1.0],
                [6.4, 0.0, 5.0],
                [6.4, 6.4, 5.0],
                [0.0, 32.0, 1.0],
            ],
            (0, 0, 8, 8),
            255,
        )?;
        for y in 0..8 {
            for x in 0..8 {
                // Solve a camera ray against Z=1+X, independently of the
                // rasterizer's triangulation and barycentric interpolation.
                let sx = f64::from(x) + 0.5;
                let sy = f64::from(y) + 0.5;
                let world_x = sx / (8.0 - sx);
                let world_y = sy * (1.0 + world_x) / 8.0;
                let expected = if (0.0..4.0).contains(&world_x) && (0.0..4.0).contains(&world_y) {
                    [
                        world_x.floor() as u8 * 30,
                        world_y.floor() as u8 * 30,
                        47,
                        128,
                    ]
                } else {
                    [0; 4]
                };
                let at = (y * 8 + x) as usize * 4;
                assert_eq!(&frame.rgba[at..at + 4], &expected, "pixel {x},{y}");
            }
        }
        Ok(())
    }

    #[test]
    fn either_winding_clips_to_the_object_viewport() -> Result<()> {
        let texture = grid(1, 1, 128);
        let clockwise = [
            [-100.0, -100.0, 1.0],
            [100.0, -100.0, 1.0],
            [100.0, 100.0, 1.0],
            [-100.0, 100.0, 1.0],
        ];
        for quad in [
            clockwise,
            [clockwise[0], clockwise[3], clockwise[2], clockwise[1]],
        ] {
            let mut frame = Framebuffer::transparent(8, 8)?;
            frame.blit_projective(&texture, quad, (1, 2, 3, 2), 255)?;
            for y in 0..8 {
                for x in 0..8 {
                    let expected = if (1..4).contains(&x) && (2..4).contains(&y) {
                        [0, 0, 47, 128]
                    } else {
                        [0; 4]
                    };
                    let at = (y * 8 + x) * 4;
                    assert_eq!(&frame.rgba[at..at + 4], &expected);
                }
            }
            let previous = frame.rgba.clone();
            frame.blit_projective(
                &texture,
                quad,
                (i32::MAX, i32::MAX, u32::MAX, u32::MAX),
                255,
            )?;
            assert_eq!(frame.rgba, previous);
        }
        Ok(())
    }

    #[test]
    fn whole_planes_at_or_behind_the_near_plane_are_culled_without_erasing_other_layers()
    -> Result<()> {
        let texture = grid(2, 2, 255);
        let mut frame = Framebuffer::new(8, 8)?;
        frame.clear([10, 20, 30]);
        let before = frame.rgba.clone();
        let quad = [
            [0.0, 0.0, 1.0],
            [4.0, 0.0, 1.0],
            [4.0, 4.0, 1.0],
            [0.0, 4.0, 1.0],
        ];
        for depth in [i32::MIN as f64, -100.0, 0.0, 1e-12] {
            frame.blit_projective(
                &texture,
                quad.map(|p| [p[0], p[1], depth]),
                (0, 0, 8, 8),
                255,
            )?;
            assert_eq!(frame.rgba, before);
        }
        let mut malformed = quad.map(|p| [p[0], p[1], -100.0]);
        malformed[0][0] = f64::NAN;
        assert!(
            frame
                .blit_projective(&texture, malformed, (0, 0, 8, 8), 255)
                .is_err()
        );
        frame.blit_projective(&texture, quad, (0, 0, 8, 8), 255)?;
        assert_ne!(frame.rgba, before);
        Ok(())
    }
    #[test]
    fn validates_geometry_before_drawing_and_handles_edge_on_planes() -> Result<()> {
        let texture = grid(1, 1, 255);
        let mut frame = Framebuffer::transparent(8, 8)?;
        let quad = [
            [0.0, 0.0, 1.0],
            [4.0, 0.0, 1.0],
            [4.0, 4.0, 1.0],
            [0.0, 4.0, 1.0],
        ];
        for invalid in [f64::NAN, f64::INFINITY, -1.0, 0.0, 1e-12] {
            let mut bad = quad;
            bad[0][2] = invalid;
            assert!(
                frame
                    .blit_projective(&texture, bad, (0, 0, 8, 8), 255)
                    .is_err()
            );
        }
        let mut bad = quad;
        bad[0][0] = f64::NAN;
        assert!(
            frame
                .blit_projective(&texture, bad, (0, 0, 8, 8), 255)
                .is_err()
        );
        let bow_tie = [quad[0], quad[2], quad[1], quad[3]];
        assert!(
            frame
                .blit_projective(&texture, bow_tie, (0, 0, 8, 8), 255)
                .is_err()
        );
        let mut concave = quad;
        concave[2] = [1.0, 1.0, 1.0];
        assert!(
            frame
                .blit_projective(&texture, concave, (0, 0, 8, 8), 255)
                .is_err()
        );
        frame.blit_projective(
            &texture,
            [
                [2.0, 0.0, 1.0],
                [2.0, 1.0, 1.0],
                [2.0, 4.0, 1.0],
                [2.0, 3.0, 1.0],
            ],
            (0, 0, 8, 8),
            255,
        )?;
        assert!(frame.rgba.iter().all(|b| *b == 0));
        let invalid = Texture {
            rgba: vec![],
            ..texture
        };
        assert!(
            frame
                .blit_projective(&invalid, quad, (0, 0, 8, 8), 255)
                .is_err()
        );
        Ok(())
    }
}
