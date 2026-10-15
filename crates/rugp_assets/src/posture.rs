//! CPostureMoment#4 data, Vm60 1001d680..1001db54.
//!
//! References share the enclosing ObjectReader cache. In particular an inline
//! body's first reference need not carry a new ancestry header. Do not parse
//! its copied byte buffer with a fresh ObjectReader to recover these fields.
use crate::{
    binary::Reader,
    object::{ClassId, InlineObject, ObjectReader, ObjectRef},
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct PostureFields {
    /// Serialized native +10 state byte; activation is a separate operation.
    pub state: u8,
    pub version: u8,
    /// Native +27 options, retained without interpreting unported modes.
    pub options: u8,
    /// Native +38 vector (returned by 1001d5c0).
    pub base_point: [i32; 3],
    /// Stored elapsed time (+6c), -1 denotes the unstarted state.
    pub elapsed_ms: i32,
    /// Raw serialized values, not rescaled script angles.
    pub angles: [i32; 3],
    pub position: [i32; 3],
    pub other: [i32; 3],
    pub position_mode: u16,
    pub angle_mode: u16,
    pub other_mode: u16,
    /// Position, angular and other channel multipliers (+78/+7c/+80).
    pub scales: [f32; 3],
}
#[derive(Debug, Clone)]
pub struct Posture {
    pub fields: PostureFields,
    /// Previous motion, owning window, position, angular and other motion data.
    /// Sampling at 1001e360/1001e5d0, not reflection labels, establishes this order.
    pub references: [ObjectRef; 5],
}
impl Posture {
    pub fn parse(data: &[u8], class: &ClassId, shift: u32) -> Result<Self> {
        let mut reader = ObjectReader::new(data);
        reader.input.skip(3)?;
        reader.scrambled = true;
        reader.pad(3)?; // Standalone RIO object/class context, like CPm/CBg2d.
        let posture = Self::read_body(&mut reader, class, shift)?;
        ensure!(
            reader.input.remaining() == 0,
            "{} trailing posture bytes",
            reader.input.remaining()
        );
        Ok(posture)
    }
    pub fn from_inline(inline: &InlineObject, class: &ClassId) -> Result<Self> {
        validate_class(class)?;
        Ok(Self {
            fields: inline
                .posture
                .as_deref()
                .cloned()
                .context("inline posture fields missing")?,
            references: inline
                .references
                .clone()
                .try_into()
                .map_err(|_| anyhow::anyhow!("posture requires five bound references"))?,
        })
    }
    pub(crate) fn read_body(
        reader: &mut ObjectReader<'_>,
        class: &ClassId,
        shift: u32,
    ) -> Result<Self> {
        validate_class(class)?;
        let state = reader.input.u8()?;
        let version = reader.input.u8()?;
        ensure!(version <= 2, "unsupported posture version {version}");
        let options = reader.input.u8()?;
        let references = [
            reader.reference(shift)?,
            reader.reference(shift)?,
            reader.reference(shift)?,
            reader.reference(shift)?,
            reader.reference(shift)?,
        ];
        let input = &mut reader.input;
        let base_point = vector(input)?;
        let elapsed_ms = input.u32()? as i32;
        // Schema 4 always stores these two vectors, even at inner version 0.
        let angles = vector(input)?;
        let position = vector(input)?;
        let other = if version >= 2 { vector(input)? } else { [0; 3] };
        let position_mode = input.u16()?;
        let angle_mode = input.u16()?;
        let other_mode = if version >= 2 { input.u16()? } else { 0 };
        let scales = [
            f32::from_bits(input.u32()?),
            f32::from_bits(input.u32()?),
            f32::from_bits(input.u32()?),
        ];
        ensure!(
            scales.iter().all(|v| v.is_finite()),
            "non-finite posture scale"
        );
        Ok(Self {
            fields: PostureFields {
                state,
                version,
                options,
                base_point,
                elapsed_ms,
                angles,
                position,
                other,
                position_mode,
                angle_mode,
                other_mode,
                scales,
            },
            references,
        })
    }
}
fn vector(input: &mut Reader<'_>) -> Result<[i32; 3]> {
    Ok([
        input.u32()? as i32,
        input.u32()? as i32,
        input.u32()? as i32,
    ])
}
fn validate_class(class: &ClassId) -> Result<()> {
    ensure!(
        class.name == "CPostureMoment" && class.schema == 4,
        "unsupported posture {}#{}",
        class.name,
        class.schema
    );
    Ok(())
}
