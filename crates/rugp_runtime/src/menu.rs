//! Resource-defined image menus and exact indexed hit testing.
use crate::{
    image_manager::ImageManager,
    render::{Framebuffer, Texture},
};
use anyhow::{Context, Result, ensure};
use rugp_assets::{
    RioArchive,
    menu::{ImageMenu, SelectorMask},
    object::ObjectRef,
};

pub struct MenuState {
    pub data: ImageMenu,
    mask: SelectorMask,
    pub hover: Option<u32>,
    pub pressed: Option<u32>,
}
impl MenuState {
    pub fn load(archive: &RioArchive, reference: &ObjectRef) -> Result<Self> {
        let class = reference.class.as_ref().context("menu class missing")?;
        ensure!(class.name == "CRbx", "unsupported image menu class");
        let size = reference.size.context("menu extent missing")?;
        ensure!(size <= 16 * 1024 * 1024, "oversized image menu");
        let data = ImageMenu::parse(
            &archive.read_at(
                reference.offset.context("menu offset missing")?,
                size as usize,
            )?,
            class.schema,
            archive.address_shift,
        )?;
        ensure!(
            data.format == 0x10046 && data.stride == 0 && data.trailing_data == [0; 4],
            "unsupported CRbx backing surface"
        );
        ensure!(
            data.base
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CRip" && c.schema == 1),
            "unsupported image menu hit-map class"
        );
        let size = data.base.size.context("menu hit-map extent missing")?;
        ensure!(size <= 64 * 1024 * 1024, "oversized menu hit map");
        let mask = SelectorMask::parse(&archive.read_at(
            data.base.offset.context("menu hit-map offset missing")?,
            size as usize,
        )?)?;
        ensure!(
            (mask.width, mask.height) == (data.width, data.height),
            "menu/hit-map dimensions differ"
        );
        Ok(Self {
            data,
            mask,
            hover: None,
            pressed: None,
        })
    }
    pub fn mask_bytes(&self) -> usize {
        self.mask.pixels.len()
    }
    pub fn hit(&self, x: i32, y: i32) -> Option<u32> {
        let key = u32::from(self.mask.at(x, y)?);
        self.data
            .selectors
            .iter()
            .find(|s| s.header == key && s.id != 0 && s.status & 5 == 1)
            .map(|s| s.header)
    }
    pub fn result(&self, key: u32) -> Option<u32> {
        self.data
            .selectors
            .iter()
            .find(|s| s.header == key && s.id != 0 && s.status & 5 == 1)
            .map(|s| s.id)
    }
    pub fn set_enabled(&mut self, id: Option<u32>, enabled: bool) {
        for s in &mut self.data.selectors {
            if id.is_none_or(|id| s.id == id) {
                s.status = if enabled {
                    (s.status & !2) | 1
                } else {
                    s.status & !7
                };
            }
        }
        if self.hover.is_some_and(|key| self.result(key).is_none()) {
            self.hover = None;
        }
        if self.pressed.is_some_and(|key| self.result(key).is_none()) {
            self.pressed = None;
        }
    }
    /// rvmm 1004dd90: ignored items are neither selectable nor drawn in
    /// their disabled state. This differs from AllDisableSelecter.
    pub fn ignore_all(&mut self) {
        for selector in &mut self.data.selectors {
            selector.status = (selector.status & !3) | 4;
        }
        self.hover = None;
        self.pressed = None;
    }
    pub fn navigate(&mut self, delta: i32) {
        let keys: Vec<_> = self
            .data
            .selectors
            .iter()
            .filter(|s| s.id != 0 && s.status & 5 == 1)
            .map(|s| s.header)
            .collect();
        if keys.is_empty() {
            self.hover = None;
            return;
        }
        let next = self
            .hover
            .and_then(|key| keys.iter().position(|&k| k == key))
            .map_or_else(
                || if delta < 0 { keys.len() - 1 } else { 0 },
                |i| (i as i32 + delta).rem_euclid(keys.len() as i32) as usize,
            );
        self.hover = Some(keys[next]);
        self.pressed = None;
    }
    pub fn compose(&self, images: &mut ImageManager) -> Result<Texture> {
        let mut frame = Framebuffer::new(u32::from(self.data.width), u32::from(self.data.height))?;
        let draw =
            |frame: &mut Framebuffer, images: &mut ImageManager, r: &ObjectRef| -> Result<()> {
                if r.offset.is_none() && r.inline.is_none() {
                    return Ok(());
                }
                let texture = images.load(r)?;
                ensure!(
                    (texture.width, texture.height) == (frame.width, frame.height),
                    "positioned menu image canvas is not implemented"
                );
                frame.blit(&texture, (0, 0, frame.width, frame.height), 255)
            };
        draw(&mut frame, images, &self.data.overlay)?;
        for s in &self.data.selectors {
            if s.id == 0 {
                continue;
            }
            let enabled = s.status & 1 != 0;
            if enabled && s.flags & 255 != 0 {
                draw(&mut frame, images, &s.images[0])?;
            } else if s.status & 4 == 0 {
                draw(&mut frame, images, &s.images[1])?;
            }
            // Native state images overlay the base image. Selecting a state
            // must not replace its underlying enabled/disabled composition.
            if enabled && self.pressed == Some(s.header) && self.pressed == self.hover {
                ensure!(
                    s.images[3].offset.is_some() || s.images[3].inline.is_some(),
                    "pressed menu-image fallback is not implemented"
                );
                draw(&mut frame, images, &s.images[3])?;
            } else if enabled && self.hover == Some(s.header) {
                draw(&mut frame, images, &s.images[2])?;
            }
        }
        Ok(Texture {
            width: frame.width,
            height: frame.height,
            display_scale: 1.0,
            rgba: frame.rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugp_assets::{menu::ImageSelector, profile::Profile};
    fn null() -> ObjectRef {
        ObjectRef {
            class: None,
            cache_index: Some(0),
            flags: 0,
            offset: None,
            size: None,
            owner: None,
            selector: 0,
            inline: None,
        }
    }
    #[test]
    fn masked_ids_disabled_items_and_keyboard_order() {
        let data = ImageMenu {
            max_id: 4,
            base: null(),
            overlay: null(),
            selectors: (0..4)
                .map(|id| ImageSelector {
                    header: id + 1,
                    id,
                    flags: 0x09400040,
                    images: std::array::from_fn(|_| null()),
                    properties: Profile::default(),
                    status: 0x101,
                    rect: [0; 4],
                })
                .collect(),
            format: 0x10046,
            width: 4,
            height: 1,
            stride: 0,
            trailing_data: vec![0; 4],
        };
        let mut state = MenuState {
            data,
            mask: SelectorMask {
                width: 4,
                height: 1,
                x: 0,
                y: 0,
                pixels: vec![1, 2, 3, 4],
            },
            hover: None,
            pressed: None,
        };
        assert_eq!(state.hit(0, 0), None);
        state.set_enabled(None, false);
        state.set_enabled(Some(1), true);
        state.set_enabled(Some(3), true);
        assert_eq!(state.hit(1, 0), Some(2));
        assert_eq!(state.hit(2, 0), None);
        state.navigate(1);
        assert_eq!(state.hover, Some(2));
        state.navigate(1);
        assert_eq!(state.hover, Some(4));
        state.navigate(1);
        assert_eq!(state.hover, Some(2));
        state.navigate(-1);
        assert_eq!(state.hover, Some(4));
        assert_eq!(state.result(4), Some(3));
        state.set_enabled(Some(3), false);
        assert_eq!(state.hover, None);
        state.hover = Some(2);
        state.pressed = Some(2);
        state.ignore_all();
        assert!(state.data.selectors.iter().all(|s| s.status == 0x104));
        assert_eq!(state.hover, None);
        assert_eq!(state.pressed, None);
        assert_eq!(state.hit(1, 0), None);
        assert_eq!(state.result(2), None);
        state.navigate(1);
        assert_eq!(state.hover, None);
        // EnableSelecter only clears bit 1 and sets bit 0; it does not erase
        // the independent ignore flag (native 1004dcf7).
        state.set_enabled(Some(1), true);
        assert_eq!(state.data.selectors[1].status, 0x105);
        assert_eq!(state.hit(1, 0), None);
        state.set_enabled(None, false);
        state.set_enabled(Some(1), true);
        assert_eq!(state.data.selectors[1].status, 0x101);
        assert_eq!(state.hit(1, 0), Some(2));
    }
}
