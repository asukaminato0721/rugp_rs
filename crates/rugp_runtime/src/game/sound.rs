use super::*;
use crate::audio_envelope::GainEnvelope;
use crate::audio_state::Bus;

impl SoundRequest {
    /// Vm60 10073467..100734e5 only dispatches the four global stop
    /// operations for null sources. All other kind bytes return immediately,
    /// without playback, gain validation, waiting or channel changes.
    pub fn null_source_is_noop(&self) -> bool {
        self.source.is_none() && !matches!(self.kind, 0 | 1 | 254 | 255)
    }
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
            4 | 6 => GainEnvelope::from_native(self.parameters),
            other => bail!("sound operation {other} is not playback"),
        }
    }
}

impl Game {
    pub(super) fn queue_sound(&mut self, mut request: SoundRequest) -> Result<bool> {
        if request.null_source_is_noop() {
            return Ok(false);
        }
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
                6 => {
                    ensure!(
                        request.flags & 1 == 0,
                        "cannot wait directly on sound gain motion"
                    );
                    request.gain_envelope()?;
                    // Adjust the existing playback; do not allocate a new
                    // ticket or change its PCM completion deadline.
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
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_null_sound_play_is_a_native_noop_not_a_global_stop_or_wait() -> Result<()> {
        use rugp_assets::{object::ClassId, script::Script};
        use rugp_scene_vm::Scenario;
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let scene = Scenario::decode_with_schema(
            &Script::load(&game.archive, 345)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let mut instruction = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x3318)
            .context("null-source sound packet")?
            .clone();
        assert!(matches!(
            game.vm.evaluate(&instruction.arguments[0])?,
            Value::Null
        ));
        assert_eq!(game.vm.evaluate(&instruction.arguments[1])?.integer()?, 2);
        let tickets = [
            game.audio_state.play(1, Bus::Bgm)?,
            game.audio_state.play(2, Bus::Effect)?,
            game.audio_state.play(3, Bus::Voice)?,
        ];
        let before = game.frame().to_vec();
        instruction.address = 0;
        instruction.next_address = 24;
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 40,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    instruction,
                    Instruction {
                        index: 1,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmRet".into(),
                            schema: 17,
                        },
                        address: 24,
                        next_address: u32::MAX,
                        flags: 0,
                        arguments: vec![],
                    },
                ],
            },
        )?;
        game.vm = Vm::new(Arc::new(program), 1)?;
        assert_eq!(game.step(0)?, RunState::Ended);
        assert!(game.pending_sounds().is_empty() && game.audio_wait.is_none());
        for kind in 0..=255 {
            if matches!(kind, 0 | 1 | 254 | 255) {
                continue;
            }
            assert!(!game.queue_sound(SoundRequest {
                ticket: None,
                source: None,
                kind,
                flags: 255,
                parameters: [u16::MAX; 3],
            })?);
        }
        assert!(game.pending_sounds().is_empty() && game.audio_wait.is_none());
        assert!(tickets.iter().all(|t| !game.audio_state.finished(*t)));
        assert_eq!(game.frame(), before);
        Ok(())
    }

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
        for kind in 0..=255 {
            request.kind = kind;
            assert_eq!(
                request.null_source_is_noop(),
                !matches!(kind, 0 | 1 | 254 | 255)
            );
        }
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
