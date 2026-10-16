//! CPU rasterizer for tessellated egui meshes into a 0RGB softbuffer frame.
//! egui feathers its own edges, so plain coverage-tested triangles suffice.
use egui::{
    Color32, TextureFilter, TextureId, TexturesDelta,
    epaint::{ClippedPrimitive, ImageData, ImageDelta, Primitive, Vertex},
};
use std::collections::HashMap;

struct Texture {
    width: usize,
    height: usize,
    /// Premultiplied, gamma-space, as egui provides them.
    pixels: Vec<Color32>,
    linear: bool,
}
impl Texture {
    fn texel(&self, x: isize, y: isize) -> [f32; 4] {
        let x = x.clamp(0, self.width as isize - 1) as usize;
        let y = y.clamp(0, self.height as isize - 1) as usize;
        self.pixels[y * self.width + x].to_array().map(f32::from)
    }
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let x = u * self.width as f32 - 0.5;
        let y = v * self.height as f32 - 0.5;
        if !self.linear {
            return self.texel(x.round() as isize, y.round() as isize);
        }
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let [a, b, c, d] = [
            self.texel(x0, y0),
            self.texel(x0 + 1, y0),
            self.texel(x0, y0 + 1),
            self.texel(x0 + 1, y0 + 1),
        ];
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            top + (bottom - top) * fy
        })
    }
}

#[derive(Default)]
pub(super) struct Raster {
    textures: HashMap<TextureId, Texture>,
}
impl Raster {
    /// Apply before painting.
    pub fn set_textures(&mut self, delta: &TexturesDelta) {
        for (&id, deltas) in &delta.set {
            for image in deltas {
                self.update(id, image);
            }
        }
    }
    /// Apply after painting.
    pub fn free_textures(&mut self, delta: &TexturesDelta) {
        for id in &delta.free {
            self.textures.remove(id);
        }
    }
    fn update(&mut self, id: TextureId, delta: &ImageDelta) {
        let ImageData::Color(image) = &delta.image;
        let [width, height] = image.size;
        let linear = delta.options.magnification == TextureFilter::Linear;
        let Some([left, top]) = delta.pos else {
            self.textures.insert(
                id,
                Texture {
                    width,
                    height,
                    pixels: image.pixels.clone(),
                    linear,
                },
            );
            return;
        };
        let Some(texture) = self.textures.get_mut(&id) else {
            return;
        };
        if left + width > texture.width || top + height > texture.height {
            return;
        }
        for row in 0..height {
            let start = (top + row) * texture.width + left;
            texture.pixels[start..start + width]
                .copy_from_slice(&image.pixels[row * width..(row + 1) * width]);
        }
    }
    pub fn paint(
        &self,
        target: &mut [u32],
        size: (usize, usize),
        pixels_per_point: f32,
        primitives: &[ClippedPrimitive],
    ) {
        for primitive in primitives {
            let Primitive::Mesh(mesh) = &primitive.primitive else {
                continue;
            };
            let Some(texture) = self.textures.get(&mesh.texture_id) else {
                continue;
            };
            let clip = primitive.clip_rect;
            let clip = [
                (clip.min.x * pixels_per_point).round().max(0.0) as usize,
                (clip.min.y * pixels_per_point).round().max(0.0) as usize,
                ((clip.max.x * pixels_per_point).round().max(0.0) as usize).min(size.0),
                ((clip.max.y * pixels_per_point).round().max(0.0) as usize).min(size.1),
            ];
            if clip[0] >= clip[2] || clip[1] >= clip[3] {
                continue;
            }
            for &[a, b, c] in mesh.indices.as_chunks::<3>().0 {
                let vertex = |i: u32| mesh.vertices.get(i as usize);
                let (Some(a), Some(b), Some(c)) = (vertex(a), vertex(b), vertex(c)) else {
                    continue;
                };
                triangle(target, size.0, clip, texture, [a, b, c], pixels_per_point);
            }
        }
    }
}

/// Evaluated in one canonical direction, so triangles sharing an edge get
/// exactly opposite values and the tie rule below assigns each pixel once.
fn edge(a: (f32, f32), b: (f32, f32), p: (f32, f32)) -> f32 {
    if a.1 > b.1 || (a.1 == b.1 && a.0 > b.0) {
        return -edge(b, a, p);
    }
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

/// Pixel centres exactly on an edge belong to one side only.
fn covers(a: (f32, f32), b: (f32, f32), sign: f32, value: f32) -> bool {
    let (dx, dy) = ((b.0 - a.0) * sign, (b.1 - a.1) * sign);
    value > 0.0 || (value == 0.0 && (dy > 0.0 || (dy == 0.0 && dx < 0.0)))
}

fn triangle(
    target: &mut [u32],
    stride: usize,
    clip: [usize; 4],
    texture: &Texture,
    vertices: [&Vertex; 3],
    pixels_per_point: f32,
) {
    let p = vertices.map(|v| (v.pos.x * pixels_per_point, v.pos.y * pixels_per_point));
    let area = edge(p[0], p[1], p[2]);
    if !area.is_normal() {
        return;
    }
    let bound = |axis: fn(&(f32, f32)) -> f32, low: usize, high: usize| {
        let values = p.map(|v| axis(&v));
        let min = values.iter().copied().fold(f32::INFINITY, f32::min);
        let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        (
            (min.floor().max(0.0) as usize).max(low),
            (max.ceil().max(0.0) as usize).min(high),
        )
    };
    let (x0, x1) = bound(|v| v.0, clip[0], clip[2]);
    let (y0, y1) = bound(|v| v.1, clip[1], clip[3]);
    let colors = vertices.map(|v| v.color.to_array().map(f32::from));
    for y in y0..y1 {
        let py = y as f32 + 0.5;
        for x in x0..x1 {
            let point = (x as f32 + 0.5, py);
            let sign = area.signum();
            let e = [
                edge(p[1], p[2], point),
                edge(p[2], p[0], point),
                edge(p[0], p[1], point),
            ];
            if !(covers(p[1], p[2], sign, e[0] * sign)
                && covers(p[2], p[0], sign, e[1] * sign)
                && covers(p[0], p[1], sign, e[2] * sign))
            {
                continue;
            }
            let [w0, w1, w2] = e.map(|e| e / area);
            let u = w0 * vertices[0].uv.x + w1 * vertices[1].uv.x + w2 * vertices[2].uv.x;
            let v = w0 * vertices[0].uv.y + w1 * vertices[1].uv.y + w2 * vertices[2].uv.y;
            let texel = texture.sample(u, v);
            let source: [f32; 4] = std::array::from_fn(|i| {
                (w0 * colors[0][i] + w1 * colors[1][i] + w2 * colors[2][i]) * texel[i] / 255.0
            });
            if source[3] <= 0.0 && source[..3].iter().all(|&c| c <= 0.0) {
                continue;
            }
            let pixel = &mut target[y * stride + x];
            let keep = 1.0 - source[3] / 255.0;
            let blend = |shift: u32, channel: f32| {
                let dst = ((*pixel >> shift) & 0xff) as f32;
                ((channel + dst * keep).round().clamp(0.0, 255.0) as u32) << shift
            };
            *pixel = blend(16, source[0]) | blend(8, source[1]) | blend(0, source[2]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Rect, epaint::Mesh};
    #[test]
    fn opaque_and_translucent_quads_respect_clip_and_blend_premultiplied() {
        let mut raster = Raster::default();
        let mut delta = TexturesDelta::default();
        delta.set.insert(
            TextureId::default(),
            [ImageDelta::full(
                ImageData::Color(egui::ColorImage::new([1, 1], vec![Color32::WHITE]).into()),
                egui::TextureOptions::NEAREST,
            )]
            .into(),
        );
        raster.set_textures(&delta);
        delta.clear();
        let quad = |rect: Rect, color: Color32| {
            let mut mesh = Mesh::default();
            mesh.add_colored_rect(rect, color);
            ClippedPrimitive {
                clip_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(3.0, 4.0)),
                primitive: Primitive::Mesh(mesh),
            }
        };
        let mut target = vec![0; 16];
        raster.paint(
            &mut target,
            (4, 4),
            1.0,
            &[
                quad(
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(4.0, 4.0)),
                    Color32::from_rgb(200, 0, 0),
                ),
                quad(
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(4.0, 2.0)),
                    Color32::from_black_alpha(128),
                ),
            ],
        );
        assert_eq!(target[3], 0, "clipped column stays untouched");
        assert_eq!(target[4 * 3], 200 << 16);
        assert_eq!(target[0], 100 << 16);
    }
}
