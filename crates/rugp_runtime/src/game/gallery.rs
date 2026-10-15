//! Semantic gallery progress, distinct from Windows reference-stream text keys.
//! Portable saves do not import Windows profiles. Stable resource IDs bind the
//! same original selector packets to successfully used story images, without
//! depending on cache addresses, decoding/preloading, or rewinding global data.
use super::*;
use rugp_scene_vm::Argument;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ImageId {
    class: String,
    schema: u16,
    offset: u64,
    size: u32,
    selector: u32,
}
impl ImageId {
    fn of(reference: &ObjectRef) -> Result<Option<Self>> {
        let Some(class) = &reference.class else {
            ensure!(
                reference.offset.is_none() && reference.inline.is_none(),
                "malformed null gallery key"
            );
            return Ok(None);
        };
        ensure!(
            reference.inline.is_none() && reference.flags & 0x40 == 0,
            "inline gallery progress keys are unsupported"
        );
        ensure!(
            class.name.len() <= 128
                && !class.name.is_empty()
                && class
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "invalid gallery key class"
        );
        let offset = reference
            .offset
            .context("gallery key has no stored identity")?;
        let size = reference.size.context("gallery key has no stored extent")?;
        ensure!(size > 0, "gallery key has an empty extent");
        Ok(Some(Self {
            class: class.name.clone(),
            schema: class.schema,
            offset,
            size,
            selector: reference.selector,
        }))
    }
    fn key(&self) -> String {
        format!(
            "$rugp:seen:1:{}:{:04x}:{:016x}:{:08x}:{:08x}",
            self.class, self.schema, self.offset, self.size, self.selector
        )
    }
}
fn progress_key(value: &Value) -> Result<String> {
    let id = match value {
        Value::Null => None,
        Value::Reference(reference) => ImageId::of(reference)?,
        _ => bail!("gallery key is not an object reference"),
    };
    Ok(id.map_or_else(|| "$rugp:seen:1:null".into(), |id| id.key()))
}
#[derive(Default)]
pub(super) struct Gallery {
    bindings: BTreeMap<ImageId, BTreeMap<u64, ObjectRef>>,
}
impl Gallery {
    pub(super) fn from_program(program: &Program) -> Result<Self> {
        let mut result = Self::default();
        for code in program.scenes.values() {
            for instruction in &code.instructions {
                for argument in &instruction.arguments {
                    let Argument::Message { class, arguments } = argument else {
                        continue;
                    };
                    if class.name.trim_start_matches("&-") != "OM_SetSelecterByDatabase" {
                        continue;
                    }
                    let reference = |name: &str| {
                        arguments.iter().find_map(|arg| {
                            if let Argument::Field {
                                name: field,
                                argument,
                                ..
                            } = arg
                                && field == name
                                && let Argument::Reference(reference) = argument.as_ref()
                            {
                                Some(reference)
                            } else {
                                None
                            }
                        })
                    };
                    let Some(image) = reference("キー名となるオブジェクト") else {
                        continue;
                    };
                    let Some(id) = ImageId::of(image)? else {
                        continue;
                    };
                    let database =
                        reference("データベース").context("gallery database operand missing")?;
                    ensure!(
                        database
                            .class
                            .as_ref()
                            .is_some_and(|c| c.name == "CStdb" && c.schema == 1),
                        "unsupported gallery database class"
                    );
                    let db = database
                        .offset
                        .context("gallery database identity missing")?;
                    result
                        .bindings
                        .entry(id)
                        .or_default()
                        .insert(db, database.clone());
                    ensure!(
                        result.bindings.len() <= 4096,
                        "gallery image binding limit exceeded"
                    );
                }
            }
        }
        Ok(result)
    }
}
impl Game {
    pub(super) fn configure_gallery_selector(
        &mut self,
        target: &ObjectRef,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let Some(Value::Text(name)) = fields.get("選択肢名") else {
            bail!("gallery selector name is not text");
        };
        let selector = super::profile::atoi(name) as u32;
        let menu = self.ensure_menu(target)?;
        // Like rvmm 1004de16, a missing numeric selector returns without
        // resolving/querying the database or inventing a new selector.
        if !self.menus[&menu]
            .data
            .selectors
            .iter()
            .any(|s| s.id == selector)
        {
            return Ok(());
        }
        let Some(Value::Reference(database)) = fields.get("データベース") else {
            bail!("gallery database is not an object");
        };
        let key = progress_key(
            fields
                .get("キー名となるオブジェクト")
                .context("gallery key missing")?,
        )?;
        let value = super::profile::atoi(self.profile_database(database)?.read_string(&key, "0"));
        self.menus
            .get_mut(&menu)
            .unwrap()
            .set_database_status(selector, value);
        self.refresh_menu(menu, false)
    }
    pub(super) fn record_gallery_images(&mut self, images: &[ObjectRef]) -> Result<()> {
        for image in images {
            let Some(id) = ImageId::of(image)? else {
                continue;
            };
            let Some(databases) = self.gallery.bindings.get(&id).cloned() else {
                continue;
            };
            let key = id.key();
            for (offset, database) in databases {
                if super::profile::atoi(self.profile_database(&database)?.read_string(&key, "0"))
                    != 1
                {
                    self.profiles.write_string(offset, &key, "1")?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{choose_image as choose, wait_selection};
    use super::*;
    use rugp_assets::{object::ClassId, script::Script};
    use rugp_scene_vm::Scenario;
    fn stored() -> ObjectRef {
        ObjectRef {
            class: Some(ClassId {
                name: "Cr6Ti".into(),
                schema: 3,
            }),
            cache_index: Some(1),
            flags: 0xc108,
            offset: Some(12345),
            size: Some(500),
            owner: None,
            selector: 0,
            inline: None,
        }
    }
    #[test]
    fn progress_identity_is_stable_across_cache_instances_but_not_different_resources() -> Result<()>
    {
        let source = stored();
        let expected = progress_key(&Value::Reference(source.clone()))?;
        let mut other = source.clone();
        other.cache_index = Some(999);
        other.flags ^= 0x200;
        assert_eq!(progress_key(&Value::Reference(other.clone()))?, expected);
        for variant in 0..5 {
            let mut other = source.clone();
            match variant {
                0 => other.offset = Some(12346),
                1 => other.size = Some(501),
                2 => other.selector = 1,
                3 => other.class.as_mut().unwrap().schema = 2,
                _ => other.class.as_mut().unwrap().name = "CRip008".into(),
            }
            assert_ne!(progress_key(&Value::Reference(other))?, expected);
        }
        assert_ne!(progress_key(&Value::Null)?, expected);
        for value in [Value::Text("1".into()), Value::Integer(1), Value::Real(1.0)] {
            assert!(progress_key(&value).is_err());
        }
        for variant in 0..4 {
            let mut other = stored();
            match variant {
                0 => other.offset = None,
                1 => other.size = Some(0),
                2 => other.flags |= 0x40,
                _ => other.class.as_mut().unwrap().name = "bad;key".into(),
            }
            assert!(progress_key(&Value::Reference(other)).is_err());
        }
        Ok(())
    }
    fn packet(game: &Game, scene: usize, address: u32) -> Result<Instruction> {
        let root = game.archive.volumes[0].path.parent().context("game root")?;
        let native = NativeSchema::from_directory(root)?;
        Scenario::decode_with_schema(
            &Script::load(&game.archive, scene)?,
            game.archive.address_shift,
            Some(&native),
        )?
        .instructions
        .into_iter()
        .find(|i| i.address == address)
        .context("gallery packet missing")
    }
    fn image_vm(image: ObjectRef, destination: ObjectRef) -> Result<Vm> {
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 64,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    Instruction {
                        index: 0,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmImage".into(),
                            schema: 17,
                        },
                        address: 0,
                        next_address: 48,
                        flags: 0,
                        arguments: vec![
                            Argument::Number(0),
                            Argument::Number(0),
                            Argument::Number(0),
                            Argument::Number(0),
                            Argument::Reference(image),
                            Argument::Reference(destination),
                        ],
                    },
                    Instruction {
                        index: 1,
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
        Vm::new(Arc::new(program), 1)
    }
    fn status(game: &mut Game, request: &Instruction) -> Result<u16> {
        let message = game.vm.message(request)?;
        let Value::Reference(target) = &message.target else {
            bail!("menu target missing");
        };
        let id = target.offset.context("menu identity missing")?;
        let Value::Text(name) = &message.fields["選択肢名"] else {
            bail!("selector missing");
        };
        let selector = super::super::profile::atoi(name) as u32;
        game.message(message)?;
        Ok(game.menus[&id]
            .data
            .selectors
            .iter()
            .find(|s| s.id == selector)
            .context("selector missing")?
            .status)
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_gallery_records_successful_host_use_not_preload_failure_or_restore_and_persists()
    -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let directory = std::env::temp_dir().join(format!(
            "rugp-gallery-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let mut game = Game::open_persistent(root.as_ref(), &directory)?;
        let request = packet(&game, 239, 0x88)?;
        let message = game.vm.message(&request)?;
        let Value::Reference(image) = &message.fields["キー名となるオブジェクト"]
        else {
            bail!("image missing");
        };
        let image = image.clone();
        assert_eq!(status(&mut game, &request)? & 7, 2);
        // Cached decoding (e.g. diagnostic/preload) must not unlock a CG.
        game.images.load(&image)?;
        assert_eq!(status(&mut game, &request)? & 7, 2);
        let mut destination = stored();
        destination.class.as_mut().unwrap().name = "WrongDestination".into();
        game.vm = image_vm(image.clone(), destination.clone())?;
        assert!(game.step(0).is_err());
        assert_eq!(status(&mut game, &request)? & 7, 2);
        destination.class.as_mut().unwrap().name = "CImgBox".into();
        game.vm = image_vm(image, destination)?;
        let before = game.snapshot()?;
        let mut resources = crate::audio::AudioResources::new(Arc::clone(&game.archive))?;
        game.restore_snapshot(&before, &mut resources)?;
        assert_eq!(status(&mut game, &request)? & 7, 2);
        assert_eq!(game.step(0)?, RunState::Ended);
        assert_eq!(status(&mut game, &request)? & 5, 1);
        game.restore_snapshot(&before, &mut resources)?;
        // Loading before the CG does not rewind globally accumulated progress.
        assert_eq!(status(&mut game, &request)? & 5, 1);
        assert_eq!(
            status(&mut game, &packet_from_root(root.as_ref(), 242, 0x11c)?)? & 7,
            2
        );
        let data = game.user_data_dir().unwrap().to_owned();
        drop(game);
        let mut game = Game::open_persistent(root.as_ref(), &directory)?;
        assert_eq!(status(&mut game, &request)? & 5, 1);
        drop(game);
        for entry in std::fs::read_dir(&data)? {
            std::fs::remove_file(entry?.path())?;
        }
        std::fs::remove_dir(data)?;
        std::fs::remove_dir(directory)?;
        Ok(())
    }
    fn packet_from_root(root: &Path, scene: usize, address: u32) -> Result<Instruction> {
        let archive = RioArchive::open(root)?;
        let native = NativeSchema::from_directory(root)?;
        Scenario::decode_with_schema(
            &Script::load(&archive, scene)?,
            archive.address_shift,
            Some(&native),
        )?
        .instructions
        .into_iter()
        .find(|i| i.address == address)
        .context("gallery packet missing")
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_all_263_database_selector_packets_keep_unseen_and_null_entries_locked() -> Result<()>
    {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let mut count = 0;
        let mut nulls = 0;
        for scene in [239, 242] {
            let code = Scenario::decode_with_schema(
                &Script::load(&game.archive, scene)?,
                game.archive.address_shift,
                Some(&native),
            )?;
            for instruction in &code.instructions {
                if !instruction.arguments.iter().any(|a| matches!(a,
                    Argument::Message { class, .. } if class.name.ends_with("OM_SetSelecterByDatabase"))) { continue; }
                let message = game.vm.message(instruction)?;
                nulls += usize::from(matches!(
                    message.fields["キー名となるオブジェクト"],
                    Value::Null
                ));
                assert_eq!(status(&mut game, instruction)? & 7, 2);
                count += 1;
            }
        }
        assert_eq!((count, nulls), (263, 7));
        ensure!(
            !game.gallery.bindings.is_empty() && game.gallery.bindings.len() <= 256,
            "gallery image catalogue missing"
        );
        Ok(())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_premium_hub_runs_locked_cg_pages_and_returns_without_unlock_bypasses() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let mut audio = None;
        // Isolated menu coverage: real startup establishes windows/globals.
        // Enter the original premium hub, without modifying any clear/unlock
        // flag. This does not claim that the title's gated premium is unlocked.
        for _ in 0..200 {
            game.step(10000)?;
            crate::audio::mute_requests(&mut game, &mut audio)?;
            if game.waiting_for_selection() {
                break;
            }
        }
        ensure!(game.waiting_for_selection(), "title missing");
        let native = NativeSchema::from_directory(root.as_ref())?;
        let program = Program::load(&game.archive, &native)?;
        let mut vm = Vm::new(Arc::new(program), 251)?;
        vm.variables = std::mem::take(&mut game.vm.variables);
        game.vm = vm;
        game.selection = None;
        wait_selection(&mut game, &mut audio)?;
        assert_eq!(game.vm.cursor().unwrap().scene, 251);
        choose(&mut game, 1)?;
        wait_selection(&mut game, &mut audio)?;
        assert_eq!(game.vm.cursor().unwrap().scene, 239);
        for page in 0..12 {
            let target = game.selection.as_ref().unwrap().target;
            let menu = game.objects[&target].menu.unwrap();
            if game.menus[&menu]
                .data
                .selectors
                .iter()
                .any(|s| s.id == 26 + page && s.status & 1 != 0)
            {
                choose(&mut game, 26 + page)?;
                wait_selection(&mut game, &mut audio)?;
            }
            assert_eq!(game.vm.cursor().unwrap().scene, 239);
            let target = game.selection.as_ref().unwrap().target;
            let menu = game.objects[&target].menu.unwrap();
            ensure!(
                game.menus[&menu]
                    .data
                    .selectors
                    .iter()
                    .filter(|s| (1..=25).contains(&s.id))
                    .all(|s| s.status & 1 == 0),
                "unseen CG became selectable on page {page}"
            );
        }
        let database = game
            .gallery
            .bindings
            .values()
            .next()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .clone();
        ensure!(
            !game
                .profile_database(&database)?
                .text()
                .contains("$rugp:seen:1:"),
            "browsing locked thumbnails recorded unseen CGs"
        );
        choose(&mut game, 40)?;
        wait_selection(&mut game, &mut audio)?;
        assert_eq!(game.vm.cursor().unwrap().scene, 251);
        Ok(())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_premium_music_replay_and_locked_bonus_return_to_hub() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let mut audio = None;
        for _ in 0..200 {
            game.step(10000)?;
            crate::audio::mute_requests(&mut game, &mut audio)?;
            if game.waiting_for_selection() {
                break;
            }
        }
        ensure!(game.waiting_for_selection(), "title missing");
        let title = game.vm.cursor().cloned();
        let native = NativeSchema::from_directory(root.as_ref())?;
        let program = Program::load(&game.archive, &native)?;
        let mut vm = Vm::new(Arc::new(program), 251)?;
        vm.variables = std::mem::take(&mut game.vm.variables);
        game.vm = vm;
        game.selection = None;
        wait_selection(&mut game, &mut audio)?;
        let hub = game.vm.cursor().cloned();
        for id in [2, 3, 4, 5] {
            ensure!(game.vm.cursor() == hub.as_ref(), "hub missing");
            if id == 5 {
                let target = game
                    .selection
                    .as_ref()
                    .context("hub selection missing")?
                    .target;
                let menu = game.objects[&target]
                    .menu
                    .context("hub image menu missing")?;
                ensure!(
                    game.menus[&menu]
                        .data
                        .selectors
                        .iter()
                        .any(|s| s.id == 5 && s.status & 1 == 0),
                    "fresh profile exposed an unearned bonus selector"
                );
                continue;
            }
            choose(&mut game, id)?;
            if id == 4 {
                let mut pages = 0;
                for _ in 0..200 {
                    if game.dialogue_waits_for_input() || game.waiting_for_input() {
                        game.activate_selection()?;
                        pages += 1;
                    }
                    crate::audio::mute_requests(&mut game, &mut audio)?;
                    game.step(1000)?;
                    if game.waiting_for_selection() {
                        break;
                    }
                }
                ensure!(
                    pages > 0 && game.waiting_for_selection() && game.vm.cursor() == hub.as_ref(),
                    "locked bonus did not return to the original hub after its explanation"
                );
                continue;
            }
            wait_selection(&mut game, &mut audio)?;
            if id == 2 {
                ensure!(
                    game.vm.cursor().is_some_and(|pc| pc.scene == 237),
                    "music menu missing"
                );
                choose(&mut game, 1)?;
                wait_selection(&mut game, &mut audio)?;
                ensure!(
                    game.pending_sounds().iter().any(|r| r.kind == 2
                        && r.ticket.is_some()
                        && r.source
                            .as_ref()
                            .is_some_and(|s| s.offset == Some(1072549207))),
                    "original music selector did not issue its real playback request"
                );
            } else {
                // Both original replay pages stay locked in a fresh profile.
                for page in [27, 26] {
                    choose(&mut game, page)?;
                    wait_selection(&mut game, &mut audio)?;
                    ensure!(
                        game.vm.cursor().is_some_and(|pc| pc.scene == 242),
                        "replay menu missing"
                    );
                    let target = game
                        .selection
                        .as_ref()
                        .context("replay selection missing")?
                        .target;
                    let menu = game.objects[&target]
                        .menu
                        .context("replay image menu missing")?;
                    ensure!(
                        game.menus[&menu]
                            .data
                            .selectors
                            .iter()
                            .filter(|s| (1..=25).contains(&s.id))
                            .all(|s| s.status & 1 == 0),
                        "unseen replay became selectable"
                    );
                }
            }
            choose(&mut game, if id == 2 { 33 } else { 28 })?;
            wait_selection(&mut game, &mut audio)?;
            ensure!(
                game.vm.cursor() == hub.as_ref(),
                "submenu did not return to hub"
            );
        }
        choose(&mut game, 6)?;
        wait_selection(&mut game, &mut audio)?;
        ensure!(
            game.vm.cursor() == title.as_ref(),
            "premium back did not reach original title"
        );
        Ok(())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME; requires a Japanese outline font"]
    fn retail_original_new_game_path_records_a_seen_cg_without_writing_progress_from_a_test_hook()
    -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        // Host/resource/clock verification, not pixel verification. No saves
        // or original files are written by this volatile diagnostic session.
        game.set_scene_rasterization(false);
        let database = game
            .gallery
            .bindings
            .values()
            .next()
            .context("gallery empty")?
            .values()
            .next()
            .context("gallery database missing")?
            .clone();
        let mut audio = None;
        let mut confirmations = 0;
        let mut choices = 0;
        for tick in 0..5000 {
            if game.waiting_for_input() {
                game.activate_selection()?;
            }
            if game.dialogue_waits_for_input() {
                game.activate_selection()?;
                confirmations += 1;
            }
            if game.waiting_for_selection() {
                game.navigate_selection(1)?;
                game.activate_selection()?;
                choices += 1;
            }
            crate::audio::mute_requests(&mut game, &mut audio)?;
            ensure!(
                game.step(if tick == 0 { 0 } else { 1000 })? != RunState::Ended,
                "new game ended before a CG was encountered"
            );
            if game
                .profile_database(&database)?
                .text()
                .contains("$rugp:seen:1:")
            {
                ensure!(
                    choices > 0 && confirmations > 0,
                    "progress appeared before new game dialogue"
                );
                eprintln!(
                    "original story recorded a CG at {:?}; ticks={tick}, pages={confirmations}, choices={choices}",
                    game.vm.cursor()
                );
                let progress = std::mem::take(&mut game.profiles);
                drop(game);
                drop(audio);
                return view_seen_cg(root.as_ref(), progress);
            }
        }
        bail!(
            "first-CG probe exhausted at {:?}, pages={confirmations}, choices={choices}",
            game.vm.cursor()
        )
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME; original earned replay playback"]
    fn retail_earned_replay_plays_original_scene_and_returns_without_forced_unlock() -> Result<()> {
        check_earned_replay(200, 478, 0, 1)
    }

    // Original SC242 database packets and their dispatch targets. Null keys
    // (seven permanently unavailable entries) are audited separately. Fixed
    // batches allow independent reruns without silently filtering coverage.
    const REMAINING_REPLAYS: [(u32, usize, u32, u32); 32] = [
        (228, 346, 0, 2),
        (256, 327, 0, 3),
        (312, 407, 0, 5),
        (340, 425, 0, 6),
        (368, 415, 0, 7),
        (396, 415, 0, 8),
        (424, 418, 0, 9),
        (452, 418, 0, 10),
        (508, 296, 0, 12),
        (564, 341, 0, 14),
        (620, 356, 0, 16),
        (648, 396, 0, 17),
        (676, 398, 0, 18),
        (704, 403, 0, 19),
        (732, 401, 0, 20),
        (760, 405, 0, 21),
        (788, 412, 0, 22),
        (816, 407, 0, 23),
        (844, 334, 0, 24),
        (872, 313, 0, 25),
        (11136, 449, 1, 1),
        (11192, 453, 1, 3),
        (11220, 386, 1, 4),
        (11248, 384, 1, 5),
        (11304, 269, 1, 7),
        (11332, 189, 1, 8),
        (11360, 292, 1, 9),
        (11388, 290, 1, 10),
        (11416, 304, 1, 11),
        (11444, 370, 1, 12),
        (11472, 231, 1, 13),
        (11500, 217, 1, 14),
    ];

    #[test]
    #[ignore = "set RUGP_TEST_GAME; long original earned replay playback"]
    fn retail_earned_replays_page_one_two_through_eight() -> Result<()> {
        check_earned_replays(&REMAINING_REPLAYS[..6])
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME; long original earned replay playback"]
    fn retail_earned_replays_page_one_nine_through_eighteen() -> Result<()> {
        check_earned_replays(&REMAINING_REPLAYS[6..13])
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME; long original earned replay playback"]
    fn retail_earned_replays_page_one_nineteen_through_twenty_five() -> Result<()> {
        check_earned_replays(&REMAINING_REPLAYS[13..20])
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME; long original earned replay playback"]
    fn retail_earned_replays_page_two() -> Result<()> {
        check_earned_replays(&REMAINING_REPLAYS[20..])
    }

    fn check_earned_replays(cases: &[(u32, usize, u32, u32)]) -> Result<()> {
        for &(packet, entry, page, selector) in cases {
            check_earned_replay(packet, entry, page, selector).with_context(|| {
                format!("replay page {page}, selector {selector}, story {entry}")
            })?;
        }
        Ok(())
    }

    fn check_earned_replay(
        packet_address: u32,
        entry: usize,
        page: u32,
        selector: u32,
    ) -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        game.set_scene_rasterization(false);
        let mut audio = None;
        wait_selection(&mut game, &mut audio)?;
        // Start the corresponding story scene to earn its actual image use. This
        // isolated scene probe does not claim to have played its story prefix.
        let binding = game.vm.message(&packet(&game, 242, packet_address)?)?;
        ensure!(
            matches!(&binding.fields["選択肢名"], Value::Text(id) if id == &selector.to_string()),
            "replay fixture selector does not match native binding"
        );
        let Value::Reference(expected_menu) = &binding.target else {
            bail!("replay menu identity missing");
        };
        let Value::Reference(database) = &binding.fields["データベース"] else {
            bail!("replay database missing");
        };
        let key = progress_key(&binding.fields["キー名となるオブジェクト"])?;
        ensure!(
            game.profile_database(database)?.read_string(&key, "0") == "0",
            "fresh replay was already earned"
        );
        let native = NativeSchema::from_directory(root.as_ref())?;
        let program = Arc::new(Program::load(&game.archive, &native)?);
        let mut vm = Vm::new(program, entry)?;
        vm.variables = std::mem::take(&mut game.vm.variables);
        game.vm = vm;
        game.selection = None;
        let mut pages = 0;
        let mut earned = false;
        for tick in 0u32..10_000 {
            if game.dialogue_waits_for_input() {
                game.activate_selection()?;
                pages += 1;
            }
            if game.waiting_for_input() {
                game.activate_selection()?;
            }
            if game.waiting_for_selection() {
                game.navigate_selection(1)?;
                game.activate_selection()?;
            }
            crate::audio::mute_requests(&mut game, &mut audio)?;
            ensure!(
                game.step(1000).with_context(|| format!(
                    "earning replay {page}/{selector}, tick {tick}, pc {:?}",
                    game.vm.cursor()
                ))? != RunState::Ended,
                "story ended before earning replay"
            );
            // Some original scenes show the bound CG before their first
            // dialogue. Keep playing until both real image use and dialogue
            // have occurred, rather than treating that ordering as an error.
            if pages > 0 && game.profile_database(database)?.read_string(&key, "0") == "1" {
                eprintln!(
                    "original scene {entry} earned replay {page}/{selector}: tick={tick}, pages={pages}"
                );
                earned = true;
                break;
            }
            if tick.is_multiple_of(1000) {
                eprintln!(
                    "earning replay {page}/{selector}: tick={tick}, pages={pages}, pc={:?}",
                    game.vm.cursor()
                );
            }
        }
        ensure!(
            earned,
            "story did not earn replay at {:?}",
            game.vm.cursor()
        );
        let progress = std::mem::take(&mut game.profiles);
        drop(game);
        drop(audio);

        let mut game = Game::open(root.as_ref())?;
        game.set_scene_rasterization(false);
        game.profiles = progress;
        let mut audio = None;
        wait_selection(&mut game, &mut audio)?;
        let title = game.vm.cursor().cloned();
        // Isolate the hub, not its title permission. No ending or replay flag
        // is fabricated; the original enabled selector sets replay mode itself.
        let program = Arc::new(Program::load(&game.archive, &native)?);
        let mut vm = Vm::new(program, 251)?;
        vm.variables = std::mem::take(&mut game.vm.variables);
        game.vm = vm;
        game.selection = None;
        wait_selection(&mut game, &mut audio)?;
        choose(&mut game, 3)?;
        wait_selection(&mut game, &mut audio)?;
        if page == 1 {
            choose(&mut game, 27)?;
            wait_selection(&mut game, &mut audio)?;
        }
        let replay = game.vm.cursor().cloned();
        ensure!(
            replay.as_ref().is_some_and(|pc| pc.scene == 242),
            "replay page missing"
        );
        let target = game
            .selection
            .as_ref()
            .context("replay selection missing")?
            .target;
        ensure!(
            game.objects[&target].menu == expected_menu.offset,
            "replay fixture reached the wrong native menu page"
        );
        choose(&mut game, selector)?;
        let mut entered = false;
        let mut pages = 0;
        let mut confirmations = 0;
        let mut choices = 0;
        for tick in 0u32..20_000 {
            entered |= game.vm.cursor().is_some_and(|pc| pc.scene == entry);
            if game.waiting_for_selection() && game.vm.cursor() == replay.as_ref() {
                ensure!(
                    entered && pages > 0,
                    "replay returned without playing original scene"
                );
                eprintln!(
                    "earned replay {page}/{selector} returned: ticks={tick}, pages={pages}, confirmations={confirmations}, choices={choices}"
                );
                choose(&mut game, 28)?;
                wait_selection(&mut game, &mut audio)?;
                choose(&mut game, 6)?;
                wait_selection(&mut game, &mut audio)?;
                ensure!(
                    game.vm.cursor() == title.as_ref(),
                    "replay back did not reach title"
                );
                return Ok(());
            }
            if game.waiting_for_selection() {
                // Some original replays retain an in-story text choice. It
                // is not the replay-page return; use normal enabled input.
                ensure!(
                    game.selection_text_target().is_some(),
                    "replay reached an unexpected image menu at {:?}",
                    game.vm.cursor()
                );
                game.navigate_selection(1)?;
                game.activate_selection()?;
                choices += 1;
            }
            if game.dialogue_waits_for_input() {
                game.activate_selection()?;
                pages += 1;
            }
            if game.waiting_for_input() {
                game.activate_selection()?;
                confirmations += 1;
            }
            crate::audio::mute_requests(&mut game, &mut audio)?;
            ensure!(
                game.step(1000).with_context(|| format!(
                    "earned replay tick {tick}, pc {:?}",
                    game.vm.cursor()
                ))? != RunState::Ended,
                "replay ended instead of returning"
            );
            if tick.is_multiple_of(1000) {
                eprintln!(
                    "replay {page}/{selector}: tick={tick}, pages={pages}, pc={:?}",
                    game.vm.cursor()
                );
            }
        }
        bail!("replay budget exhausted at {:?}", game.vm.cursor())
    }
    fn view_seen_cg(root: &Path, progress: crate::profile_store::ProfileStore) -> Result<()> {
        let mut game = Game::open(root)?;
        game.profiles = progress;
        let mut audio = None;
        for _ in 0..200 {
            game.step(10000)?;
            crate::audio::mute_requests(&mut game, &mut audio)?;
            if game.waiting_for_selection() {
                break;
            }
        }
        ensure!(
            game.waiting_for_selection(),
            "fresh gallery host did not reach title"
        );
        let native = NativeSchema::from_directory(root)?;
        let cg = Scenario::decode_with_schema(
            &Script::load(&game.archive, 239)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let mut selected = None;
        let mut page = None;
        for instruction in &cg.instructions {
            if instruction.arguments.iter().any(|a| {
                matches!(a,
                Argument::Message { class, .. } if class.name.ends_with("OM_AllIgnoreSelecter"))
            }) {
                page = Some(page.map_or(0u32, |p| p + 1));
            }
            if !instruction.arguments.iter().any(|a| {
                matches!(a,
                Argument::Message { class, .. } if class.name.ends_with("OM_SetSelecterByDatabase"))
            }) {
                continue;
            }
            let message = game.vm.message(instruction)?;
            let Value::Reference(database) = &message.fields["データベース"] else {
                bail!("database missing");
            };
            let key = progress_key(&message.fields["キー名となるオブジェクト"])?;
            if super::super::profile::atoi(game.profile_database(database)?.read_string(&key, "0"))
                == 1
            {
                let Value::Reference(menu) = &message.target else {
                    bail!("menu missing");
                };
                let Value::Text(selector) = &message.fields["選択肢名"] else {
                    bail!("selector missing");
                };
                selected = Some((
                    page.context("gallery page boundary missing")?,
                    menu.offset.context("menu identity missing")?,
                    super::super::profile::atoi(selector) as u32,
                ));
                break;
            }
        }
        let (page, menu, selector) =
            selected.context("original story progress did not match CG gallery")?;
        // Isolated hub entry preserves the real earned profile, but does not
        // fabricate any ending-clear permission to unlock the title button.
        let program = Program::load(&game.archive, &native)?;
        let mut vm = Vm::new(Arc::new(program), 251)?;
        vm.variables = std::mem::take(&mut game.vm.variables);
        game.vm = vm;
        game.selection = None;
        wait_selection(&mut game, &mut audio)?;
        choose(&mut game, 1)?;
        wait_selection(&mut game, &mut audio)?;
        let target = game
            .selection
            .as_ref()
            .context("CG selection missing")?
            .target;
        let current = game.objects[&target].menu.context("CG menu missing")?;
        if game.menus[&current]
            .data
            .selectors
            .iter()
            .any(|s| s.id == 26 + page && s.status & 1 != 0)
        {
            choose(&mut game, 26 + page)?;
            wait_selection(&mut game, &mut audio)?;
        }
        let target = game
            .selection
            .as_ref()
            .context("CG selection missing")?
            .target;
        ensure!(
            game.objects[&target].menu == Some(menu),
            "earned CG page was not reached"
        );
        let page_pixels = game.frame().to_vec();
        choose(&mut game, selector)?;
        let mut clicks = 0;
        for tick in 0..100 {
            if game.waiting_for_input() {
                ensure!(
                    game.frame() != page_pixels,
                    "CG viewer still displays the thumbnail page"
                );
                let cursor = game.vm.cursor().cloned();
                game.step(100000)?;
                assert_eq!(game.vm.cursor(), cursor.as_ref());
                assert!(game.waiting_for_input());
                game.pointer_move(0, 0)?;
                game.pointer_button(true)?;
                super::super::ui_snapshot::test_roundtrip(&mut game)?;
                game.pointer_button(false)?;
                assert!(game.waiting_for_input());
                assert_eq!(game.vm.cursor(), cursor.as_ref());
                game.activate_selection()?;
                clicks += 1;
            }
            crate::audio::mute_requests(&mut game, &mut audio)?;
            game.step(1000)
                .with_context(|| format!("CG viewer tick {tick}"))?;
            if game.waiting_for_selection() {
                ensure!(clicks > 0, "CG viewer returned without a user confirmation");
                ensure!(
                    game.vm.cursor().is_some_and(|pc| pc.scene == 239),
                    "CG viewer returned elsewhere"
                );
                eprintln!(
                    "earned CG viewer returned to its page after {tick} ticks and {clicks} real confirmations"
                );
                return Ok(());
            }
        }
        bail!(
            "earned CG viewer remains suspended at {:?}, instruction={:?}",
            game.vm.cursor(),
            game.vm.pending_instruction().map(|i| &i.class.name)
        )
    }
}
