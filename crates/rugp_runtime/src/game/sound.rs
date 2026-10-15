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
        // Bit 0 waits; native placement flags are resolved by the VM host.
        // Placement is independent of gain motion and the native unused bits.
        self.stereo.validate()?;
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
                    request.ticket = Some(self.audio_state.play_sound_spatial(
                        source,
                        request.gain_envelope()?,
                        request.stereo,
                    )?);
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
                    self.audio_state
                        .set_source_gain(id, request.gain_envelope()?);
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
    fn retail_literal_inline_spatial_packets_produce_bounded_channel_gains() -> Result<()> {
        use rugp_scene_vm::Argument;
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let archive = RioArchive::open(Path::new(&root))?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let program = Program::load(&archive, &native)?;
        let mut counts = BTreeMap::new();
        let mut references = 0;
        for scene in program.scenes.values() {
            for i in &scene.instructions {
                if i.class.name != "CVmSound" {
                    continue;
                }
                let (Argument::Number(kind), Argument::Number(flags)) =
                    (&i.arguments[1], &i.arguments[2])
                else {
                    bail!("nonliteral sound controls");
                };
                if !matches!(kind, 2 | 4) || flags & 4 == 0 {
                    continue;
                }
                let Argument::Reference(source) = &i.arguments[0] else {
                    continue;
                };
                if !source.class.as_ref().is_some_and(|c| c.name == "CPmEffect") {
                    continue;
                }
                if flags & 8 != 0 {
                    references += 1;
                    continue;
                }
                let Argument::Bytes(bytes) = &i.arguments[6] else {
                    bail!("inline acoustic bytes missing");
                };
                crate::audio_spatial::StereoGains::from_position(*flags as u8, bytes)?
                    .validate()?;
                *counts.entry((*flags, bytes.len())).or_insert(0usize) += 1;
            }
        }
        ensure!(
            !counts.is_empty(),
            "no original inline spatial packets checked"
        );
        eprintln!(
            "inline spatial packets {counts:?}; separate referenced-motion packets={references}"
        );
        Ok(())
    }

    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_spatial_sound_wait_and_full_restore_keep_channel_gains_and_fresh_tickets()
    -> Result<()> {
        use rugp_assets::{object::ClassId, script::Script};
        use rugp_scene_vm::{Argument, Scenario};
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let code = Scenario::decode_with_schema(
            &Script::load(&game.archive, 202)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let mut play = code
            .instructions
            .iter()
            .find(|i| i.address == 652)
            .context("original spatial play missing")?
            .clone();
        let Argument::Reference(source) = play.arguments[0].clone() else {
            bail!("spatial source missing");
        };
        play.address = 0;
        play.next_address = 24;
        let mut program = Program::default();
        program.insert(
            1,
            Scenario {
                object_id: 1,
                memory_size: 72,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    play,
                    Instruction {
                        index: 1,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmSync".into(),
                            schema: 17,
                        },
                        address: 24,
                        next_address: 48,
                        flags: 0,
                        arguments: vec![
                            Argument::Reference(source.clone()),
                            Argument::Number(16),
                            Argument::Number(0),
                        ],
                    },
                    Instruction {
                        index: 2,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmRet".into(),
                            schema: 17,
                        },
                        address: 48,
                        next_address: u32::MAX,
                        flags: 0,
                        arguments: vec![],
                    },
                ],
            },
        )?;
        game.vm = Vm::new(Arc::new(program), 1)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert_eq!(game.vm.cursor().unwrap().address, 24);
        let request = &game.pending_sounds()[0];
        assert_eq!(request.flags, 0x14);
        assert!(request.stereo.left > request.stereo.right && request.stereo.right > 0.0);
        let stereo = request.stereo;
        let original = request.ticket.context("spatial ticket missing")?;
        let mut audio = None;
        crate::audio::mute_requests(&mut game, &mut audio)?;
        let saved = game.snapshot()?;
        let mut wire = Vec::new();
        saved.write(&mut wire)?;
        let saved = super::super::snapshot::GameSnapshot::read(wire.as_slice())?;
        let resumes = game.restore_snapshot(&saved, audio.as_mut().unwrap())?;
        let restored = game.audio_wait.as_ref().unwrap()[0];
        assert!(restored > original);
        assert_eq!(resumes[0].stereo, stereo);
        game.finish_audio_ticket(original);
        assert_eq!(game.step(0)?, RunState::Waiting);
        game.queue_sound(SoundRequest {
            ticket: None,
            source: Some(source),
            kind: 3,
            flags: 0,
            stereo: Default::default(),
            parameters: [0; 3],
        })?;
        assert_eq!(game.step(0)?, RunState::Ended);
        Ok(())
    }

    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_audio_and_vm_snapshots_resume_a_real_pcm_wait_without_replay_or_stale_completion()
    -> Result<()> {
        use crate::{
            audio::{AudioResources, mute_requests},
            audio_state::snapshot::PlaybackSnapshot,
        };
        use rugp_assets::object::{
            ClassId, ObjectRef,
            snapshot::{ReferenceSnapshot, ReferenceSnapshotWriter},
        };
        use rugp_scene_vm::{Argument, Scenario, vm::snapshot::VmSnapshot};
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let source = ObjectRef {
            class: Some(ClassId {
                name: "CPmEffect".into(),
                schema: 0,
            }),
            cache_index: None,
            flags: 0xc108,
            offset: Some(251819724),
            size: Some(60),
            owner: None,
            selector: 0,
            inline: None,
        };
        for already_expired in [false, true] {
            let mut game = Game::open(root.as_ref())?;
            let mut resources = AudioResources::new(Arc::clone(&game.archive))?;
            let duration = resources
                .muted_duration(&source)?
                .context("expected nonlooping sound")?;
            ensure!(
                duration > 3,
                "retail PCM too short for resume boundary test"
            );
            let elapsed = if already_expired {
                duration + 10
            } else {
                duration / 3
            };
            let mut program = Program::default();
            program.insert(
                10,
                Scenario {
                    object_id: 1,
                    memory_size: 48,
                    null_records: Vec::new(),
                    trailing_bytes: 0,
                    instructions: vec![
                        Instruction {
                            index: 0,
                            stream_offset: 0,
                            class: ClassId {
                                name: "CVmSound".into(),
                                schema: 17,
                            },
                            address: 0,
                            next_address: 24,
                            flags: 0,
                            arguments: vec![
                                Argument::Reference(source.clone()),
                                Argument::Number(4),
                                Argument::Number(1),
                                Argument::Number(0),
                                Argument::Number(4096),
                                Argument::Number(100),
                            ],
                        },
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
                            arguments: Vec::new(),
                        },
                    ],
                },
            )?;
            let program = Arc::new(program);
            game.vm = Vm::new(Arc::clone(&program), 1)?;
            assert_eq!(game.step(0)?, RunState::Waiting);
            let original_ticket = game.pending_sounds()[0].ticket.context("play ticket")?;
            // Device-owned playback has no virtual PCM deadline until output
            // finishes. Draining must not discard source/gain information.
            assert_eq!(game.take_sounds().len(), 1);
            assert_eq!(game.step(elapsed)?, RunState::Waiting);
            let mut writer = ReferenceSnapshotWriter::default();
            let vm = game.vm.snapshot(&mut writer)?;
            let audio = game.audio_state.snapshot(&mut writer)?;
            let graph = writer.finish()?;
            let wait = game.audio_wait.clone();
            let wire = serde_json::to_vec(&(vm, audio, graph, wait))?;
            let (vm, audio, graph, wait): (
                VmSnapshot,
                PlaybackSnapshot,
                ReferenceSnapshot,
                Option<Vec<u64>>,
            ) = serde_json::from_slice(&wire)?;
            game.queue_sound(SoundRequest {
                ticket: None,
                source: Some(source.clone()),
                kind: 2,
                flags: 0,
                stereo: Default::default(),
                parameters: [0; 3],
            })?;
            let future_ticket = game.pending_sounds()[0]
                .ticket
                .context("future play ticket")?;
            let (vm, roots) = vm.restore(program, &graph)?;
            let mut audio = audio.restore(&roots, &game.audio_state)?;
            let wait = audio.remap_wait(wait.as_deref().context("saved wait")?)?;
            assert!(wait[0] > future_ticket);
            audio.prepare_muted(&mut resources)?;
            assert_eq!(audio.playbacks.len(), usize::from(!already_expired));
            if !already_expired {
                assert_eq!(audio.playbacks[0].elapsed_ms, u64::from(elapsed));
            }
            // Candidate states are complete before touching the live game.
            game.vm = vm;
            game.audio_state = audio.registry;
            game.audio_wait = Some(wait.clone());
            game.sounds.clear(); // reset/rebuild output, never redrain old commands
            game.finish_audio_ticket(original_ticket);
            game.finish_audio_ticket(future_ticket);
            if !already_expired {
                assert!(!game.audio_state.finished(wait[0]));
                assert_eq!(game.step(duration - elapsed - 1)?, RunState::Waiting);
                assert_eq!(game.vm.cursor().map(|c| c.address), Some(0));
                assert_eq!(game.step(1)?, RunState::Ended);
            } else {
                assert!(game.audio_state.finished(wait[0]));
                assert_eq!(game.step(0)?, RunState::Ended);
            }
            let mut resources = Some(resources);
            mute_requests(&mut game, &mut resources)?;
            assert!(game.pending_sounds().is_empty() && game.audio_wait.is_none());
        }
        Ok(())
    }
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
                stereo: Default::default(),
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
            stereo: Default::default(),
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
            stereo: Default::default(),
            parameters: [1024, 2048, 1000],
        };
        assert_eq!(request.gain_envelope()?.control_at_ms(500), 1536);
        request.flags = 3; // wait bit + native unused bit
        assert!(request.gain_envelope().is_ok());
        request.flags = 0x14; // placement has already been resolved by the host
        assert!(request.gain_envelope().is_ok());
        request.stereo.left = f32::NAN;
        assert!(request.gain_envelope().is_err());
        request.stereo = Default::default();
        request.flags = 0;
        request.kind = 2;
        request.parameters = [65535; 3]; // ordinary play ignores the three words
        assert_eq!(request.gain_envelope()?, GainEnvelope::UNITY);
        request.kind = 3;
        assert!(request.gain_envelope().is_err());
        Ok(())
    }
}
