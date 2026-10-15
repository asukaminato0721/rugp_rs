//! CVmSound operation 4's amplitude envelope, not a decibel fade or play delay.
use anyhow::{Result, ensure};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GainEnvelope {
    start: u16,
    end: u16,
    duration_ms: u16,
}
impl GainEnvelope {
    pub const UNITY: Self = Self {
        start: 4096,
        end: 4096,
        duration_ms: 0,
    };
    pub fn from_native([start, end, duration_ms]: [u16; 3]) -> Result<Self> {
        // Native reads start/delta as signed words in the mixer. The retail
        // controls are nonnegative (0..4096); negative gains need separate
        // interpretation rather than being silently treated as large volumes.
        ensure!(
            start <= i16::MAX as u16 && end <= i16::MAX as u16,
            "negative native gain is not implemented"
        );
        Ok(Self {
            start,
            end,
            duration_ms,
        })
    }
    pub fn start_amplitude(self) -> f64 {
        f64::from(self.start) / 4096.0
    }
    pub fn end_amplitude(self) -> f64 {
        f64::from(self.end) / 4096.0
    }
    pub fn duration_ms(self) -> u16 {
        self.duration_ms
    }
    /// Reference native control value, before the legacy mixer's block-based
    /// sample interpolation. Vm60 0x1007ed84 uses MulDiv on Q12 values and an
    /// arithmetic shift. A zero-duration packet uses its start, not its end
    /// (0x1007d250). The output adapter performs the same linear amplitude
    /// motion on its own sample clock; device buffer/quantization may differ.
    pub fn control_at_ms(self, elapsed: u64) -> u16 {
        if self.duration_ms == 0 {
            return self.start;
        }
        if elapsed >= u64::from(self.duration_ms) {
            return self.end;
        }
        let delta = (i64::from(self.end) - i64::from(self.start)) * 4096;
        let product = delta * elapsed as i64;
        let divisor = i64::from(self.duration_ms);
        let rounded = product.signum() * ((product.abs() + divisor / 2) / divisor);
        ((i64::from(self.start) * 4096 + rounded) >> 12) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_gain_words_linear_amplitude_endpoints_and_zero_duration() -> Result<()> {
        let fade = GainEnvelope::from_native([0, 4096, 2500])?;
        assert_eq!(fade.control_at_ms(0), 0);
        assert_eq!(fade.control_at_ms(625), 1024);
        assert_eq!(fade.control_at_ms(1250), 2048);
        assert_eq!(fade.control_at_ms(2500), 4096);
        assert_eq!(fade.control_at_ms(u64::MAX), 4096);
        assert_eq!(fade.start_amplitude(), 0.0);
        assert_eq!(fade.end_amplitude(), 1.0);
        assert_eq!(
            GainEnvelope::from_native([4096, 0, 3])?.control_at_ms(1),
            2730
        );
        assert_eq!(
            GainEnvelope::from_native([4096, 0, 3])?.control_at_ms(2),
            1365
        );
        assert_eq!(
            GainEnvelope::from_native([0, 4096, 0])?.control_at_ms(100),
            0
        );
        assert_eq!(
            GainEnvelope::from_native([1024, 4096, 0])?.control_at_ms(u64::MAX),
            1024
        );
        assert!(GainEnvelope::from_native([65535, 4096, 1000]).is_err());
        assert!(GainEnvelope::from_native([0, 32768, 1000]).is_err());
        Ok(())
    }
}
