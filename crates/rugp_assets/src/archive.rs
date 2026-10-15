use crate::{
    InstallManifest, ObjectNode,
    crypto::{MAX_CHUNK_SIZE, RIO_KEY, read_encrypted},
    object::{ENCRYPTED_SIGNATURE, ObjectReader},
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize)]
pub struct Volume {
    pub path: PathBuf,
    pub offset: u64,
    pub size: u64,
}

/// Volumes are kept on disk and read by range; opening a game never loads its gigabytes of assets.
pub struct RioArchive {
    pub manifest: InstallManifest,
    pub volumes: Vec<Volume>,
    pub nodes: Vec<ObjectNode>,
    pub address_shift: u32,
    pub project_data: Vec<u8>,
    pub project: Option<crate::project::GameProject>,
}

impl RioArchive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let path = if path.is_dir() {
            let mut files: Vec<_> = fs::read_dir(path)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|s| s.eq_ignore_ascii_case("rio")))
                .collect();
            files.sort();
            ensure!(
                files.len() == 1,
                "expected one .rio file in {}, found {}",
                path.display(),
                files.len()
            );
            files.remove(0)
        } else {
            path.to_owned()
        };
        let ici = PathBuf::from(format!("{}.ici", path.display()));
        let ici = if ici.is_file() {
            ici
        } else {
            path.with_extension("ici")
        };
        let manifest = InstallManifest::decode(
            &fs::read(&ici).with_context(|| format!("read {}", ici.display()))?,
        )
        .context("decode ICI install manifest")?;
        let mut volumes = Vec::new();
        let mut offset = 0u64;
        for n in 1..=999 {
            let volume_path = if n == 1 {
                path.clone()
            } else {
                PathBuf::from(format!("{}.{n:03}", path.display()))
            };
            if !volume_path.is_file() {
                break;
            }
            let size = fs::metadata(&volume_path)?.len();
            volumes.push(Volume {
                path: volume_path,
                offset,
                size,
            });
            offset = offset.checked_add(size).context("volume size overflow")?;
        }
        ensure!(!volumes.is_empty(), "no RIO volumes");
        let mut archive = Self {
            manifest,
            volumes,
            nodes: Vec::new(),
            address_shift: 0,
            project_data: Vec::new(),
            project: None,
        };
        let base = archive.manifest.toc_offset;
        let mut toc_offset = base;
        if archive.read_at(base, 4)? != ENCRYPTED_SIGNATURE.to_le_bytes() {
            toc_offset = base.checked_mul(2).context("TOC offset overflow")?;
            ensure!(
                archive.read_at(toc_offset, 4)? == ENCRYPTED_SIGNATURE.to_le_bytes(),
                "encrypted RIO project signature not found at {base:#x} or {toc_offset:#x}"
            );
            archive.address_shift = 1;
        }
        let toc = archive.read_at(toc_offset, archive.manifest.toc_size as usize)?;
        let mut reader = ObjectReader::new(&toc);
        let class = reader.header()?;
        ensure!(
            class.name == "CrelicUnitedGameProject",
            "unexpected project class {}",
            class.name
        );
        archive.nodes = reader
            .nodes(archive.address_shift)
            .context("decode project object tree")?;
        archive.project_data =
            read_encrypted(&mut reader.input, RIO_KEY).context("decrypt project metadata")?;
        archive.project = Some(
            crate::project::GameProject::read(
                &mut reader.with_data(&archive.project_data),
                archive.address_shift,
            )
            .context("parse game project")?,
        );
        ensure!(
            reader.input.remaining() <= 3,
            "unexpected data following project: {} bytes",
            reader.input.remaining()
        );
        Ok(archive)
    }
    pub fn len(&self) -> u64 {
        self.volumes.last().map_or(0, |v| v.offset + v.size)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn read_at(&self, offset: u64, size: usize) -> Result<Vec<u8>> {
        ensure!(size <= MAX_CHUNK_SIZE, "read exceeds memory limit: {size}");
        ensure!(
            offset
                .checked_add(size as u64)
                .is_some_and(|n| n <= self.len()),
            "RIO extent outside volumes: {offset:#x}+{size:#x} (length {:#x})",
            self.len()
        );
        let mut result = vec![0; size];
        let mut cursor = offset;
        let mut written = 0;
        for volume in &self.volumes {
            if cursor < volume.offset || cursor >= volume.offset + volume.size {
                continue;
            }
            let length = ((volume.offset + volume.size - cursor) as usize).min(size - written);
            let mut file = File::open(&volume.path)?;
            file.seek(SeekFrom::Start(cursor - volume.offset))?;
            file.read_exact(&mut result[written..written + length])?;
            cursor += length as u64;
            written += length;
            if written == size {
                break;
            }
        }
        ensure!(written == size, "incomplete RIO extent read");
        Ok(result)
    }
    pub fn read_node(&self, id: usize) -> Result<Vec<u8>> {
        let node = self.nodes.get(id).context("object ID out of range")?;
        self.read_at(
            node.offset.context("object has no stored extent")?,
            node.size.context("object has no stored extent")? as usize,
        )
    }
}
