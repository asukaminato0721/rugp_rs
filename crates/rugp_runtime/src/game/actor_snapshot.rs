//! Independent actor clocks and suspended waits. Share the reference graph with
//! VM/scene snapshots so an async image event remains the same instance as the
//! object held by script variables. Restoration never ticks or acknowledges it.
use super::*;
use crate::{
    staffroll::{StaffRoll, snapshot::StaffRollSnapshot},
    video::{VideoPlayer, VideoSnapshot},
};
use rugp_assets::object::{InlineIdentity, snapshot::ReferenceSnapshotWriter};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorSnapshot {
    version: u32,
    wait_ms: Option<u32>,
    timers: Vec<(u64, timer::Timer)>,
    timer_wait: Option<u64>,
    images: Vec<SavedImage>,
    image_wait: Option<u32>,
    video: Option<(u64, VideoSnapshot)>,
    video_wait: bool,
    staffroll: Option<(u64, StaffRollSnapshot)>,
    staffroll_wait: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedImage {
    handle: u32,
    window: u64,
    elapsed: u32,
    total: u32,
    fade_in: u32,
    fade_out: u32,
    cancelled: bool,
}
impl SavedImage {
    fn finished(&self) -> bool {
        self.cancelled || self.elapsed >= self.total
    }
}
impl Game {
    pub fn snapshot_actors(
        &self,
        references: &mut ReferenceSnapshotWriter,
    ) -> Result<ActorSnapshot> {
        let image_wait = self
            .image_wait
            .as_ref()
            .map(|identity| {
                self.async_images
                    .iter()
                    .position(|a| a.identity() == *identity)
                    .map(|index| index as u32)
                    .context("waiting image event missing before capture")
            })
            .transpose()?;
        let images = self
            .async_images
            .iter()
            .map(|a| {
                validate_handle(&a.handle)?;
                Ok(SavedImage {
                    handle: references.insert(&a.handle)?,
                    window: a.window,
                    elapsed: a.elapsed,
                    total: a.total,
                    fade_in: a.fade_in,
                    fade_out: a.fade_out,
                    cancelled: a.cancelled,
                })
            })
            .collect::<Result<_>>()?;
        let snapshot = ActorSnapshot {
            version: 1,
            wait_ms: self.wait_ms,
            timers: self.timers.iter().map(|(id, t)| (*id, t.clone())).collect(),
            timer_wait: self.timer_wait,
            images,
            image_wait,
            video: self
                .video
                .as_ref()
                .map(|(id, player)| Ok::<_, anyhow::Error>((*id, player.snapshot(references)?)))
                .transpose()?,
            video_wait: self.video_wait,
            staffroll: self
                .staffroll
                .as_ref()
                .map(|(id, roll)| Ok::<_, anyhow::Error>((*id, roll.snapshot()?)))
                .transpose()?,
            staffroll_wait: self.staffroll_wait,
        };
        snapshot.validate()?;
        snapshot.validate_vm(&self.vm)?;
        Ok(snapshot)
    }
}
impl ActorSnapshot {
    pub(crate) fn wait_count(&self) -> usize {
        [
            self.wait_ms.is_some(),
            self.timer_wait.is_some(),
            self.image_wait.is_some(),
            self.video_wait,
            self.staffroll_wait,
        ]
        .into_iter()
        .filter(|wait| *wait)
        .count()
    }
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported actor snapshot version");
        ensure!(self.wait_count() <= 1, "overlapping actor snapshot waits");
        ensure!(
            self.timers.len() <= 4096 && self.images.len() <= 4096,
            "actor snapshot count exceeds limit"
        );
        let mut timers = HashSet::new();
        for (id, _) in &self.timers {
            ensure!(timers.insert(*id), "duplicate actor snapshot timer");
        }
        if let Some(id) = self.timer_wait {
            ensure!(
                timers.contains(&id),
                "waiting actor snapshot timer is missing"
            );
        }
        let mut roots = HashSet::new();
        let mut windows = HashSet::new();
        for image in &self.images {
            ensure!(
                roots.insert(image.handle),
                "reused image snapshot handle root"
            );
            ensure!(
                image.elapsed <= image.total
                    && image.total <= i32::MAX as u32
                    && image.fade_in <= i32::MAX as u32
                    && image.fade_out <= i32::MAX as u32,
                "invalid image snapshot clock"
            );
            if !image.finished() {
                ensure!(
                    windows.insert(image.window),
                    "overlapping active image snapshot windows"
                );
            }
        }
        if let Some(index) = self.image_wait {
            ensure!(
                (index as usize) < self.images.len(),
                "waiting actor snapshot image is missing"
            );
        }
        ensure!(
            !self.video_wait || self.video.is_some(),
            "waiting actor snapshot movie is missing"
        );
        ensure!(
            !self.staffroll_wait || self.staffroll.is_some(),
            "waiting actor snapshot staff roll is missing"
        );
        Ok(())
    }
    fn validate_vm(&self, vm: &Vm) -> Result<()> {
        if self.wait_count() == 0 {
            return Ok(());
        }
        let instruction = vm
            .pending_instruction()
            .context("actor snapshot wait has no suspended VM instruction")?;
        if self.wait_ms.is_none() {
            ensure!(
                instruction.class.name == "CVmSync",
                "actor snapshot synchronization has a different VM instruction"
            );
        }
        // Do not reevaluate a mutable wait operand. The saved event identity,
        // not a possibly replaced variable value, is what the host is awaiting.
        Ok(())
    }
    pub fn restore(
        &self,
        game: &Game,
        references: &[ObjectRef],
        scene: &scene_snapshot::RestoredScene,
        vm: &Vm,
    ) -> Result<RestoredActors> {
        self.validate()?;
        self.validate_vm(vm)?;
        ensure!(
            self.wait_count() + scene.wait_count() <= 1,
            "overlapping scene/actor snapshot waits"
        );
        let mut identities = HashSet::new();
        let images = self
            .images
            .iter()
            .map(|saved| {
                let handle = references
                    .get(saved.handle as usize)
                    .context("image snapshot handle root out of range")?;
                let identity = validate_handle(handle)?;
                ensure!(
                    identities.insert(identity),
                    "duplicate image snapshot event identity"
                );
                Ok(async_image::ImageFade {
                    handle: handle.clone(),
                    window: saved.window,
                    elapsed: saved.elapsed,
                    total: saved.total,
                    fade_in: saved.fade_in,
                    fade_out: saved.fade_out,
                    cancelled: saved.cancelled,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let image_wait = self
            .image_wait
            .map(|index| images[index as usize].identity());
        let video = self
            .video
            .as_ref()
            .map(|(id, video)| {
                Ok::<_, anyhow::Error>((*id, video.restore(Arc::clone(&game.archive), references)?))
            })
            .transpose()?;
        let staffroll = self
            .staffroll
            .as_ref()
            .map(|(id, roll)| Ok::<_, anyhow::Error>((*id, roll.restore()?)))
            .transpose()?;
        ensure!(
            video.as_ref().is_none_or(|(_, p)| p.finished())
                || staffroll.as_ref().is_none_or(|(_, p)| p.finished()),
            "concurrent active movie/staff-roll snapshot"
        );
        let restored = RestoredActors {
            size: scene.size(),
            wait_ms: self.wait_ms,
            timers: self.timers.iter().map(|(id, t)| (*id, t.clone())).collect(),
            timer_wait: self.timer_wait,
            images,
            image_wait,
            video,
            video_wait: self.video_wait,
            staffroll,
            staffroll_wait: self.staffroll_wait,
        };
        restored.validate_scene(|id| scene.object(id).is_some(), scene.size())?;
        Ok(restored)
    }
}
fn validate_handle(handle: &ObjectRef) -> Result<InlineIdentity> {
    ensure!(
        handle
            .class
            .as_ref()
            .is_some_and(|c| c.name == "CGenericUso" && c.schema == 1)
            && handle.flags == 0x40
            && handle.offset.is_none()
            && handle.size.is_none()
            && handle.owner.is_none()
            && handle.cache_index.is_none()
            && handle.selector == 0,
        "invalid image snapshot event reference"
    );
    let body = handle
        .inline
        .as_ref()
        .context("image snapshot event has no inline identity")?;
    ensure!(
        body.data.is_empty()
            && body.references.is_empty()
            && body.bitmap.is_none()
            && body.posture.is_none()
            && body.motion.is_none(),
        "invalid image snapshot event payload"
    );
    Ok(body.identity())
}
pub struct RestoredActors {
    size: (u32, u32),
    wait_ms: Option<u32>,
    timers: BTreeMap<u64, timer::Timer>,
    timer_wait: Option<u64>,
    images: Vec<async_image::ImageFade>,
    image_wait: Option<InlineIdentity>,
    video: Option<(u64, VideoPlayer)>,
    video_wait: bool,
    staffroll: Option<(u64, StaffRoll)>,
    staffroll_wait: bool,
}
impl RestoredActors {
    fn validate_scene(&self, has_object: impl Fn(u64) -> bool, size: (u32, u32)) -> Result<()> {
        ensure!(self.size == size, "actor snapshot framebuffer size differs");
        for image in self.images.iter().filter(|a| !a.finished()) {
            ensure!(
                has_object(image.window),
                "active image snapshot window is missing"
            );
        }
        if let Some((_, player)) = &self.video {
            ensure!(player.size() == size, "movie snapshot output size differs");
        }
        if let Some((_, roll)) = &self.staffroll {
            ensure!(
                roll.finished() || has_object(roll.window),
                "active staff-roll snapshot window is missing"
            );
        }
        Ok(())
    }
    /// Apply alongside the prepared VM and scene. Does not advance time, clear
    /// finished events, redraw a movie over the scene, or acknowledge a wait.
    pub fn apply(self, game: &mut Game) -> Result<()> {
        self.validate_scene(|id| game.objects.contains_key(&id), game.size())?;
        self.commit(game);
        Ok(())
    }
    pub(super) fn commit(self, game: &mut Game) {
        game.wait_ms = self.wait_ms;
        game.timers = self.timers;
        game.timer_wait = self.timer_wait;
        game.async_images = self.images;
        game.image_wait = self.image_wait;
        game.video = self.video;
        game.video_wait = self.video_wait;
        game.staffroll = self.staffroll;
        game.staffroll_wait = self.staffroll_wait;
    }
}

#[cfg(test)]
pub(super) fn test_roundtrip(game: &mut Game) -> Result<()> {
    use crate::render::snapshot::{TextureSnapshotTable, TextureSnapshotWriter};
    use rugp_assets::object::snapshot::ReferenceSnapshot;
    use rugp_scene_vm::vm::snapshot::VmSnapshot;
    let frame = game.frame().to_vec();
    let cursor = game.vm.cursor().cloned();
    let mut references = ReferenceSnapshotWriter::default();
    let mut textures = TextureSnapshotWriter::default();
    let vm = game.vm.snapshot(&mut references)?;
    let scene = game.snapshot_scene(&mut references, &mut textures)?;
    let actors = game.snapshot_actors(&mut references)?;
    let wire = serde_json::to_vec(&(vm, scene, actors, references.finish()?, textures.finish()?))?;
    let (vm, scene, actors, references, textures): (
        VmSnapshot,
        scene_snapshot::SceneSnapshot,
        ActorSnapshot,
        ReferenceSnapshot,
        TextureSnapshotTable,
    ) = serde_json::from_slice(&wire)?;
    let (vm, references) = game.vm.restore_snapshot(&vm, &references)?;
    let textures = textures.restore()?;
    let scene = scene.restore(game, &references, &textures)?;
    let actors = actors.restore(game, &references, &scene, &vm)?;
    // No clock or host callback runs between preparing the candidates and this
    // data swap. Other components are intentionally unchanged at this instant.
    scene.apply(game)?;
    actors.apply(game)?;
    game.vm = vm;
    ensure!(
        game.frame() == frame,
        "actor restore changed current pixels"
    );
    ensure!(
        game.vm.cursor() == cursor.as_ref(),
        "actor restore acknowledged a VM instruction"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugp_assets::object::ClassId;
    fn bare() -> ActorSnapshot {
        ActorSnapshot {
            version: 1,
            wait_ms: None,
            timers: vec![],
            timer_wait: None,
            images: vec![],
            image_wait: None,
            video: None,
            video_wait: false,
            staffroll: None,
            staffroll_wait: false,
        }
    }
    fn image(handle: u32, window: u64) -> SavedImage {
        SavedImage {
            handle,
            window,
            elapsed: 0,
            total: 1000,
            fade_in: 100,
            fade_out: 100,
            cancelled: false,
        }
    }
    #[test]
    fn timer_epochs_including_unsigned_wrap_and_dormant_thresholds_are_not_retimed() -> Result<()> {
        let timer = |enabled, elapsed, interval| {
            serde_json::from_value::<timer::Timer>(
                serde_json::json!({"enabled":enabled,"elapsed":elapsed,"interval":interval}),
            )
        };
        let mut saved = bare();
        saved.timers = vec![
            (1, timer(true, u32::MAX - 1, u32::MAX)?),
            (2, timer(true, 200, 100)?),
            (3, timer(false, 0x146e4ebcu32, 0)?),
        ];
        saved.timer_wait = Some(1);
        saved.validate()?;
        let wire = serde_json::to_vec(&saved)?;
        let restored: ActorSnapshot = serde_json::from_slice(&wire)?;
        assert!(serde_json::to_value(&restored.timers)? == serde_json::to_value(&saved.timers)?);
        assert_eq!(restored.timer_wait, Some(1));
        Ok(())
    }
    #[test]
    fn expired_and_cancelled_image_handles_remain_queryable_without_faking_elapsed_time()
    -> Result<()> {
        let mut saved = bare();
        let mut finished = image(0, 9);
        finished.elapsed = 1000;
        let mut cancelled = image(1, 9);
        cancelled.cancelled = true;
        saved.images = vec![finished, cancelled, image(2, 9)];
        saved.image_wait = Some(0);
        saved.validate()?;
        let restored: ActorSnapshot = serde_json::from_slice(&serde_json::to_vec(&saved)?)?;
        assert!(restored.images[0].finished() && restored.images[1].finished());
        assert!(!restored.images[2].finished());
        assert_eq!(restored.images[1].elapsed, 0);
        assert_eq!(restored.images[1].total, 1000);
        assert_eq!(restored.image_wait, Some(0));
        Ok(())
    }
    #[test]
    fn malformed_links_duplicate_handles_clocks_and_overlapping_waits_fail() -> Result<()> {
        let mut saved = bare();
        saved.version = 9;
        assert!(saved.validate().is_err());
        let mut saved = bare();
        saved.timer_wait = Some(1);
        assert!(saved.validate().is_err());
        let mut saved = bare();
        saved.image_wait = Some(0);
        assert!(saved.validate().is_err());
        let mut saved = bare();
        saved.video_wait = true;
        assert!(saved.validate().is_err());
        let mut saved = bare();
        saved.staffroll_wait = true;
        assert!(saved.validate().is_err());
        let mut saved = bare();
        saved.images = vec![image(0, 1), image(0, 2)];
        assert!(saved.validate().is_err());
        let mut saved = bare();
        saved.images = vec![image(0, 1), image(1, 1)];
        assert!(saved.validate().is_err());
        let mut saved = bare();
        let mut bad = image(0, 1);
        bad.elapsed = 1001;
        saved.images = vec![bad];
        assert!(saved.validate().is_err());
        let mut saved = bare();
        saved.images = vec![image(0, 1)];
        saved.image_wait = Some(0);
        saved.wait_ms = Some(1);
        assert!(saved.validate().is_err());
        Ok(())
    }
    #[test]
    fn event_handles_require_canonical_host_instances_not_archive_offsets_or_byte_equality()
    -> Result<()> {
        let handle = ObjectRef::transient(ClassId {
            name: "CGenericUso".into(),
            schema: 1,
        });
        let other = ObjectRef::transient(ClassId {
            name: "CGenericUso".into(),
            schema: 1,
        });
        assert_ne!(validate_handle(&handle)?, validate_handle(&other)?);
        assert_eq!(validate_handle(&handle)?, validate_handle(&handle.clone())?);
        let mut bad = handle.clone();
        bad.offset = Some(123);
        assert!(validate_handle(&bad).is_err());
        let mut bad = handle.clone();
        bad.class.as_mut().unwrap().schema = 2;
        assert!(validate_handle(&bad).is_err());
        let mut bad = handle.clone();
        bad.inline.as_mut().unwrap().data.push(1);
        assert!(validate_handle(&bad).is_err());
        let mut bad = handle;
        bad.owner = Some(Box::new(other));
        assert!(validate_handle(&bad).is_err());
        Ok(())
    }
    fn reference(name: &str, schema: u16, offset: u64) -> ObjectRef {
        ObjectRef {
            class: Some(ClassId {
                name: name.into(),
                schema,
            }),
            cache_index: None,
            flags: 0xc108,
            offset: Some(offset),
            size: Some(7),
            owner: None,
            selector: 0,
            inline: None,
        }
    }
    fn sync_vm(arguments: Vec<rugp_scene_vm::Argument>) -> Result<Vm> {
        use rugp_scene_vm::Scenario;
        let mut program = Program::default();
        program.insert(
            10,
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
                            name: "CVmSync".into(),
                            schema: 17,
                        },
                        address: 0,
                        next_address: 24,
                        flags: 0,
                        arguments,
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
        Vm::new(Arc::new(program), 1)
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_host_actor_roundtrip_keeps_original_event_after_operand_replacement_and_generic_wait_tail()
    -> Result<()> {
        use rugp_scene_vm::Argument;
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        game.frame = Framebuffer::new(4, 4)?;
        let output = reference("CVmVarObj", 1, 7);
        let window = reference("CImgBox", 15, 1);
        game.vm = sync_vm(vec![
            Argument::Reference(output.clone()),
            Argument::Number(16),
            Argument::Number(0),
        ])?;
        game.objects.insert(
            1,
            ObjectState {
                rect: (0, 0, 4, 4),
                layers: BTreeMap::from([(
                    0,
                    Layer::Screen(Arc::new(Texture {
                        width: 1,
                        height: 1,
                        display_scale: 1.0,
                        rgba: vec![200, 100, 50, 255],
                    })),
                )]),
                ..Default::default()
            },
        );
        let manager = ObjectRef::transient(ClassId {
            name: "CKiminozoEffectManager".into(),
            schema: 1,
        });
        game.fade_image(
            &manager,
            &BTreeMap::from([
                ("表示方式".into(), Value::Integer(0)),
                ("モード".into(), Value::Integer(1)),
                ("対象VRAMウインドウ".into(), Value::Reference(window)),
                ("総時間".into(), Value::Integer(1000)),
                ("非同期ハンドル".into(), Value::Reference(output)),
            ]),
        )?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert_eq!(game.step(200)?, RunState::Waiting);
        // Mutable operands are not a substitute for the event already held by
        // a suspended host operation. The old event remains the wait target.
        game.vm.variables.insert(7, Value::Null);
        let old = game.image_wait.clone().unwrap();
        test_roundtrip(&mut game)?;
        assert_ne!(game.image_wait.as_ref().unwrap(), &old);
        assert_eq!(
            game.image_wait.as_ref().unwrap(),
            &game.async_images[0].identity()
        );
        assert!(matches!(game.vm.variables[&7], Value::Null));
        assert_eq!(game.step(799)?, RunState::Waiting);
        assert_eq!(game.step(1)?, RunState::Ended);
        // Completed equal-byte handles are still distinct instances, and do
        // not require their old window to exist merely to answer a query.
        game.async_images.push(async_image::ImageFade {
            handle: ObjectRef::transient(ClassId {
                name: "CGenericUso".into(),
                schema: 1,
            }),
            window: 1,
            elapsed: 0,
            total: 1000,
            fade_in: 0,
            fade_out: 0,
            cancelled: true,
        });
        game.objects.remove(&1);
        test_roundtrip(&mut game)?;
        assert_ne!(
            game.async_images[0].identity(),
            game.async_images[1].identity()
        );
        assert!(game.async_images[1].cancelled);
        assert_eq!(game.async_images[1].elapsed, 0);
        for handle in game
            .async_images
            .iter()
            .map(|a| a.handle.clone())
            .collect::<Vec<_>>()
        {
            game.vm = sync_vm(vec![
                Argument::Reference(handle.clone()),
                Argument::Number(34),
                Argument::Number(0),
            ])?;
            assert!(matches!(game.vm.step()?, Step::Host { .. }));
            game.synchronize_image(&handle, 34, 0)?;
            assert_eq!(game.vm.synchronization_status, Some(false));
        }
        let null = ObjectRef {
            class: None,
            cache_index: Some(0),
            flags: 0,
            offset: None,
            size: None,
            owner: None,
            selector: 0,
            inline: None,
        };
        game.ended = false;
        game.vm = sync_vm(vec![
            Argument::Reference(null),
            Argument::Number(0),
            Argument::Number(1000),
        ])?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert_eq!(game.step(400)?, RunState::Waiting);
        test_roundtrip(&mut game)?;
        assert_eq!(game.step(599)?, RunState::Waiting);
        assert_eq!(game.step(1)?, RunState::Ended);
        Ok(())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_actor_movie_wait_survives_delta_frame_and_final_boundary_and_never_overwrites_title()
    -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let mut audio = None;
        game.step(0)?;
        for _ in 0..300 {
            crate::audio::mute_requests(&mut game, &mut audio)?;
            if game.video.is_some() {
                break;
            }
            game.step(100)?;
        }
        ensure!(
            game.video_wait,
            "startup did not reach its movie synchronization"
        );
        let cursor = game.vm.cursor().cloned();
        assert_eq!(game.step(67)?, RunState::Waiting);
        assert_eq!(game.video.as_ref().unwrap().1.frame_index(), 2);
        test_roundtrip(&mut game)?;
        assert_eq!(game.step(116966)?, RunState::Waiting);
        assert!(!game.video.as_ref().unwrap().1.finished());
        test_roundtrip(&mut game)?;
        game.step(1)?;
        assert!(game.video.as_ref().unwrap().1.finished() && !game.video_wait);
        assert_ne!(game.vm.cursor(), cursor.as_ref());
        game.step(1000)?;
        ensure!(
            game.waiting_for_selection(),
            "title not reached after restored movie"
        );
        let title = game.frame().to_vec();
        test_roundtrip(&mut game)?;
        game.step(0)?;
        ensure!(
            game.frame() == title,
            "completed movie overwrote restored title pixels"
        );
        Ok(())
    }
}
