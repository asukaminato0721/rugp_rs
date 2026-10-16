//! Assemble native state records with one reference cache. This module does
//! not capture a live Game. Each caller must supply its class state writers.
use super::{
    HOST_CONTEXT, MAX_BYTES, NativeArchive, metadata::NativeMetadata,
    reference_writer::ReferenceWriter,
};
use crate::{object::ObjectRef, project::GameProject};
use anyhow::{Result, ensure};
use std::collections::BTreeSet;

pub struct NativeWriter {
    references: ReferenceWriter,
    shift: u32,
    title: String,
    identities: BTreeSet<u64>,
}

impl NativeWriter {
    /// The root must use UUC owner identities, including project entry 2.
    /// References from an unrelated RIO cache need conversion before this call.
    pub fn new(project: &GameProject, root: &ObjectRef, shift: u32) -> Result<Self> {
        ensure!(
            root.class
                .as_ref()
                .is_some_and(|c| c.name == "CObjectOcean")
                && root.inline.is_none(),
            "invalid UUC root reference"
        );
        let mut references = ReferenceWriter::new(shift)?;
        references.data.extend(19u32.to_le_bytes());
        references.data.extend(0u32.to_le_bytes()); // NativeArchive sets the encoded offset
        references.data.push(1);
        references.data.extend(0x431u32.to_le_bytes());
        references.data.extend(20u32.to_le_bytes());
        references.reference(root)?;
        references.data.push(0);
        for value in [
            &project.game_version,
            &project.title,
            &project.scenario_version,
            &project.copyright,
            &project.title,
        ] {
            references.string(value)?;
        }
        references.data.extend(HOST_CONTEXT);
        Ok(Self {
            references,
            shift,
            title: project.title.clone(),
            identities: BTreeSet::new(),
        })
    }

    /// Consume the builder. A failed callback cannot leave a reusable partial
    /// reference cache. This API does not open an output file.
    pub fn record(
        mut self,
        offset: u64,
        body: impl FnOnce(&mut ReferenceWriter) -> Result<()>,
    ) -> Result<Self> {
        let unit = 1u64 << self.shift;
        ensure!(
            offset.is_multiple_of(unit) && offset / unit <= u64::from(u32::MAX),
            "UUC state address is out of range"
        );
        ensure!(
            self.identities.len() < 65536 && self.identities.insert(offset),
            "duplicate or excessive UUC state identity"
        );
        self.references.data.extend([1, 2, 0]);
        self.references
            .data
            .extend(((offset >> self.shift) as u32).to_le_bytes());
        self.delimited(body)?;
        Ok(self)
    }

    fn delimited(&mut self, body: impl FnOnce(&mut ReferenceWriter) -> Result<()>) -> Result<()> {
        let position = self.references.data.len();
        self.references.data.extend([0; 4]);
        let start = self.references.data.len();
        body(&mut self.references)?;
        ensure!(
            self.references.data.len() <= MAX_BYTES,
            "UUC runtime exceeds size limit"
        );
        let size = (self.references.data.len() - start) as u32;
        self.references.data[position..start].copy_from_slice(&size.to_le_bytes());
        Ok(())
    }

    fn manager(
        &mut self,
        name: &str,
        body: impl FnOnce(&mut ReferenceWriter) -> Result<()>,
    ) -> Result<()> {
        self.references.data.push(0);
        self.references.string(name)?;
        self.delimited(body)
    }

    pub fn finish(
        mut self,
        navigation: &str,
        metadata: &NativeMetadata,
        thread_media: impl FnOnce(&mut ReferenceWriter) -> Result<()>,
        sound: impl FnOnce(&mut ReferenceWriter) -> Result<()>,
    ) -> Result<NativeArchive> {
        ensure!(
            metadata.registry == self.title && metadata.application.ends_with(&self.title),
            "UUC metadata project mismatch"
        );
        self.manager("_DevRoot/AbstThreadMediaMng", thread_media)?;
        self.manager("_DevRoot/AbstSoundMng", sound)?;
        self.references.data.push(0);
        self.references.string(navigation)?;
        ensure!(
            self.references.data.len() <= MAX_BYTES,
            "UUC runtime exceeds size limit"
        );
        Ok(NativeArchive {
            runtime: self.references.data,
            metadata: metadata.write()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{NativeSave, metadata::Thumbnail};
    use super::*;
    use crate::object::ClassId;

    fn root() -> ObjectRef {
        ObjectRef {
            class: Some(ClassId {
                name: "CObjectOcean".into(),
                schema: 1,
            }),
            cache_index: None,
            flags: 0xc108,
            offset: Some(8),
            size: Some(0),
            owner: Some(Box::new(ObjectRef {
                class: None,
                cache_index: Some(2),
                flags: 0,
                offset: None,
                size: None,
                owner: None,
                selector: 0,
                inline: None,
            })),
            selector: 0,
            inline: None,
        }
    }

    fn project() -> GameProject {
        GameProject {
            version: 0x29,
            references: Default::default(),
            registry: "age\\Game".into(),
            game_version: "Version".into(),
            title: "Game".into(),
            scenario_version: "Script".into(),
            copyright: String::new(),
        }
    }

    #[test]
    fn stream_records_share_cache_and_native_container_reads_new_states() -> Result<()> {
        let root = root();
        let project = project();
        let writer = NativeWriter::new(&project, &root, 3)?
            .record(0x100, |writer| writer.reference(&root))?
            .record(0x108, |writer| {
                writer.data.extend(42i32.to_le_bytes());
                Ok(())
            })?;
        let scene = ObjectRef {
            class: Some(ClassId {
                name: "CRsa".into(),
                schema: 5,
            }),
            offset: Some(0x200),
            owner: Some(Box::new(root.clone())),
            ..root.clone()
        };
        let writer = writer.record(0x110, |writer| {
            super::super::vm::DialogueThreadState {
                scene: &scene,
                instruction: 7,
                timer: super::super::vm::SceneTimerState {
                    name: "",
                    flags: 0x20201,
                    pending_ticks: 0,
                    position: 0,
                    target: 0,
                    numerator: 1,
                    denominator: 1000,
                },
                status: 1,
            }
            .write_process(writer, "")
        })?;
        let metadata = NativeMetadata {
            application: project.registry.clone(),
            registry: project.title.clone(),
            thumbnail: Thumbnail {
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 255],
            },
            navigation: "Chapter".into(),
            clock: 0,
            label: String::new(),
            summary: "Text".into(),
            comment: "comment={Text}".into(),
        };
        let archive = writer.finish(
            "Chapter",
            &metadata,
            |writer| writer.cached(4),
            |writer| writer.cached(4),
        )?;
        let mut bytes = Vec::new();
        archive.write(&mut bytes)?;
        let saved = NativeSave::read(bytes.as_slice(), 3)?;
        assert_eq!(saved.title, "Version");
        assert_eq!(saved.objects[&0x100], [4, 0]);
        assert_eq!(saved.objects[&0x108], 42i32.to_le_bytes());
        assert_eq!(saved.navigation, "Chapter");
        assert_eq!(saved.dialogue_cursor(0x110, 3)?, (0x200, 7));
        assert_eq!(
            NativeMetadata::read(&NativeArchive::read(bytes.as_slice())?.metadata)?,
            metadata
        );
        assert!(
            NativeWriter::new(&project, &root, 3)?
                .record(0x101, |_| Ok(()))
                .is_err()
        );
        assert!(
            NativeWriter::new(&project, &root, 3)?
                .record(0x100, |_| Ok(()))?
                .record(0x100, |_| Ok(()))
                .is_err()
        );
        Ok(())
    }
}
