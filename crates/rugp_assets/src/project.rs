use crate::object::{ObjectReader, ObjectRef};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub struct GameProject {
    pub version: u32,
    pub references: BTreeMap<String, ObjectRef>,
    pub registry: String,
    pub game_version: String,
    pub title: String,
    pub scenario_version: String,
    pub copyright: String,
}

impl GameProject {
    pub fn read(r: &mut ObjectReader<'_>, shift: u32) -> Result<Self> {
        let version = r.input.u32()?;
        ensure!(
            (0x24..=0x29).contains(&version),
            "unsupported project version {version:#x}"
        );
        let mut references = BTreeMap::new();
        for name in [
            "database",
            "database_aux",
            "boxes",
            "objects",
            "objects_aux",
            "processes",
        ] {
            references.insert(name.to_owned(), r.reference(shift)?);
        }
        for (minimum, name) in [
            (0x25, "settings"),
            (0x26, "rio"),
            (0x27, "rio_aux"),
            (0x29, "rio_extra"),
        ] {
            if version >= minimum {
                references.insert(name.to_owned(), r.reference(shift)?);
            }
        }
        let metadata_version = r.input.u32()?;
        ensure!(
            metadata_version <= 6,
            "unsupported game metadata version {metadata_version}"
        );
        let registry = if metadata_version >= 2 {
            r.input.string()?
        } else {
            String::new()
        };
        let game_version = r.input.string()?;
        let title = r.input.string()?;
        let scenario_version = r.input.string()?;
        let copyright = r.input.string()?;
        r.input.u32()?;
        if metadata_version > 0 {
            if metadata_version >= 2 {
                r.input.skip(20)?;
            }
            r.input.string()?;
            r.input.string()?;
            if metadata_version >= 3 {
                r.input.string()?;
            }
            if metadata_version >= 4 {
                r.input.string()?;
                r.input.u32()?;
            }
            if metadata_version >= 5 {
                r.input.u32()?;
            }
            if metadata_version >= 6 {
                r.input.string()?;
                r.input.string()?;
                r.input.string()?;
            }
        }
        if version >= 0x28 {
            references.insert("startup".to_owned(), r.reference(shift)?);
        }
        ensure!(
            r.input.remaining() == 0,
            "unparsed project metadata: {} bytes",
            r.input.remaining()
        );
        Ok(Self {
            version,
            references,
            registry,
            game_version,
            title,
            scenario_version,
            copyright,
        })
    }
}
