//! Native archive references. One writer owns the cache for the whole UUC
//! runtime stream, including references inside length-delimited object records.
use crate::object::{ClassId, ObjectRef};
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;

/// Bind a resolved RIO resource graph to the UUC project identity. Numeric
/// cache entries belong to their reader and cannot be copied into a save.
/// Inline resource bodies require their own serializer and are rejected here.
pub fn from_rio(reference: &ObjectRef) -> Result<ObjectRef> {
    fn convert(reference: &ObjectRef, depth: usize) -> Result<ObjectRef> {
        ensure!(depth < 128, "UUC reference exceeds recursion limit");
        ensure!(
            reference.inline.is_none() && reference.flags & 0x40 == 0,
            "inline RIO resource needs a UUC serializer"
        );
        let mut output = reference.clone();
        if output.class.is_none() {
            output.cache_index = Some(match output.cache_index.unwrap_or(0) {
                0 => 0,
                3 => 2,
                _ => anyhow::bail!("unbound RIO cache entry in UUC reference"),
            });
        } else {
            output.cache_index = None;
            output.flags |= 0x8108;
            output.owner = Some(Box::new(convert(
                output
                    .owner
                    .as_deref()
                    .context("RIO reference owner missing")?,
                depth + 1,
            )?));
        }
        Ok(output)
    }
    convert(reference, 0)
}

pub struct ReferenceWriter {
    pub data: Vec<u8>,
    shift: u32,
    next_index: u32,
    classes: BTreeMap<(String, u16), u32>,
    objects: BTreeMap<Vec<u8>, u32>,
}

impl ReferenceWriter {
    /// UUC reserves cache entries 0, 1 and 2 before writing its root.
    pub fn new(shift: u32) -> Result<Self> {
        ensure!(shift < 32, "invalid UUC address shift");
        Ok(Self {
            data: Vec::new(),
            shift,
            next_index: 3,
            classes: BTreeMap::new(),
            objects: BTreeMap::new(),
        })
    }

    /// Write the ANSI MFC CString used by this game. Reject text that cannot
    /// be encoded in the original code page instead of replacing characters.
    pub fn string(&mut self, value: &str) -> Result<()> {
        let (bytes, _, errors) = encoding_rs::SHIFT_JIS.encode(value);
        ensure!(!errors, "UUC text cannot be encoded as Shift-JIS");
        ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "UUC string exceeds size limit"
        );
        match bytes.len() {
            0..=254 => self.data.push(bytes.len() as u8),
            255..=65533 => {
                self.data.push(255);
                self.data.extend((bytes.len() as u16).to_le_bytes());
            }
            _ => {
                self.data.extend([255, 255, 255]);
                self.data.extend((bytes.len() as u32).to_le_bytes());
            }
        }
        self.data.extend(bytes.as_ref());
        Ok(())
    }

    fn reserve(&mut self) -> Result<u32> {
        ensure!(
            self.next_index < 1_000_000,
            "UUC reference cache exceeds limit"
        );
        let index = self.next_index;
        self.next_index += 1;
        Ok(index)
    }

    fn tag(&mut self, tag: u32) {
        if tag & 0x7fff_ffff < 0x7fff {
            self.data
                .extend(((tag & 0x7fff) as u16 | ((tag >> 16) as u16 & 0x8000)).to_le_bytes());
        } else {
            self.data.extend(0x7fffu16.to_le_bytes());
            self.data.extend(tag.to_le_bytes());
        }
    }

    fn class(&mut self, class: &ClassId) -> Result<()> {
        ensure!(
            !class.name.is_empty() && class.name.len() <= 255,
            "invalid UUC class name"
        );
        let key = (class.name.clone(), class.schema);
        if let Some(index) = self.classes.get(&key) {
            self.tag(*index | 0x8000_0000);
        } else {
            let index = self.reserve()?;
            self.data.extend(0xffffu16.to_le_bytes());
            self.data.extend(class.schema.to_le_bytes());
            self.data.extend((class.name.len() as u16).to_le_bytes());
            self.data.extend(class.name.as_bytes());
            self.classes.insert(key, index);
        }
        Ok(())
    }

    /// Write an existing cache entry. The caller must retain the index returned
    /// by `inline`, rather than copy an index from a separate RIO reader.
    pub fn cached(&mut self, index: u32) -> Result<()> {
        ensure!(
            index < self.next_index,
            "UUC object reference is not cached"
        );
        ensure!(
            !self.classes.values().any(|value| *value == index),
            "UUC object reference names a class"
        );
        self.tag(index);
        Ok(())
    }

    /// Serialize an inline runtime object. Reserve its identity before its
    /// body, as the native CArchive does. The callback shares this cache.
    pub fn inline(
        &mut self,
        class: &ClassId,
        body: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<u32> {
        self.class(class)?;
        self.data.extend(0xc040u16.to_le_bytes());
        let index = self.reserve()?;
        body(self)?;
        Ok(index)
    }

    /// Write an MFC CObject, such as CVmStackFrame. These objects have no
    /// project-reference flags between the class tag and their state.
    pub fn mfc_object(
        &mut self,
        class: &ClassId,
        body: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<u32> {
        self.class(class)?;
        let index = self.reserve()?;
        body(self)?;
        Ok(index)
    }

    /// Write a stored reference decoded in the native cache context. Inline
    /// bodies require their class serializer and must use `inline` instead.
    pub fn reference(&mut self, object: &ObjectRef) -> Result<()> {
        self.reference_at_depth(object, 0)
    }

    fn reference_at_depth(&mut self, object: &ObjectRef, depth: usize) -> Result<()> {
        ensure!(depth < 128, "UUC reference exceeds recursion limit");
        let Some(class) = &object.class else {
            let index = object.cache_index.unwrap_or(0);
            ensure!(index <= 2, "unbound external UUC object reference");
            return self.cached(index);
        };
        ensure!(
            object.inline.is_none() && object.flags & 0x40 == 0,
            "UUC inline object needs a class serializer"
        );
        let owner = object
            .owner
            .as_deref()
            .context("UUC stored reference owner missing")?;
        let offset = object.offset.context("UUC reference offset missing")?;
        let size = object.size.context("UUC reference size missing")?;
        let unit = 1u64 << self.shift;
        ensure!(
            offset % unit == 0 && offset / unit <= u64::from(u32::MAX),
            "UUC reference address is out of range"
        );
        ensure!(
            object.flags & 7 != 0
                || object.selector
                    <= if object.flags & 0x8000 != 0 {
                        255
                    } else {
                        65535
                    },
            "UUC reference selector is out of range"
        );
        // Build a cache identity independent of the reader's numeric indices.
        let key = identity(object, 0)?;
        if let Some(index) = self.objects.get(&key) {
            return self.cached(*index);
        }
        self.class(class)?;
        self.data.extend(object.flags.to_le_bytes());
        self.data.extend(
            ((offset >> self.shift) as u32)
                .wrapping_add(0xa2fb6ad1)
                .to_le_bytes(),
        );
        let low = size & 0x7ffff;
        let encoded =
            ((low << 13) | (((size >> 19) + (low & 0xfff)) & 0x1fff)).wrapping_add(0xe7b5d9f8);
        self.data.extend(encoded.to_le_bytes());
        self.reference_at_depth(owner, depth + 1)?;
        if object.flags & 7 != 0 {
            self.data.extend(object.selector.to_le_bytes());
        } else if object.flags & 0x8000 != 0 {
            self.data.push(object.selector as u8);
        } else {
            self.data.extend((object.selector as u16).to_le_bytes());
        }
        let index = self.reserve()?;
        self.objects.insert(key, index);
        Ok(())
    }
}

fn identity(object: &ObjectRef, depth: usize) -> Result<Vec<u8>> {
    ensure!(depth < 128, "UUC reference exceeds recursion limit");
    let mut key = Vec::new();
    if let Some(class) = &object.class {
        key.extend((class.name.len() as u32).to_le_bytes());
        key.extend(class.name.as_bytes());
        key.extend(class.schema.to_le_bytes());
        key.extend(object.flags.to_le_bytes());
        key.extend(
            object
                .offset
                .context("UUC reference offset missing")?
                .to_le_bytes(),
        );
        key.extend(
            object
                .size
                .context("UUC reference size missing")?
                .to_le_bytes(),
        );
        key.extend(object.selector.to_le_bytes());
        key.extend(identity(
            object
                .owner
                .as_deref()
                .context("UUC reference owner missing")?,
            depth + 1,
        )?);
    } else {
        key.extend(object.cache_index.unwrap_or(0).to_le_bytes());
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::ObjectReader;

    #[test]
    #[ignore = "set RUGP_TEST_GAME and RUGP_TEST_UUC_DIR to the retail game and Vmreg directories"]
    fn retail_rio_project_reference_matches_original_uuc_root() -> Result<()> {
        let game = crate::RioArchive::open(std::path::PathBuf::from(
            std::env::var_os("RUGP_TEST_GAME").context("game directory")?,
        ))?;
        let saves = std::path::PathBuf::from(
            std::env::var_os("RUGP_TEST_UUC_DIR").context("UUC directory")?,
        );
        let archive =
            super::super::NativeArchive::read(std::fs::File::open(saves.join("vmr00001.uuc"))?)?;
        let mut reader = ObjectReader::new(&archive.runtime);
        reader.input.skip(17)?;
        reader.pad(1)?;
        let native = reader.reference_without_count(game.address_shift)?;
        let mut source = None;
        for reference in game
            .project
            .as_ref()
            .context("project missing")?
            .references
            .values()
        {
            reference.visit(&mut |reference| {
                if reference.offset == native.offset && reference.class == native.class {
                    source = Some(reference.clone());
                }
            });
        }
        let converted = from_rio(&source.context("RIO root missing")?)?;
        let mut writer = ReferenceWriter::new(game.address_shift)?;
        writer.reference(&converted)?;
        assert_eq!(writer.data, archive.runtime[17..reader.input.position]);
        Ok(())
    }

    #[test]
    fn cstrings_use_native_length_boundaries_and_reject_unencodable_text() -> Result<()> {
        for length in [0, 1, 254, 255, 65533, 65534, 65535, 65536] {
            let value = "x".repeat(length);
            let mut writer = ReferenceWriter::new(0)?;
            writer.string(&value)?;
            let prefix = match length {
                0..=254 => 1,
                255..=65533 => 3,
                _ => 7,
            };
            assert_eq!(writer.data.len(), length + prefix);
            let mut reader = crate::binary::Reader::new(&writer.data);
            assert_eq!(reader.string()?, value);
            assert_eq!(reader.remaining(), 0);
        }
        let mut writer = ReferenceWriter::new(0)?;
        writer.string("設定")?;
        assert_eq!(writer.data, [4, 0x90, 0xdd, 0x92, 0xe8]);
        let before = writer.data.clone();
        assert!(writer.string("😀").is_err());
        assert_eq!(writer.data, before);
        Ok(())
    }

    #[test]
    fn stored_references_share_class_and_object_caches_across_inline_bodies() -> Result<()> {
        let root = ObjectRef {
            class: None,
            cache_index: Some(2),
            flags: 0,
            offset: None,
            size: None,
            owner: None,
            selector: 0,
            inline: None,
        };
        let object = ObjectRef {
            class: Some(ClassId {
                name: "CObjectOcean".into(),
                schema: 1,
            }),
            cache_index: None,
            flags: 0xc108,
            offset: Some(0x7fffffff8),
            size: Some(0xdeadbeef),
            owner: Some(Box::new(root)),
            selector: 7,
            inline: None,
        };
        let mut writer = ReferenceWriter::new(3)?;
        writer.reference(&object)?;
        writer.reference(&object)?;
        let timer = ClassId {
            name: "CSceneTimer".into(),
            schema: 1,
        };
        let index = writer.inline(&timer, |writer| {
            writer.data.extend([1, 0]); // version 1, empty CString
            writer.data.extend([0; 22]);
            Ok(())
        })?;
        writer.cached(index)?;
        writer.reference(&object)?;
        let mut reader = ObjectReader::new(&writer.data);
        reader.pad(1)?;
        let first = reader.reference_without_count(3)?;
        assert_eq!(first.offset, object.offset);
        assert_eq!(first.size, object.size);
        assert_eq!(first.selector, 7);
        assert_eq!(reader.reference_without_count(3)?.offset, first.offset);
        let first = reader.reference_without_count(3)?;
        let second = reader.reference_without_count(3)?;
        assert_eq!(
            first.inline.unwrap().identity(),
            second.inline.unwrap().identity()
        );
        assert_eq!(reader.reference_without_count(3)?.offset, object.offset);
        assert_eq!(reader.input.remaining(), 0);
        Ok(())
    }
}
