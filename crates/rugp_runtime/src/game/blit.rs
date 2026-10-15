//! Full-screen native blits and bounded offscreen image areas.
use super::{Game, Layer, Transition};
use crate::render::{Framebuffer, Texture};
use anyhow::{Context, Result, bail, ensure};
use rugp_assets::{fade::Fade, object::ObjectRef};
use rugp_scene_vm::{Argument, Instruction, vm::Value};
use std::sync::Arc;

impl Game {
    fn surface(&mut self, reference: &ObjectRef) -> Result<Arc<Texture>> {
        let id = reference.offset.context("surface has no bound object")?;
        if id == self.primary_buffer {
            return Ok(Arc::new(Texture {
                width: self.frame.width,
                height: self.frame.height,
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
            let mut frame = Framebuffer::new(self.frame.width, self.frame.height)?;
            if let Some(object) = self.objects.get(&id) {
                for layer in object.layers.values() {
                    frame.blit(&layer.texture, object.rect, 255)?;
                }
            }
            return Ok(Arc::new(Texture {
                width: frame.width,
                height: frame.height,
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
            if reference
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CParseBgBox")
            {
                self.objects
                    .entry(id)
                    .or_default()
                    .layers
                    .insert(0, Layer { texture });
            }
        }
        Ok(())
    }
    /// Returns whether the VM must wait for this effect before acknowledging it.
    pub(super) fn blit(&mut self, instruction: &Instruction) -> Result<bool> {
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
        let fade = Fade::from_inline(&class.name, data)?;
        ensure!(
            fade.mode == 0x11,
            "unsupported fade region mode {:#x}",
            fade.mode
        );
        ensure!(
            fade.rectangles
                == [
                    [0, 0, self.frame.width as i16, self.frame.height as i16],
                    [0; 4],
                    [0; 4]
                ],
            "partial/translated blit regions are not implemented yet"
        );
        ensure!(
            self.vm.evaluate(control)?.integer()? == 0 && self.vm.evaluate(flags)?.integer()? == 0,
            "unsupported blit control flags"
        );
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
        let background = if let Some(color) = fade.color {
            let mut frame = Framebuffer::new(self.frame.width, self.frame.height)?;
            frame.clear(color);
            Arc::new(Texture {
                width: frame.width,
                height: frame.height,
                rgba: frame.rgba,
            })
        } else {
            self.surface(&background)?
        };
        ensure!(
            source.width == self.frame.width
                && source.height == self.frame.height
                && background.width == self.frame.width
                && background.height == self.frame.height,
            "blit surface dimensions do not match"
        );
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
