use crate::{Argument, Scenario};
use anyhow::{Context, Result, ensure};
use rugp_assets::{
    RioArchive,
    native_schema::NativeSchema,
    object::{ClassId, ObjectRef},
    script::Script,
};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct Resource {
    pub class: ClassId,
    pub offset: u64,
    pub size: u32,
}

#[derive(Debug, Default, Serialize)]
pub struct ResourceIndex {
    pub resources: BTreeMap<u64, Resource>,
    pub instructions: BTreeMap<String, usize>,
    pub messages: BTreeMap<String, usize>,
    pub dialogue_count: usize,
}

impl ResourceIndex {
    pub fn scan(archive: &RioArchive, native: &NativeSchema) -> Result<Self> {
        let mut index = Self::default();
        for node in &archive.nodes {
            if let (Some(offset), Some(size)) = (node.offset, node.size) {
                index.resources.insert(
                    offset,
                    Resource {
                        class: node.class.clone(),
                        offset,
                        size,
                    },
                );
            }
            if node.class.name != "CRsa" {
                continue;
            }
            let script = Script::load(archive, node.id)?;
            let scene = Scenario::decode_with_schema(&script, archive.address_shift, Some(native))?;
            scene.visit_references(&mut |reference| {
                index.add(reference);
            });
            for command in &scene.instructions {
                *index
                    .instructions
                    .entry(command.class.name.clone())
                    .or_default() += 1;
                if command.class.name == "CVmMsg" {
                    index.dialogue_count += 1;
                }
                for argument in &command.arguments {
                    if let Argument::Message { class, .. } = argument {
                        *index.messages.entry(class.name.clone()).or_default() += 1;
                    }
                }
            }
        }
        for resource in index.resources.values() {
            ensure!(
                resource
                    .offset
                    .checked_add(resource.size as u64)
                    .is_some_and(|end| end <= archive.len()),
                "{} resource {:#x}+{:#x} outside archive",
                resource.class.name,
                resource.offset,
                resource.size
            );
        }
        Ok(index)
    }
    fn add(&mut self, reference: &ObjectRef) {
        if let (Some(class), Some(offset), Some(size)) =
            (&reference.class, reference.offset, reference.size)
        {
            self.resources.entry(offset).or_insert_with(|| Resource {
                class: class.clone(),
                offset,
                size,
            });
        }
    }
    /// Image-menu states and hit maps are indirect resources, not scene
    /// operands. Keep expansion explicit, like sound-wrapper expansion.
    pub fn expand_menu_sources(&mut self, archive: &RioArchive) -> Result<()> {
        let menus: Vec<_> = self
            .resources
            .values()
            .filter(|r| r.class.name == "CRbx")
            .cloned()
            .collect();
        for resource in menus {
            ensure!(resource.size <= 16 * 1024 * 1024, "oversized menu resource");
            let menu = rugp_assets::menu::ImageMenu::parse(
                &archive.read_at(resource.offset, resource.size as usize)?,
                resource.class.schema,
                archive.address_shift,
            )?;
            menu.visit_references(&mut |r| self.add(r));
        }
        for r in self.resources.values() {
            ensure!(
                r.offset
                    .checked_add(u64::from(r.size))
                    .is_some_and(|end| end <= archive.len()),
                "expanded menu resource outside archive"
            );
        }
        Ok(())
    }
    pub fn read(&self, archive: &RioArchive, offset: u64) -> Result<Vec<u8>> {
        let resource = self
            .resources
            .get(&offset)
            .context("resource offset not in scene reference index")?;
        archive.read_at(resource.offset, resource.size as usize)
    }
    /// Resolve message-window borders without changing the direct-reference
    /// coverage counts reported by the normal scan.
    pub fn expand_message_sources(&mut self, archive: &RioArchive) -> Result<()> {
        let windows: Vec<_> = self
            .resources
            .values()
            .filter(|r| r.class.name == "CMsgSelBox")
            .cloned()
            .collect();
        for resource in windows {
            ensure!(
                resource.size <= 16 * 1024 * 1024,
                "oversized message window"
            );
            let window = rugp_assets::message::MessageBox::parse(
                &archive.read_at(resource.offset, resource.size as usize)?,
                resource.class.schema,
                archive.address_shift,
            )?;
            window.visit_references(&mut |r| self.add(r));
        }
        let frames: Vec<_> = self
            .resources
            .values()
            .filter(|r| r.class.name == "CFrameImage")
            .cloned()
            .collect();
        for resource in frames {
            ensure!(resource.size <= 16 * 1024 * 1024, "oversized frame image");
            let frame = rugp_assets::message::FrameImage::parse(
                &archive.read_at(resource.offset, resource.size as usize)?,
                resource.class.schema,
                archive.address_shift,
            )?;
            frame.visit_references(&mut |r| self.add(r));
        }
        for r in self.resources.values() {
            ensure!(
                r.offset
                    .checked_add(u64::from(r.size))
                    .is_some_and(|end| end <= archive.len()),
                "expanded message resource outside archive"
            );
        }
        Ok(())
    }
    /// Include sources stored inside BGM/effect wrappers, rather than only
    /// the direct scene references. Voice OOR extents are already direct.
    pub fn expand_sound_sources(&mut self, archive: &RioArchive) -> Result<()> {
        let wrappers: Vec<_> = self
            .resources
            .values()
            .filter(|r| ["CPmBgm", "CPmEffect"].contains(&r.class.name.as_str()) && r.size > 3)
            .cloned()
            .collect();
        for wrapper in wrappers {
            let bytes = archive.read_at(wrapper.offset, wrapper.size as usize)?;
            let sample = rugp_assets::sound::Sample::parse(&bytes, archive.address_shift)
                .with_context(|| format!("{} at {:#x}", wrapper.class.name, wrapper.offset))?;
            sample.source.visit(&mut |r| self.add(r));
        }
        for r in self.resources.values() {
            ensure!(
                r.offset
                    .checked_add(r.size as u64)
                    .is_some_and(|end| end <= archive.len()),
                "expanded sound resource outside archive"
            );
        }
        Ok(())
    }
}
