//! Native sampled parameter curves. These are independent of CFade effects.
//!
//! Draw-ratio consumer evidence for the remaining runtime integration:
//! Vm60 100447e0 stores time/curve/mode and marks the request pending.
//! With a null curve it constructs this class and fills an ascending table
//! for low mode 0, descending for 1 (100448b6..10044909). An explicitly
//! supplied curve bypasses that default factory, including low mode 0x10.
//! 10042a8a..10042aa5 activates pending mode-0x100 curves with ParseScreen.
//! 10044faa..10044fc8 evaluates active curves using ParseScreen's Q16
//! progress. 10044750..100447d5 arithmetic-shifts the result by five,
//! clamps to [0, 255], and writes the object's draw opacity at +110.
//! This is separate from camera interpolation and per-plane hidden flags.
use crate::{binary::Reader, object::ClassId};
use anyhow::{Result, ensure};

/// CMimeFlat16LimitOne#1: seventeen signed samples over sixteen Q16 intervals.
/// "LimitOne" clamps the *input* to [0, 65536], not the sampled output.
/// rvmm 1001bb40 serializes 34 bytes; 1001bb70 evaluates the signed curve.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MimeCurve {
    pub samples: [i16; 17],
}
impl MimeCurve {
    pub fn parse(data: &[u8], class: &ClassId) -> Result<Self> {
        let mut input = Reader::new(data);
        input.skip(3)?;
        Self::read_complete(&mut input, class)
    }
    pub fn from_inline(data: &[u8], class: &ClassId) -> Result<Self> {
        Self::read_complete(&mut Reader::new(data), class)
    }
    fn read_complete(input: &mut Reader<'_>, class: &ClassId) -> Result<Self> {
        let curve = Self::read_body(input, class)?;
        ensure!(input.remaining() == 0, "trailing sampled curve bytes");
        Ok(curve)
    }
    pub(crate) fn read_body(input: &mut Reader<'_>, class: &ClassId) -> Result<Self> {
        ensure!(
            class.name == "CMimeFlat16LimitOne" && class.schema == 1,
            "unsupported sampled curve {}#{}",
            class.name,
            class.schema
        );
        let mut samples = [0; 17];
        for sample in &mut samples {
            *sample = input.u16()? as i16;
        }
        Ok(Self { samples })
    }
    /// Native null-curve defaults for draw-ratio modes 0 and 1. Values are
    /// Q5 alpha (8192 = unity); other consumers may interpret them differently.
    /// rvmm 1001bbd0/1001bbf0 fill the ascending/descending sample tables.
    pub fn alpha_ramp(descending: bool) -> Self {
        Self {
            samples: std::array::from_fn(|i| (if descending { 16 - i } else { i }) as i16 * 512),
        }
    }
    pub fn evaluate(&self, position_q16: i32) -> i32 {
        let x = position_q16.clamp(0, 65536) as usize;
        let index = x >> 12;
        let fraction = (x & 4095) as i32;
        let first = i32::from(self.samples[index]);
        if fraction == 0 {
            return first;
        }
        // The arithmetic shift matters for negative slopes: native rounds
        // toward -infinity, not toward zero as signed division would do.
        first + (((i32::from(self.samples[index + 1]) - first) * fraction) >> 12)
    }
}
