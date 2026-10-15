//! Region blits and bounded offscreen image areas.
use super::{Game, Layer, Transition};
use crate::render::{Framebuffer, Texture};
use anyhow::{Context, Result, bail, ensure};
use rugp_assets::{fade::Fade, object::ObjectRef};
use rugp_scene_vm::{Argument, Instruction, vm::Value};
use std::sync::Arc;

/// Rectangles are x/y/width/height; an empty extent selects the full surface.
/// Crop/scale into a destination snapshot, preserving pixels outside the region.
fn place_region(
    source: &Texture,
    src: [i16; 4],
    destination: &Texture,
    dst: [i16; 4],
) -> Result<Texture> {
    let rect = |raw: [i16; 4], width: u32, height: u32| -> Result<[i64; 4]> {
        ensure!(raw[2] >= 0 && raw[3] >= 0, "negative blit extent");
        Ok([
            i64::from(raw[0]),
            i64::from(raw[1]),
            if raw[2] == 0 {
                i64::from(width)
            } else {
                i64::from(raw[2])
            },
            if raw[3] == 0 {
                i64::from(height)
            } else {
                i64::from(raw[3])
            },
        ])
    };
    let [sx, sy, sw, sh] = rect(src, source.width, source.height)?;
    let [dx, dy, dw, dh] = rect(dst, destination.width, destination.height)?;
    let mut rgba = destination.rgba.clone();
    for y in dy.max(0)..(dy + dh).min(i64::from(destination.height)) {
        let source_y = sy + (y - dy) * sh / dh;
        if !(0..i64::from(source.height)).contains(&source_y) {
            continue;
        }
        for x in dx.max(0)..(dx + dw).min(i64::from(destination.width)) {
            let source_x = sx + (x - dx) * sw / dw;
            if !(0..i64::from(source.width)).contains(&source_x) {
                continue;
            }
            let input = (source_y as usize * source.width as usize + source_x as usize) * 4;
            let output = (y as usize * destination.width as usize + x as usize) * 4;
            rgba[output..output + 4].copy_from_slice(&source.rgba[input..input + 4]);
        }
    }
    Ok(Texture {
        width: destination.width,
        height: destination.height,
        display_scale: 1.0,
        rgba,
    })
}

impl Game {
    pub(super) fn build_parse_image(
        &mut self,
        id: u64,
        fields: &std::collections::BTreeMap<String, Value>,
    ) -> Result<()> {
        let Some(Value::Reference(destination)) = fields.get("対象ウインドウ") else {
            bail!("parse image destination missing")
        };
        let mut frame = Framebuffer::new(self.frame.width, self.frame.height)?;
        self.objects
            .get(&id)
            .context("parse image source missing")?
            .render(&mut frame)?;
        for (&other_id, object) in &self.objects {
            if other_id != id && object.visible && object.has_character_image() {
                object.render(&mut frame)?;
            }
        }
        self.write_surface(
            destination,
            Texture {
                width: frame.width,
                height: frame.height,
                display_scale: 1.0,
                rgba: frame.rgba,
            },
        )
    }
    fn surface(&mut self, reference: &ObjectRef) -> Result<Arc<Texture>> {
        let id = reference.offset.context("surface has no bound object")?;
        if id == self.primary_buffer {
            return Ok(Arc::new(Texture {
                width: self.frame.width,
                height: self.frame.height,
                display_scale: 1.0,
                rgba: self.frame.rgba.clone(),
            }));
        }
        if let Some(buffer) = self.buffers.get(&id) {
            return Ok(Arc::clone(buffer));
        }
        if reference
            .class
            .as_ref()
            .is_some_and(|c| ["CImgAreaBox", "CParseBgBox"].contains(&c.name.as_str()))
        {
            let size = self
                .objects
                .get(&id)
                .map_or((self.frame.width, self.frame.height), |o| {
                    if o.rect.2 > 0 && o.rect.3 > 0 {
                        (o.rect.2, o.rect.3)
                    } else {
                        (self.frame.width, self.frame.height)
                    }
                });
            let mut frame = Framebuffer::new(size.0, size.1)?;
            if let Some(object) = self.objects.get(&id) {
                object.render(&mut frame)?;
            }
            return Ok(Arc::new(Texture {
                width: frame.width,
                height: frame.height,
                display_scale: 1.0,
                rgba: frame.rgba,
            }));
        }
        self.images.load(reference)
    }
    fn write_surface(&mut self, reference: &ObjectRef, texture: Texture) -> Result<()> {
        let id = reference
            .offset
            .context("destination surface has no bound object")?;
        if id == self.primary_buffer {
            self.frame.rgba = texture.rgba;
            self.transition = None;
        } else {
            ensure!(
                reference
                    .class
                    .as_ref()
                    .is_some_and(|c| ["CImgAreaBox", "CParseBgBox"].contains(&c.name.as_str())),
                "unsupported destination surface"
            );
            let old_size = self.buffers.get(&id).map_or(0, |b| b.rgba.len());
            let used: usize = self.buffers.values().map(|b| b.rgba.len()).sum();
            ensure!(
                used - old_size + texture.rgba.len() <= 128 * 1024 * 1024,
                "offscreen surface budget exceeded"
            );
            let texture = Arc::new(texture);
            self.buffers.insert(id, Arc::clone(&texture));
            self.objects
                .entry(id)
                .or_default()
                .layers
                .insert(0, Layer::Screen(texture));
        }
        Ok(())
    }
    /// Returns whether the VM must wait for this effect before acknowledging it.
    pub(super) fn blit(&mut self, instruction: &Instruction) -> Result<bool> {
        ensure!(
            self.parse_transition.is_none(),
            "framebuffer effects during a ParseScreen transition are not implemented yet"
        );
        let [
            Argument::Message { class, arguments },
            control,
            blocking,
            flags,
            duration,
        ] = instruction.arguments.as_slice()
        else {
            bail!("blit has no inline effect");
        };
        ensure!(class.schema == 12, "unsupported fade schema");
        let [Argument::Bytes(data), source, background, destination] = arguments.as_slice() else {
            bail!("invalid fade reference fields");
        };
        let mut fade = Fade::from_inline(&class.name, data)?;
        ensure!(
            matches!(fade.mode, 0 | 0x11),
            "unsupported fade region mode {:#x}",
            fade.mode
        );
        if fade.mode == 0 {
            // Default surface bounds. Specialized stretch records also retain
            // initial strip geometry, which is not their final image crop.
            fade.rectangles = [[0; 4]; 3];
        }
        let control = self.vm.evaluate(control)?.integer()?;
        let flags = self.vm.evaluate(flags)?.integer()?;
        if matches!(
            class.name.as_str(),
            "CFadeNormal" | "CFadeMergeBlack" | "CFadeMergeWhite" | "CFadeMergeColor"
        ) {
            ensure!(control == 0 && flags == 0, "unsupported blit control flags");
        } // Spatial fallbacks omit their effect-specific control words.
        let blocking = self.vm.evaluate(blocking)?.integer()?;
        ensure!(blocking == 0 || blocking == 1, "invalid blit wait mode");
        let duration = self.vm.evaluate(duration)?.integer()?;
        ensure!(
            duration >= 0,
            "unbounded blit duration is not implemented yet"
        );
        let duration = duration as u32;
        let resolve = |arg: &Argument| -> Result<ObjectRef> {
            match self.vm.evaluate(arg)? {
                Value::Reference(reference) => Ok(reference),
                _ => bail!("fade surface is not an object"),
            }
        };
        let source = resolve(source)?;
        let background = resolve(background)?;
        let destination = resolve(destination)?;
        let source = self.surface(&source)?;
        let previous = self.surface(&destination)?;
        let background = if let Some(color) = fade.color {
            let mut frame = Framebuffer::new(self.frame.width, self.frame.height)?;
            frame.clear(color);
            Arc::new(Texture {
                width: frame.width,
                height: frame.height,
                display_scale: 1.0,
                rgba: frame.rgba,
            })
        } else {
            self.surface(&background)?
        };
        let source = place_region(&source, fade.rectangles[0], &previous, fade.rectangles[2])?;
        let background = place_region(
            &background,
            fade.rectangles[1],
            &previous,
            fade.rectangles[2],
        )?;
        if duration == 0 {
            let alpha = u32::from(fade.curve.alpha(0, 0));
            let rgba = source
                .rgba
                .iter()
                .zip(&background.rgba)
                .map(|(s, b)| {
                    ((u32::from(*s) * alpha + u32::from(*b) * (256 - alpha) + 128) / 256) as u8
                })
                .collect();
            self.write_surface(
                &destination,
                Texture {
                    width: source.width,
                    height: source.height,
                    display_scale: 1.0,
                    rgba,
                },
            )?;
        } else {
            ensure!(
                destination.offset == Some(self.primary_buffer),
                "timed offscreen blit is not implemented yet"
            );
            self.transition = Some(Transition {
                from: background.rgba.clone(),
                to: source.rgba.clone(),
                elapsed: 0,
                duration,
                curve: Some(fade.curve),
            });
            self.update_frame();
            if blocking != 0 {
                self.wait_ms = Some(duration);
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crop_scale_and_clipped_destination_preserve_other_pixels() -> Result<()> {
        let source = Texture {
            width: 4,
            height: 1,
            display_scale: 1.0,
            rgba: vec![1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255],
        };
        let destination = Texture {
            width: 4,
            height: 2,
            display_scale: 1.0,
            rgba: [9, 0, 0, 255].repeat(8),
        };
        let output = place_region(&source, [1, 0, 2, 1], &destination, [1, 1, 2, 1])?;
        assert_eq!(
            output
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[0])
                .collect::<Vec<_>>(),
            [9, 9, 9, 9, 9, 2, 3, 9]
        );
        let clipped = place_region(&source, [0; 4], &destination, [-2, 0, 4, 1])?;
        assert_eq!(&clipped.rgba[..8], &[3, 0, 0, 255, 4, 0, 0, 255]);
        let full = place_region(&source, [0; 4], &destination, [0; 4])?;
        assert_eq!(&full.rgba[..16], &source.rgba);
        assert_eq!(&full.rgba[16..], &source.rgba);
        assert!(place_region(&source, [0, 0, -1, 1], &destination, [0; 4]).is_err());
        Ok(())
    }
}
