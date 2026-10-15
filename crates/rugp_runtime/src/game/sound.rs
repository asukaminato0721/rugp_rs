use super::*;
use crate::audio_state::Bus;

impl Game {
    pub(super) fn queue_sound(&mut self, mut request: SoundRequest) -> Result<bool> {
        if let Some(source) = &request.source {
            let id = source.offset.context("sound source identity missing")?;
            match request.kind {
                2 => {
                    ensure!(
                        request.flags & !1 == 0,
                        "unsupported sound play flags {:#x}",
                        request.flags
                    );
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
