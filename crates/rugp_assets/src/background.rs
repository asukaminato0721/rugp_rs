//! CBg2d's image planes and native display-space metadata.
use crate::object::{ClassId, ObjectReader, ObjectRef};
use anyhow::{Context, Result, ensure};

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
        Ok(Self::read(data, schema, shift)?.0)
    }
    fn read(data: &[u8], schema: u16, shift: u32) -> Result<(Self, ObjectReader<'_>)> {
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
        let trailing_data = data[r.input.position..].to_vec();
        Ok((
            Self {
                width,
                height,
                display_metadata,
                flags,
                planes,
                properties,
                trailing_data,
            },
            r,
        ))
    }
}

/// Schema-6 extension, serialized after the legacy image planes. Raw MFC
/// layer objects and RIO image references share the enclosing archive cache.
pub struct SceneBackground {
    pub background: Background,
    pub color: u32,
    pub screen: [i16; 2],
    pub range: [i32; 2],
    pub scale: f32,
    pub layers: Vec<BackgroundLayer>,
}
pub struct BackgroundLayer {
    pub anchor: [i16; 2],
    pub position: [i32; 3],
    pub extra: [i32; 3],
    pub scale: f32,
    pub image: ObjectRef,
    pub name: String,
    pub id: u16,
    pub attributes: [u8; 2],
    pub parameters: [i32; 3],
    pub flags: u32,
}
impl SceneBackground {
    pub fn parse(data: &[u8], schema: u16, shift: u32) -> Result<Self> {
        let (background, mut r) = Background::read(data, schema, shift)?;
        let version = r.input.u8()?;
        ensure!(version == 1, "unsupported CBg2d extension {version}");
        let color = r.input.u32()?;
        let screen = [r.input.u16()? as i16, r.input.u16()? as i16];
        let range = [r.input.u32()? as i32, r.input.u32()? as i32];
        let scale = f32::from_bits(r.input.u32()?);
        ensure!(scale.is_finite(), "non-finite CBg2d scale");
        let mut layers = Vec::new();
        read_layer(&mut r, shift, &mut layers, 0).context("CBg2d layer list")?;
        let count = r.input.u32()? as usize;
        ensure!(
            count == layers.len(),
            "CBg2d layer count {count} != {} at {:#x}, {} remaining",
            layers.len(),
            r.input.position,
            r.input.remaining()
        );
        ensure!(r.input.remaining() == 0, "trailing CBg2d extension bytes");
        Ok(Self {
            background,
            color,
            screen,
            range,
            scale,
            layers,
        })
    }
}

// rvmm 1005add0 reads the derived image fields *before* 1005ac20
// recursively reads the next pointer and then the base parameter fields.
fn read_layer(
    r: &mut ObjectReader<'_>,
    shift: u32,
    layers: &mut Vec<BackgroundLayer>,
    depth: usize,
) -> Result<()> {
    ensure!(depth < 64, "CBg2d layer recursion limit");
    let start = r.input.position;
    let Some(class) = r.class()? else {
        ensure!(
            r.input.consumed_since(start)? == [0, 0],
            "cached/cyclic CBg2d layer pointer"
        );
        return Ok(());
    };
    ensure!(
        class.name == "CBg2dLayerRip007Image" && class.schema == 1,
        "unsupported background layer {}#{}",
        class.name,
        class.schema
    );
    r.pad(1)?; // ReadObject reserves its object slot before Serialize.
    let anchor = [r.input.u16()? as i16, r.input.u16()? as i16];
    let position = [
        r.input.u32()? as i32,
        r.input.u32()? as i32,
        r.input.u32()? as i32,
    ];
    let extra = [
        r.input.u32()? as i32,
        r.input.u32()? as i32,
        r.input.u32()? as i32,
    ];
    let scale = f32::from_bits(r.input.u32()?);
    ensure!(scale.is_finite(), "non-finite background layer scale");
    let image = r.reference(shift)?;
    let index = layers.len();
    layers.push(BackgroundLayer {
        anchor,
        position,
        extra,
        scale,
        image,
        name: String::new(),
        id: 0,
        attributes: [0; 2],
        parameters: [0; 3],
        flags: 0,
    });
    read_layer(r, shift, layers, depth + 1)?;
    let layer = &mut layers[index];
    layer.name = r.input.string()?;
    layer.id = r.input.u16()?;
    layer.attributes = [r.input.u8()?, r.input.u8()?];
    layer.parameters = [
        r.input.u32()? as i32,
        r.input.u32()? as i32,
        r.input.u32()? as i32,
    ];
    layer.flags = r.input.u32()?;
    Ok(())
}
