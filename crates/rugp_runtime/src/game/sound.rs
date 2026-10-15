use super::*;
use crate::audio_envelope::GainEnvelope;
use crate::audio_state::Bus;

impl SoundRequest {
    /// Null-source native operations address channel groups, not PCM sources.
    /// `None` denotes every bus; zero stops the native 3..11 effect group.
    pub fn global_stop_bus(&self) -> Result<Option<Bus>> {
        ensure!(self.source.is_none(), "global sound command has a source");
        match self.kind {
            0 | 254 => Ok(Some(Bus::Effect)),
            1 => Ok(Some(Bus::Bgm)),
            255 => Ok(None),
            other => bail!("unsupported global sound operation {other}"),
        }
    }
    pub fn gain_envelope(&self) -> Result<GainEnvelope> {
        // The native execution path does not inspect bit 1. Bit 0 waits for
        // playback; bit 2 enables a different, extended acoustic packet.
        ensure!(
            self.flags & !3 == 0,
            "unsupported sound play flags {:#x}",
            self.flags
        );
        match self.kind {
            2 => Ok(GainEnvelope::UNITY),
            4 => GainEnvelope::from_native(self.parameters),
            other => bail!("sound operation {other} is not playback"),
        }
    }
}

impl Game {
    pub(super) fn queue_sound(&mut self, mut request: SoundRequest) -> Result<bool> {
        if let Some(source) = &request.source {
            let id = source.offset.context("sound source identity missing")?;
            match request.kind {
                2 | 4 => {
                    request.gain_envelope()?;
                    let bus = match source.class.as_ref().map(|c| c.name.as_str()) {
                        Some("CPmBgm") => Bus::Bgm,
                        Some("COptimizedObs") => Bus::Voice,
                        _ => Bus::Effect,
                    };
                    request.ticket = Some(self.audio_state.play(id, bus)?);
                }
                3 => {
                    self.audio_state.stop_source(id);
                }
                5 => {
                    // Native CVmSound dispatch 100738c3 fades the existing
                    // instance; it does not replay the sample. Playback-first:
                    // use the device's current amplitude rather than resetting
                    // to the packet's initial gain. Bit 0 is invalid natively.
                    ensure!(
                        request.flags & 1 == 0,
                        "cannot wait directly on sound fade-stop"
                    );
                    self.audio_state
                        .fade_stop_source(id, u32::from(request.parameters[2]));
                }
                other => bail!("unsupported sound operation {other}"),
            }
        } else {
            match request.global_stop_bus()? {
                Some(bus) => self.audio_state.stop_bus(bus),
                None => self.audio_state.stop_all(),
            }
        }
        let waiting = request.ticket.is_some() && request.flags & 1 != 0;
        if waiting {
            self.audio_wait = Some(vec![request.ticket.unwrap()]);
        }
        self.sounds.push(request);
        Ok(waiting)
    }
    pub(super) fn synchronize_sound(
        &mut self,
        source: &ObjectRef,
        mode: i32,
        timeout: i32,
    ) -> Result<()> {
        ensure!(
            [16, 32].contains(&mode) && timeout == 0,
            "unsupported sound synchronization mode/timeout {mode}/{timeout}"
        );
        let tickets = self.audio_state.source_tickets(
            source
                .offset
                .context("sound synchronization identity missing")?,
        );
        // Vm60 0x10072ba0: 0x20 queries the media event and writes process
        // status. Only the other branch (0x10) enters the native wait loop.
        if mode == 32 {
            return self.vm.acknowledge_sync_query(tickets.is_empty());
        }
        if tickets.is_empty() {
            self.vm.acknowledge_host()?;
        } else {
            self.audio_wait = Some(tickets);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn global_channel_commands_share_device_and_muted_group_mapping() -> Result<()> {
        let mut request = SoundRequest {
            ticket: None,
            source: None,
            kind: 0,
            flags: 0,
            parameters: [4096, 0, 1000],
        };
        for (kind, bus) in [
            (0, Some(Bus::Effect)),
            (1, Some(Bus::Bgm)),
            (254, Some(Bus::Effect)),
            (255, None),
        ] {
            request.kind = kind;
            assert_eq!(request.global_stop_bus()?, bus);
        }
        request.kind = 2;
        assert!(request.global_stop_bus().is_err());
        Ok(())
    }
    #[test]
    fn native_play_modes_validate_envelopes_and_do_not_reinterpret_flags() -> Result<()> {
        let mut request = SoundRequest {
            ticket: None,
            source: None,
            kind: 4,
            flags: 2,
            parameters: [1024, 2048, 1000],
        };
        assert_eq!(request.gain_envelope()?.control_at_ms(500), 1536);
        request.flags = 3; // wait bit + native unused bit
        assert!(request.gain_envelope().is_ok());
        request.flags = 4; // extended acoustic packet needs a different path
        assert!(request.gain_envelope().is_err());
        request.flags = 0;
        request.kind = 2;
        request.parameters = [65535; 3]; // ordinary play ignores the three words
        assert_eq!(request.gain_envelope()?, GainEnvelope::UNITY);
        request.kind = 3;
        assert!(request.gain_envelope().is_err());
        Ok(())
    }
}
