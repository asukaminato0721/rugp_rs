//! Identity-preserving texture tables for game snapshots, including dynamically
//! composed/offscreen surfaces. Equal pixels do not imply shared storage.
//! Outer file readers must cap bytes before deserializing this bounded table.
use super::{Framebuffer, Texture};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
};

const MAX_TEXTURES: usize = 65_536;
const MAX_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextureSnapshotTable {
    version: u32,
    textures: Vec<SavedTexture>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedTexture {
    width: u32,
    height: u32,
    display_scale: f32,
    rgba: Vec<u8>,
}
struct Identity(Arc<Texture>);
impl PartialEq for Identity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Identity {}
impl Hash for Identity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

#[derive(Default)]
pub struct TextureSnapshotWriter {
    // Retaining the Arc is necessary: an address cannot be recycled between
    // inserts and accidentally become an alias of a different allocation.
    identities: HashMap<Identity, u32>,
    textures: Vec<SavedTexture>,
    bytes: usize,
    failed: bool,
}
impl TextureSnapshotWriter {
    pub fn insert(&mut self, texture: &Arc<Texture>) -> Result<u32> {
        ensure!(!self.failed, "texture snapshot writer previously failed");
        let key = Identity(Arc::clone(texture));
        if let Some(id) = self.identities.get(&key) {
            return Ok(*id);
        }
        let result = self.pixels(
            texture.width,
            texture.height,
            texture.display_scale,
            &texture.rgba,
        );
        match result {
            Ok(id) => {
                self.identities.insert(key, id);
                Ok(id)
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }
    pub fn frame(&mut self, frame: &Framebuffer) -> Result<u32> {
        self.rgba_frame(frame.width, frame.height, &frame.rgba)
    }
    pub fn rgba_frame(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<u32> {
        ensure!(!self.failed, "texture snapshot writer previously failed");
        let result = self.pixels(width, height, 1.0, rgba);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn pixels(&mut self, width: u32, height: u32, display_scale: f32, rgba: &[u8]) -> Result<u32> {
        validate(width, height, display_scale, rgba.len())?;
        ensure!(
            self.textures.len() < MAX_TEXTURES,
            "texture snapshot count exceeds limit"
        );
        ensure!(
            rgba.len() <= MAX_BYTES.saturating_sub(self.bytes),
            "texture snapshot pixels exceed limit"
        );
        self.bytes += rgba.len();
        let id = self.textures.len() as u32;
        self.textures.push(SavedTexture {
            width,
            height,
            display_scale,
            rgba: rgba.to_vec(),
        });
        Ok(id)
    }
    pub fn finish(self) -> Result<TextureSnapshotTable> {
        ensure!(!self.failed, "cannot finish failed texture snapshot writer");
        Ok(TextureSnapshotTable {
            version: 1,
            textures: self.textures,
        })
    }
}
pub struct RestoredTextures(Vec<Arc<Texture>>);
impl RestoredTextures {
    pub fn get(&self, index: u32) -> Result<Arc<Texture>> {
        self.0
            .get(index as usize)
            .map(Arc::clone)
            .context("texture snapshot index out of range")
    }
    pub fn frame(&self, index: u32) -> Result<Framebuffer> {
        let texture = self.get(index)?;
        Ok(Framebuffer {
            width: texture.width,
            height: texture.height,
            rgba: texture.rgba.clone(),
        })
    }
}
impl TextureSnapshotTable {
    pub fn restore(&self) -> Result<RestoredTextures> {
        ensure!(self.version == 1, "unsupported texture snapshot version");
        ensure!(
            self.textures.len() <= MAX_TEXTURES,
            "texture snapshot count exceeds limit"
        );
        let mut bytes = 0;
        for texture in &self.textures {
            validate(
                texture.width,
                texture.height,
                texture.display_scale,
                texture.rgba.len(),
            )?;
            ensure!(
                texture.rgba.len() <= MAX_BYTES - bytes,
                "texture snapshot pixels exceed limit"
            );
            bytes += texture.rgba.len();
        }
        Ok(RestoredTextures(
            self.textures
                .iter()
                .map(|texture| {
                    Arc::new(Texture {
                        width: texture.width,
                        height: texture.height,
                        display_scale: texture.display_scale,
                        rgba: texture.rgba.clone(),
                    })
                })
                .collect(),
        ))
    }
}
fn validate(width: u32, height: u32, display_scale: f32, bytes: usize) -> Result<()> {
    ensure!(
        width > 0
            && height > 0
            && u64::from(width) * u64::from(height) <= 16_777_216
            && bytes == width as usize * height as usize * 4
            && display_scale.is_finite(),
        "invalid snapshot texture dimensions/pixels/scale"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn texture() -> Arc<Texture> {
        Arc::new(Texture {
            width: 1,
            height: 1,
            display_scale: 0.5,
            rgba: vec![12, 34, 56, 78],
        })
    }
    #[test]
    fn shared_storage_survives_wire_roundtrip_without_merging_equal_pixels() -> Result<()> {
        let original = texture();
        let other = texture();
        let mut writer = TextureSnapshotWriter::default();
        let first = writer.insert(&original)?;
        assert_eq!(writer.insert(&original)?, first);
        let second = writer.insert(&other)?;
        assert_ne!(first, second);
        let table = writer.finish()?;
        let table: TextureSnapshotTable = serde_json::from_slice(&serde_json::to_vec(&table)?)?;
        let restored = table.restore()?;
        assert!(Arc::ptr_eq(&restored.get(first)?, &restored.get(first)?));
        assert!(!Arc::ptr_eq(&restored.get(first)?, &restored.get(second)?));
        assert!(!Arc::ptr_eq(&restored.get(first)?, &original));
        assert_eq!(restored.get(first)?.rgba, original.rgba);
        assert_eq!(restored.get(first)?.display_scale, 0.5);
        assert_eq!(restored.frame(first)?.rgba, original.rgba);
        assert!(restored.get(999).is_err());
        Ok(())
    }
    #[test]
    fn malformed_pixels_scale_and_versions_fail_and_poison_writers() -> Result<()> {
        let mut writer = TextureSnapshotWriter::default();
        writer.insert(&texture())?;
        let mut table = writer.finish()?;
        table.textures[0].rgba.pop();
        assert!(table.restore().is_err());
        table.textures[0].rgba.push(78);
        table.version = 9;
        assert!(table.restore().is_err());
        table.version = 1;
        table.textures[0].display_scale = f32::NAN;
        assert!(table.restore().is_err());
        let mut writer = TextureSnapshotWriter::default();
        let bad = Arc::new(Texture {
            width: 0,
            height: 1,
            display_scale: 1.0,
            rgba: vec![],
        });
        assert!(writer.insert(&bad).is_err());
        assert!(writer.insert(&texture()).is_err());
        assert!(writer.finish().is_err());
        let mut writer = TextureSnapshotWriter {
            bytes: MAX_BYTES,
            ..Default::default()
        };
        assert!(writer.insert(&texture()).is_err());
        assert!(validate(u32::MAX, u32::MAX, 1.0, 0).is_err());
        Ok(())
    }
    #[test]
    fn identity_keys_retain_allocations_until_the_table_is_finished() -> Result<()> {
        let original = texture();
        let weak = Arc::downgrade(&original);
        let mut writer = TextureSnapshotWriter::default();
        let first = writer.insert(&original)?;
        drop(original);
        assert!(weak.upgrade().is_some());
        let other = texture();
        assert_ne!(writer.insert(&other)?, first);
        let _table = writer.finish()?;
        assert!(weak.upgrade().is_none());
        Ok(())
    }
}
