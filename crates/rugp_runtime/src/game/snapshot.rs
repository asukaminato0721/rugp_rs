//! Full runtime checkpoint. All components share one reference/texture graph.
//! Preparation is read-only; the final commit consists solely of infallible
//! swaps while holding the same exclusive Game borrow. No stale prepared state
//! is exposed, and no host operation or audio callback is replayed.
use super::*;
use crate::{
    audio::AudioResources,
    audio_state::snapshot::{PlaybackResume, PlaybackSnapshot},
    render::snapshot::{TextureSnapshotTable, TextureSnapshotWriter},
};
use rugp_assets::object::snapshot::{ReferenceSnapshot, ReferenceSnapshotWriter};
use rugp_scene_vm::vm::snapshot::VmSnapshot;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::HashSet;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameSnapshot {
    version: u32,
    project: String,
    vm: VmSnapshot,
    scene: scene_snapshot::SceneSnapshot,
    actors: actor_snapshot::ActorSnapshot,
    ui: ui_snapshot::UiSnapshot,
    audio: PlaybackSnapshot,
    audio_wait: Option<Vec<u64>>,
    references: ReferenceSnapshot,
    textures: TextureSnapshotTable,
}
impl Game {
    /// Project/layout identity, not authentication of every byte of retail data.
    /// The VM independently checks its complete immutable program fingerprint.
    pub(super) fn project_key(&self) -> String {
        let mut digest = Sha1::new();
        digest.update(self.archive.manifest.title.as_bytes());
        digest.update(&self.archive.project_data);
        for volume in &self.archive.volumes {
            digest.update(volume.size.to_le_bytes());
        }
        format!("{:x}", digest.finalize())
    }
    /// Capture a playable checkpoint only when the script allows saving.
    pub fn snapshot(&self) -> Result<GameSnapshot> {
        ensure!(
            !self.load_requested,
            "cannot save while browsing for a load"
        );
        ensure!(self.script_save_enabled, "saving is disabled by the script");
        ensure!(!self.ended, "cannot save an ended game");
        self.capture_snapshot()
    }
    /// Reconstruct output at the unchanged virtual epoch (for a paused desktop
    /// modal). Like loading, this replaces tickets and discards already-applied
    /// requests. Reset the device and install these resumes before ticking.
    pub fn rebuild_audio_output(
        &mut self,
        resources: &mut AudioResources,
    ) -> Result<Vec<PlaybackResume>> {
        ensure!(
            resources.uses_archive(&self.archive),
            "audio resources belong to another archive"
        );
        let mut references = ReferenceSnapshotWriter::default();
        let saved = self.audio_state.snapshot(&mut references)?;
        let roots = references.finish()?.restore()?;
        let mut audio = saved.restore(&roots, &self.audio_state)?;
        let wait = self
            .audio_wait
            .as_deref()
            .map(|w| audio.remap_wait(w))
            .transpose()?;
        audio.prepare_muted(resources)?;
        self.audio_state = audio.registry;
        self.audio_wait = wait;
        self.sounds.clear();
        Ok(audio.playbacks)
    }
    /// Also used by diagnostics and native checkpoint conversion. Loading an
    /// existing checkpoint does not require permission to create a new save.
    pub(super) fn capture_snapshot(&self) -> Result<GameSnapshot> {
        let mut references = ReferenceSnapshotWriter::default();
        let mut textures = TextureSnapshotWriter::default();
        let snapshot = GameSnapshot {
            version: 1,
            project: self.project_key(),
            vm: self.vm.snapshot(&mut references)?,
            scene: self.snapshot_scene(&mut references, &mut textures)?,
            actors: self.snapshot_actors(&mut references)?,
            ui: self.snapshot_ui(&mut references)?,
            audio: self.audio_state.snapshot(&mut references)?,
            audio_wait: self.audio_wait.clone(),
            references: references.finish()?,
            textures: textures.finish()?,
        };
        snapshot.validate_waits(&self.vm)?;
        Ok(snapshot)
    }
    /// Atomically replace runtime state, leaving global CStdb progress and host
    /// font/output preferences intact. The caller MUST reset old device output
    /// and rebuild its ticket mapping from the returned resumes before the next
    /// tick. Muted hosts need no output: all PCM deadlines are prepared here.
    /// On any validation/resource error, the current game is unchanged.
    pub fn restore_snapshot(
        &mut self,
        snapshot: &GameSnapshot,
        resources: &mut AudioResources,
    ) -> Result<Vec<PlaybackResume>> {
        ensure!(
            self.rasterize_scene,
            "cannot restore with scene rasterization disabled"
        );
        ensure!(snapshot.version == 1, "unsupported game snapshot version");
        ensure!(
            snapshot.project == self.project_key(),
            "save belongs to a different project"
        );
        ensure!(
            resources.uses_archive(&self.archive),
            "restore audio resources belong to another archive"
        );
        let (vm, references) = self
            .vm
            .restore_snapshot(&snapshot.vm, &snapshot.references)?;
        snapshot.validate_waits(&vm)?;
        let textures = snapshot.textures.restore()?;
        let scene = snapshot.scene.restore(self, &references, &textures)?;
        let actors = snapshot.actors.restore(self, &references, &scene, &vm)?;
        let ui = snapshot.ui.restore(self, &references, &scene, &vm)?;
        let mut audio = snapshot.audio.restore(&references, &self.audio_state)?;
        let audio_wait = snapshot
            .audio_wait
            .as_deref()
            .map(|w| audio.remap_wait(w))
            .transpose()?;
        // Decode now, while failure cannot touch the live registry/VM. Expired
        // one-shots stay finished; restored loops retain their original phase.
        audio.prepare_muted(resources)?;
        scene.commit(self);
        actors.commit(self);
        ui.commit(self);
        self.vm = vm;
        self.audio_state = audio.registry;
        self.audio_wait = audio_wait;
        // The registry already contains every queued play/gain/stop effect.
        // Replaying the old output queue would duplicate those effects.
        self.sounds.clear();
        self.load_requested = false;
        Ok(audio.playbacks)
    }
}
impl GameSnapshot {
    fn validate_waits(&self, vm: &Vm) -> Result<()> {
        let count = self.scene.wait_count()
            + self.actors.wait_count()
            + self.ui.wait_count()
            + usize::from(self.audio_wait.is_some());
        ensure!(count <= 1, "overlapping game snapshot waits");
        ensure!(
            !self.ui.ended() || count == 0,
            "ended game snapshot has an active wait"
        );
        if !self.ui.ended() {
            ensure!(
                vm.cursor().is_some(),
                "active game snapshot has a halted VM"
            );
            ensure!(
                (count != 0) == vm.pending_instruction().is_some(),
                "game snapshot wait and suspended VM differ"
            );
        }
        self.scene.validate_vm(vm)?;
        if let Some(wait) = &self.audio_wait {
            let mut tickets = HashSet::new();
            ensure!(
                !wait.is_empty()
                    && wait.len() <= 4096
                    && wait.iter().all(|t| *t > 0 && tickets.insert(*t)),
                "invalid audio wait ticket set"
            );
            ensure!(
                vm.pending_instruction()
                    .is_some_and(|i| matches!(i.class.name.as_str(), "CVmSound" | "CVmSync")),
                "audio wait has a different pending VM instruction"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wire_roundtrip(game: &Game) -> Result<GameSnapshot> {
        let saved = game.capture_snapshot()?;
        let mut bytes = Vec::new();
        saved.write(&mut bytes)?;
        GameSnapshot::read(bytes.as_slice())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_full_snapshot_restores_startup_movie_title_and_voiced_dialogue_into_a_fresh_game()
    -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let mut fresh = Game::open(root.as_ref())?;
        let mut resources = AudioResources::new(Arc::clone(&fresh.archive))?;
        let mut audio = None;
        let mut checks = 0;
        let mut title = false;
        for tick in 0..300 {
            game.step(1000)?;
            crate::audio::mute_requests(&mut game, &mut audio)?;
            let check = tick == 0
                || (game.video.is_some() && checks == 1)
                || (game.waiting_for_selection() && !title)
                || game.waiting_for_dialogue();
            if check {
                let saved = wire_roundtrip(&game)?;
                let cursor = game.vm.cursor().cloned();
                fresh.restore_snapshot(&saved, &mut resources)?;
                assert_eq!(fresh.vm.cursor(), cursor.as_ref());
                assert!(game.frame() == fresh.frame());
                assert!(fresh.pending_sounds().is_empty());
                // Continuation must agree with an uninterrupted host, not just
                // with the frozen screenshot at the moment of loading.
                for dt in [0, 7, 33, 97] {
                    assert_eq!(game.step(dt)?, fresh.step(dt)?);
                    crate::audio::mute_requests(&mut game, &mut audio)?;
                    let mut output = None;
                    crate::audio::mute_requests(&mut fresh, &mut output)?;
                    assert_eq!(game.vm.cursor(), fresh.vm.cursor());
                    assert_eq!(game.waiting_for_dialogue(), fresh.waiting_for_dialogue());
                    assert!(
                        game.frame() == fresh.frame(),
                        "continued checkpoint pixels differ"
                    );
                }
                checks += 1;
                if game.waiting_for_dialogue() {
                    break;
                }
            }
            if game.waiting_for_selection() && !title {
                title = true;
                game.navigate_selection(1)?;
                game.activate_selection()?;
            }
        }
        ensure!(
            title && game.waiting_for_dialogue() && checks >= 4,
            "full runtime checkpoints not reached"
        );
        // A late validation failure must leave VM, pixels, audio and queued
        // output intact. Global progress and host preferences are not rewound.
        let mut saved = wire_roundtrip(&game)?;
        fresh.profiles.load(42, || {
            rugp_assets::profile::Profile::from_text("key={live}".into())
        })?;
        fresh.profiles.write_string(42, "key", "newer")?;
        fresh.navigation = "live marker".into();
        let frame = fresh.frame().to_vec();
        let cursor = fresh.vm.cursor().cloned();
        saved.version = 2;
        assert!(fresh.restore_snapshot(&saved, &mut resources).is_err());
        saved.version = 1;
        saved.project.push('x');
        assert!(fresh.restore_snapshot(&saved, &mut resources).is_err());
        saved.project.pop();
        let valid_audio_wait = saved.audio_wait.take();
        saved.audio_wait = Some(vec![1, 1]);
        assert!(fresh.restore_snapshot(&saved, &mut resources).is_err());
        saved.audio_wait = valid_audio_wait;
        let mut value = serde_json::to_value(&saved.audio)?;
        value["version"] = 2.into();
        let valid_audio = std::mem::replace(&mut saved.audio, serde_json::from_value(value)?);
        assert!(fresh.restore_snapshot(&saved, &mut resources).is_err());
        saved.audio = valid_audio;
        assert!(fresh.frame() == frame);
        assert_eq!(fresh.vm.cursor(), cursor.as_ref());
        assert_eq!(fresh.navigation, "live marker");
        fresh.restore_snapshot(&saved, &mut resources)?;
        assert_eq!(
            fresh
                .profiles
                .load(42, || bail!("profile cache lost"))?
                .read_string("key", ""),
            "newer"
        );
        let directory = std::env::temp_dir().join(format!(
            "rugp-slots-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fresh.profiles = crate::profile_store::ProfileStore::persistent(&directory)?;
        ensure!(
            fresh.script_save_enabled(),
            "real dialogue unexpectedly disables saving"
        );
        fresh.save_slot(0)?;
        assert_eq!(fresh.save_slots()?.len(), 1);
        let slot = directory.join("slot-0.rugpsave");
        let valid = std::fs::read(&slot)?;
        let saved_frame = fresh.frame().to_vec();
        let saved_cursor = fresh.vm.cursor().cloned();
        fresh.script_save_enabled = false;
        assert!(fresh.save_slot(0).is_err());
        assert!(std::fs::read(&slot)? == valid);
        fresh.script_save_enabled = true;
        fresh.activate_selection()?;
        fresh.step(100)?;
        fresh.load_slot(0, &mut resources)?;
        assert!(fresh.frame() == saved_frame);
        assert_eq!(fresh.vm.cursor(), saved_cursor.as_ref());
        std::fs::write(&slot, b"invalid portable save")?;
        assert!(fresh.load_slot(0, &mut resources).is_err());
        assert!(fresh.frame() == saved_frame);
        assert_eq!(fresh.vm.cursor(), saved_cursor.as_ref());
        drop(fresh);
        std::fs::remove_file(slot)?;
        std::fs::remove_file(directory.join("profiles.lock"))?;
        std::fs::remove_dir(directory)?;
        game.script_save_enabled = false;
        assert!(game.snapshot().is_err());
        game.script_save_enabled = true;
        game.ended = true;
        assert!(game.snapshot().is_err());
        Ok(())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_full_snapshot_remaps_waits_discards_queued_audio_and_excludes_old_completions()
    -> Result<()> {
        use rugp_assets::object::ClassId;
        use rugp_scene_vm::{Argument, Scenario};
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let mut resources = AudioResources::new(Arc::clone(&game.archive))?;
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
        let duration = resources
            .muted_duration(&source)?
            .context("one-shot duration missing")?;
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 48,
                null_records: vec![],
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
                            Argument::Number(2),
                            Argument::Number(1),
                            Argument::Number(0),
                            Argument::Number(0),
                            Argument::Number(0),
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
                        arguments: vec![],
                    },
                ],
            },
        )?;
        let program = Arc::new(program);
        for expired in [false, true] {
            game.vm = Vm::new(Arc::clone(&program), 1)?;
            game.ended = false;
            assert_eq!(game.step(0)?, RunState::Waiting);
            let original = game.pending_sounds().last().unwrap().ticket.unwrap();
            let elapsed = if expired { duration + 10 } else { duration / 3 };
            game.step(elapsed)?;
            let saved = wire_roundtrip(&game)?;
            game.queue_sound(SoundRequest {
                ticket: None,
                source: Some(source.clone()),
                kind: 2,
                flags: 0,
                stereo: Default::default(),
                parameters: [0; 3],
            })?;
            let future = game.pending_sounds().last().unwrap().ticket.unwrap();
            game.restore_snapshot(&saved, &mut resources)?;
            let restored_ticket = game.audio_wait.as_ref().unwrap()[0];
            assert!(restored_ticket > future && future > original);
            let resumes = game.rebuild_audio_output(&mut resources)?;
            let ticket = game.audio_wait.as_ref().unwrap()[0];
            assert!(expired || ticket > restored_ticket);
            assert!(game.pending_sounds().is_empty());
            game.finish_audio_ticket(original);
            game.finish_audio_ticket(future);
            if !expired {
                game.finish_audio_ticket(restored_ticket);
            }
            if expired {
                assert!(resumes.is_empty());
                assert_eq!(game.step(0)?, RunState::Ended);
            } else {
                assert_eq!(resumes.len(), 1);
                assert_eq!(resumes[0].elapsed_ms, u64::from(elapsed));
                assert_eq!(game.step(duration - elapsed - 1)?, RunState::Waiting);
                assert_eq!(game.step(1)?, RunState::Ended);
            }
        }
        Ok(())
    }
}
