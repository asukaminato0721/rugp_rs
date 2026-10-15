//! Native CMoveAcsOngen#3 motion resources (rvmm 10007350).
//! Points are sequential segments, not absolute-time keyframes. Their embedded
//! MFC timing objects share the enclosing archive cache with RIO references.
use crate::{
    binary::Reader,
    object::{ClassId, InlineObject, ObjectReader},
};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub enum TimeCurve {
    /// CMN_Time_2G#0: native value 1 eases out; all other values ease in.
    Quadratic(i32),
    /// CMN_Time_Bezier#0: six f32 values per native knot. Sampling is separate.
    Bezier(Vec<[f32; 6]>),
}
impl TimeCurve {
    pub(crate) fn read_body(input: &mut Reader<'_>, class: &ClassId) -> Result<Self> {
        match (class.name.as_str(), class.schema) {
            ("CMN_Time_2G", 0) => Ok(Self::Quadratic(input.u32()? as i32)),
            ("CMN_Time_Bezier", 0) => {
                let count = input.u16()? as usize;
                ensure!(
                    count <= input.remaining() / 24,
                    "truncated time Bezier knots"
                );
                let mut knots = Vec::with_capacity(count);
                for _ in 0..count {
                    let mut knot = [0.0; 6];
                    for value in &mut knot {
                        *value = finite_float(input)?;
                    }
                    knots.push(knot);
                }
                Ok(Self::Bezier(knots))
            }
            _ => bail!(
                "unsupported motion time curve {}#{}",
                class.name,
                class.schema
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct MotionSegment {
    /// Native 2 = line, 3 = Bezier with optional control points.
    pub kind: u8,
    pub options: u8,
    pub duration_ms: i32,
    pub end: [i32; 3],
    pub controls: Vec<[i32; 3]>,
    pub time_curve: Option<TimeCurve>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct MotionCurve {
    pub start: [i32; 3],
    pub segments: Vec<MotionSegment>,
    /// Native +48; -1 selects the sequential per-segment durations.
    pub timing_mode: i32,
    /// Native +4c; used when timing_mode is not -1.
    pub duration_ms: i32,
    pub rate: f32,
    pub dimensions: u8,
    /// Native +46: bit 0 selects the final endpoint on expiry; bit 1 loops.
    pub flags: u16,
}
impl MotionCurve {
    pub fn parse(data: &[u8], class: &ClassId) -> Result<Self> {
        let mut reader = ObjectReader::new(data);
        reader.input.skip(3)?;
        let curve = Self::read_body(&mut reader, class)?;
        ensure!(
            reader.input.remaining() == 0,
            "{} trailing motion curve bytes after {}",
            reader.input.remaining(),
            reader.input.position
        );
        Ok(curve)
    }
    pub fn from_inline(inline: &InlineObject, class: &ClassId) -> Result<Self> {
        validate_class(class)?;
        inline
            .motion
            .as_deref()
            .cloned()
            .context("inline motion fields missing")
    }
    pub(crate) fn read_body(reader: &mut ObjectReader<'_>, class: &ClassId) -> Result<Self> {
        validate_class(class)?;
        let start = vector(&mut reader.input, true)?;
        let mut segments = Vec::new();
        loop {
            let kind = reader.input.u8()?;
            if kind == 0x70 {
                break;
            }
            ensure!(
                kind == 2 || kind == 3,
                "unsupported motion segment type {kind}"
            );
            ensure!(segments.len() < 65536, "motion segment count exceeds limit");
            let options = reader.input.u8()?;
            let duration_ms = reader.input.u32()? as i32;
            let three_dimensional = if kind == 3 {
                options & 1 != 0
            } else {
                options != 0
            };
            let end = vector(&mut reader.input, three_dimensional)?;
            let mut controls = Vec::new();
            if kind == 3 {
                let count = reader.input.u16()? as usize;
                let stride = if three_dimensional { 12 } else { 8 };
                ensure!(
                    count <= reader.input.remaining() / stride,
                    "truncated motion control points"
                );
                controls.reserve(count);
                for _ in 0..count {
                    controls.push(vector(&mut reader.input, three_dimensional)?);
                }
            }
            let time_curve = if kind == 2 || options & 2 != 0 {
                reader.motion_time_curve()?
            } else {
                None
            };
            segments.push(MotionSegment {
                kind,
                options,
                duration_ms,
                end,
                controls,
                time_curve,
            });
        }
        Ok(Self {
            start,
            segments,
            timing_mode: reader.input.u32()? as i32,
            duration_ms: reader.input.u32()? as i32,
            rate: finite_float(&mut reader.input)?,
            dimensions: reader.input.u8()?,
            flags: reader.input.u16()?,
        })
    }
}

/// Inspection of a complete RIO resource extent. Some retail references include
/// an opaque trailer beyond rvmm's serialized motion body. Retain it explicitly;
/// this is not proof that those extra bytes can be discarded during execution.
#[derive(Debug, Clone, Serialize)]
pub struct MotionResource {
    pub curve: MotionCurve,
    pub trailing_data: Vec<u8>,
}
impl MotionResource {
    pub fn parse(data: &[u8], class: &ClassId) -> Result<Self> {
        let mut reader = ObjectReader::new(data);
        reader.input.skip(3)?;
        let curve = MotionCurve::read_body(&mut reader, class)?;
        let trailing_data = reader.input.bytes(reader.input.remaining())?.to_vec();
        Ok(Self {
            curve,
            trailing_data,
        })
    }
}

fn validate_class(class: &ClassId) -> Result<()> {
    ensure!(
        class.name == "CMoveAcsOngen" && class.schema == 3,
        "unsupported motion {}#{}",
        class.name,
        class.schema
    );
    Ok(())
}
fn vector(input: &mut Reader<'_>, three_dimensional: bool) -> Result<[i32; 3]> {
    let x = input.u32()? as i32;
    // Native 10005246/10005e7b: compact vectors are X/Z, not X/Y.
    let y = if three_dimensional {
        input.u32()? as i32
    } else {
        0
    };
    let z = input.u32()? as i32;
    Ok([x, y, z])
}
fn finite_float(input: &mut Reader<'_>) -> Result<f32> {
    let value = f32::from_bits(input.u32()?);
    ensure!(value.is_finite(), "non-finite motion value");
    Ok(value)
}
