use super::*;
use crate::audio_envelope::GainEnvelope;
use crate::audio_state::Bus;

impl SoundRequest {
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
                    let bus = if source.class.as_ref().is_some_and(|c| c.name == "CPmBgm") {
                        Bus::Bgm
                    } else {
                        Bus::Effect
                    };
                    request.ticket = Some(self.audio_state.play(id, bus)?);
                }
                3 => {
                    self.audio_state.stop_source(id);
                }
                other => bail!("unsupported sound operation {other}"),
            }
        } else {
            match request.kind {
                255 => self.audio_state.stop_all(),
                254 => self.audio_state.stop_bus(Bus::Effect),
                other => bail!("unsupported global sound operation {other}"),
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
