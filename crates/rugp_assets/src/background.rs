//! CBg2d's image planes and native display-space metadata.
use crate::object::{ClassId, ObjectReader, ObjectRef};
use anyhow::{Result, ensure};

pub struct BackgroundPlane {
    pub metadata: [u8; 12],
    pub image: ObjectRef,
}
/// CBg2d's static-plane display fields, in their serialized order. The
/// Q16 factor scales both the base point and dimensions, but not depth.
/// rvmm 1005a74a..1005a766 reads these into +48/+58/+5c/+4c..+54;
/// Vm60 1003e3c4..1003e45d constructs the world plane from them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackgroundPlacement {
    pub mode: u16,
    pub scale_q16: i32,
    pub pitch_bias: i16,
    pub origin: [i32; 2],
    pub depth: i32,
}
pub struct Background {
    pub width: u16,
    pub height: u16,
    pub display_metadata: [u8; 20],
    pub flags: u32,
    pub planes: Vec<BackgroundPlane>,
    pub properties: Option<ObjectRef>,
    pub trailing_data: Vec<u8>,
}
impl Background {
    pub fn placement(&self) -> BackgroundPlacement {
        let b = &self.display_metadata;
        BackgroundPlacement {
            mode: u16::from_le_bytes(b[0..2].try_into().unwrap()),
            scale_q16: i32::from_le_bytes(b[2..6].try_into().unwrap()),
            pitch_bias: i16::from_le_bytes(b[6..8].try_into().unwrap()),
            origin: [
                i32::from_le_bytes(b[8..12].try_into().unwrap()),
                i32::from_le_bytes(b[12..16].try_into().unwrap()),
            ],
            depth: i32::from_le_bytes(b[16..20].try_into().unwrap()),
        }
    }
    pub fn parse(data: &[u8], schema: u16, shift: u32) -> Result<Self> {
        ensure!(schema == 6, "unsupported CBg2d schema {schema}");
        let mut r = ObjectReader::new(data);
        r.input.skip(3)?;
        r.scrambled = true;
        r.pad(3)?;
        let image_schema = r.input.u16()?;
        r.cache_class(ClassId {
            name: "Cr6Ti".to_owned(),
            schema: image_schema,
        });
        let version = r.input.u16()?;
        ensure!(version == 0, "unsupported CBg2d version {version}");
        let width = r.input.u16()?;
        let height = r.input.u16()?;
        let display_metadata = r.input.bytes(20)?.try_into()?;
        let flags = r.input.u32()?;
        if flags & 2 != 0 {
            r.input.skip(6)?;
        }
        if flags & 8 != 0 {
            r.input.skip(2)?;
        }
        if flags & 4 != 0 {
            r.input.skip(12)?;
        }
        let count = r.input.u16()? as usize;
        ensure!(count <= 4, "too many CBg2d planes: {count}");
        let mut planes = Vec::with_capacity(count);
        for _ in 0..count {
            let metadata = r.input.bytes(12)?.try_into()?;
            let image = r.reference(shift)?;
            planes.push(BackgroundPlane { metadata, image });
        }
        let properties = match r.input.u8()? {
            1 => Some(r.reference(shift)?),
            0 => None,
            value => anyhow::bail!("invalid CBg2d property discriminator {value}"),
        };
        let trailing_data = r.input.bytes(r.input.remaining())?.to_vec();
        Ok(Self {
            width,
            height,
            display_metadata,
            flags,
            planes,
            properties,
            trailing_data,
        })
    }
}
