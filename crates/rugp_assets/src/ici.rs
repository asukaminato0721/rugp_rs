use crate::{crypto::decode_ici, object::ObjectReader};
use anyhow::{Result, ensure};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct InstallSource {
    pub version: u16,
    pub source_name: String,
    pub destination_name: String,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallManifest {
    pub version: u32,
    pub title: String,
    pub toc_offset: u64,
    pub toc_size: u32,
    pub rio_name: String,
    pub sources: Vec<InstallSource>,
    /// Version-specific installer metadata following the source list.
    pub trailing_data: Vec<u8>,
}

impl InstallManifest {
    pub fn decode(data: &[u8]) -> Result<Self> {
        Self::parse(&decode_ici(data)?)
    }
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut arc = ObjectReader::new(data);
        let class = arc.header()?;
        ensure!(
            class.name == "CObjectArcMan",
            "expected CObjectArcMan, got {}",
            class.name
        );
        ensure!(
            arc.input.count()? == 0,
            "ICI root contains unexpected children"
        );
        let r = &mut arc.input;
        let version = r.u32()?;
        ensure!(
            (6..=10).contains(&version),
            "unsupported ICI version {version}"
        );
        r.skip(6)?;
        if version >= 10 {
            r.skip(8)?;
        }
        r.skip(12)?;
        let toc_offset = r.u32()? as u64;
        let toc_size = r.u32()?;
        r.u32()?;
        if version >= 8 {
            r.u32()?;
        }
        let title = r.string()?;
        r.u32()?;
        r.string()?;
        r.u32()?;
        r.string()?;
        r.string()?;
        r.string()?;
        r.u32()?;
        r.string()?;
        let count = r.count()?;
        ensure!(count <= r.remaining(), "invalid ICI string count");
        for _ in 0..count {
            r.string()?;
        }
        r.u32()?;
        let rio_name = if version >= 9 {
            r.string()?
        } else {
            String::new()
        };
        if version >= 7 {
            r.string()?;
        }
        r.u32()?;
        let count = r.count()?;
        ensure!(count <= 4096, "too many install sources");
        let mut sources = Vec::new();
        for _ in 0..count {
            if r.u8()? == 0 {
                continue;
            }
            let version = r.u16()?;
            ensure!(
                (1..=7).contains(&version),
                "unsupported install source version {version}"
            );
            if version >= 7 {
                r.skip(9)?;
                r.string()?;
            }
            r.string()?;
            r.string()?;
            let source_name = r.string()?;
            r.string()?;
            r.string()?;
            r.skip(16)?;
            r.skip(if version < 6 { 8 } else { 4 })?;
            let destination_name = r.string()?;
            let offset = r.u64()?;
            let size = r.u64()?;
            if version < 6 {
                r.u64()?;
            }
            r.u32()?;
            r.string()?;
            r.skip(20)?;
            r.string()?;
            let count = r.count()?;
            r.skip(
                count
                    .checked_mul(4)
                    .ok_or_else(|| anyhow::anyhow!("invalid source array length"))?,
            )?;
            let bit_bytes = size
                .checked_add(0xffff)
                .and_then(|n| (n >> 16).checked_add(7))
                .ok_or_else(|| anyhow::anyhow!("invalid source size"))?
                >> 3;
            r.skip(usize::try_from(bit_bytes)?)?;
            sources.push(InstallSource {
                version,
                source_name,
                destination_name,
                offset,
                size,
            });
        }
        let trailing_data = r.bytes(r.remaining())?.to_vec();
        Ok(Self {
            version,
            title,
            toc_offset,
            toc_size,
            rio_name,
            sources,
            trailing_data,
        })
    }
}
