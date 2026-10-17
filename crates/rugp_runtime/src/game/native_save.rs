//! Import native dialogue checkpoints by replaying the original program.
//! Replay uses a separate, volatile Game. Validation and resource preparation
//! must finish before the live game changes.
use super::*;
use crate::{audio::AudioResources, audio_state::snapshot::PlaybackResume};
use rugp_assets::{script::Script, uuc::NativeSave};
use rugp_scene_vm::Scenario;
use std::fs::File;

impl Game {
    /// Import a v19 UUC dialogue checkpoint with an empty native call stack.
    /// `choices` contains native selector IDs after the title's New Game item.
    /// Audio and animation clocks restart at the reconstructed dialogue page.
    pub fn import_uuc(
        &mut self,
        path: &Path,
        choices: &[u32],
        resources: &mut AudioResources,
    ) -> Result<Vec<PlaybackResume>> {
        let snapshot = self.native_checkpoint(path, choices)?;
        self.restore_snapshot(&snapshot, resources)
    }

    fn native_checkpoint(&self, path: &Path, choices: &[u32]) -> Result<snapshot::GameSnapshot> {
        let saved = NativeSave::read(
            File::open(path).context("open Windows UUC save")?,
            self.archive.address_shift,
        )?;
        let project = self
            .archive
            .project
            .as_ref()
            .context("game project missing")?;
        ensure!(
            saved.title == project.game_version
                && saved.registry == project.title
                && saved.game_version == project.scenario_version,
            "UUC save belongs to a different game or script version"
        );
        let process = self
            .archive
            .nodes
            .iter()
            .find(|n| n.class.name == "CProcessOcean")
            .and_then(|n| n.offset)
            .context("native process missing")?;
        let (scene_offset, instruction_id) =
            saved.dialogue_cursor(process, self.archive.address_shift)?;
        let node = self
            .archive
            .nodes
            .iter()
            .find(|n| n.offset == Some(scene_offset) && n.class.name == "CRsa")
            .context("UUC scene is not in this project")?;
        let directory = self.archive.volumes[0]
            .path
            .parent()
            .context("game root missing")?;
        let native = NativeSchema::from_directory(directory)?;
        let script = Script::load(&self.archive, node.id)?;
        let scenario =
            Scenario::decode_with_schema(&script, self.archive.address_shift, Some(&native))?;
        let instructions: Vec<_> = scenario
            .instructions
            .iter()
            .filter(|i| i.flags & 0xfffff == instruction_id)
            .collect();
        ensure!(
            instructions.len() == 1 && instructions[0].class.name == "CVmMsg",
            "UUC checkpoint is not a supported dialogue instruction"
        );
        let cursor = rugp_scene_vm::vm::Cursor {
            scene: node.id,
            address: instructions[0].address,
        };
        let mut scalars = BTreeMap::new();
        for (offset, data) in &saved.objects {
            let node = self
                .archive
                .nodes
                .iter()
                .find(|n| n.offset == Some(*offset))
                .context("UUC object is not in this project")?;
            let value = match node.class.name.as_str() {
                "CBool" => {
                    ensure!(data.len() == 1 && data[0] <= 1, "invalid UUC Boolean");
                    Some(Value::Integer(i32::from(data[0])))
                }
                "CInt" => {
                    ensure!(data.len() == 4, "invalid UUC integer");
                    Some(Value::Integer(i32::from_le_bytes(
                        data.as_slice().try_into()?,
                    )))
                }
                "CVmVarObj" => {
                    use rugp_assets::uuc::value::{ScalarValue, read_scalar};
                    read_scalar(data)?.map(|value| match value {
                        ScalarValue::Null => Value::Null,
                        ScalarValue::Integer(value) => Value::Integer(value),
                        ScalarValue::Real(value) => Value::Real(value),
                        ScalarValue::Text(value) => Value::Text(value),
                    })
                }
                _ => None,
            };
            if let Some(value) = value {
                scalars.insert(*offset, value);
            }
        }
        let mut replay = Game::open(directory)?;
        replay.font_file = self.font_file.clone();
        let mut audio = None; crate::game::SCRATCH_ARM.store(false, std::sync::atomic::Ordering::Relaxed);
        let mut title = false;
        let mut choice_at = 0;
        // Bound work even for a damaged save or a route that cannot reach the
        // saved instruction. Replay does not write profiles or save files.
        for _ in 0..100_000 {
            crate::audio::mute_requests(&mut replay, &mut audio)?;
            if replay.vm.cursor() == Some(&cursor) && replay.waiting_for_dialogue() {
                ensure!(choice_at == choices.len(), "unused UUC route choices");
                let instruction = replay
                    .vm
                    .pending_instruction()
                    .context("UUC dialogue instruction missing")?;
                let Value::Text(text) = replay.vm.evaluate(&instruction.arguments[1])? else {
                    bail!("UUC dialogue is not text")
                };
                let window = replay.dialogue.as_ref().unwrap().target;
                ensure!(
                    saved.dialogue_text(window)? == text,
                    "UUC dialogue text does not match the replayed route"
                );
                // Preserve native scalar variables, including route flags and
                // preferences. Transient handles come from actual execution.
                replay.vm.variables.extend(scalars);
                replay.navigation = saved.navigation;
                return replay.capture_snapshot();
            }
            if replay.waiting_for_selection() {
                let id = if !title {
                    title = true;
                    1 // Latest Edition title: New Game
                } else {
                    let id = *choices.get(choice_at).with_context(|| {
                        format!(
                            "UUC route needs a selector ID at {:?}; supply --choice",
                            replay.vm.cursor()
                        )
                    })?;
                    choice_at += 1;
                    id
                };
                replay.choose_native_selector(id)?;
            } else if replay.dialogue_waits_for_input() || replay.waiting_for_input() {
                replay.activate_selection()?;
            }
            replay.skip_video()?;
            ensure!(
                replay.step(1000)? != RunState::Ended,
                "UUC route ended before the saved dialogue"
            );
        }
        bail!(
            "UUC replay budget exceeded before {}:{:#x}",
            cursor.scene,
            cursor.address
        )
    }

    fn choose_native_selector(&mut self, id: u32) -> Result<()> {
        let target = self.selection.as_ref().context("selection missing")?.target;
        if let Some(menu) = self.text_menus.get(&target) {
            let count = menu.entries.len();
            ensure!((id as usize) < count, "UUC text selector {id} missing");
            for _ in 0..=count {
                self.navigate_selection(1)?;
                if self.text_menus[&target].hover == Some(id as usize) {
                    return self.activate_selection();
                }
            }
        } else {
            let menu = self.objects[&target]
                .menu
                .context("UUC image menu missing")?;
            let count = self.menus[&menu].data.selectors.len();
            for _ in 0..=count {
                self.navigate_selection(1)?;
                if self.menus[&menu]
                    .hover
                    .and_then(|key| self.menus[&menu].result(key))
                    == Some(id)
                {
                    return self.activate_selection();
                }
            }
        }
        bail!("UUC selector {id} is missing or disabled")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[ignore = "set RUGP_TEST_GAME and RUGP_TEST_UUC_DIR"]
    fn retail_park_character_placement() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let saves = std::path::PathBuf::from(std::env::var_os("RUGP_TEST_UUC_DIR").context("UUC directory")?);
        let mut game = Game::open(root.as_ref())?;
        let mut resources = AudioResources::new(Arc::clone(&game.archive))?;
        game.import_uuc(&saves.join("vmr00001.uuc"), &[], &mut resources)?;
        let mut audio = None; crate::game::SCRATCH_ARM.store(false, std::sync::atomic::Ordering::Relaxed);
        for tick in 0..1000 {
            if game
                .vm
                .cursor()
                .is_some_and(|c| c.scene == 483 && c.address == 0xf49c)
            {
                // Native SC483:f49c contains Mitsuki alone. The preceding
                // background HumanOut removes Shinji before the camera changes.
                assert!(!game.objects[&324542].character.has_image());
                let actor = &game.objects[&357909];
                assert!(actor.character.has_image());
                assert_eq!(actor.character.position, [1820, -320, 2080]);
                for name in ["ロール角量", "ピッチ角量", "バンク角量"] {
                    assert_eq!(number(&actor.properties, name, 0)?, 0);
                }
                if let Some(path) = std::env::var_os("RUGP_TEST_FRAME") {
                    let mut ppm = format!("P6\n{} {}\n255\n", game.frame.width, game.frame.height)
                        .into_bytes();
                    ppm.extend(
                        game.frame()
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .flat_map(|p| p[..3].iter().copied()),
                    );
                    std::fs::write(path, ppm)?;
                }
                return Ok(());
            }
            if game.dialogue_waits_for_input() || game.waiting_for_input() {
                game.activate_selection()?;
            }
            game.skip_video()?;
            game.step(100)
                .with_context(|| format!("Park tick {tick}"))?;
            crate::audio::mute_requests(&mut game, &mut audio)?;
        }
        bail!("Park scene was not reached")
    }

    #[test]
    #[ignore = "set RUGP_TEST_GAME and RUGP_TEST_UUC_DIR (contains vmr00001.uuc)"]
    fn retail_shinji_transition_uses_actor_rotation() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let saves = std::path::PathBuf::from(std::env::var_os("RUGP_TEST_UUC_DIR").context("UUC directory")?);
        let mut game = Game::open(root.as_ref())?;
        let mut resources = AudioResources::new(Arc::clone(&game.archive))?;
        game.import_uuc(&saves.join("vmr00001.uuc"), &[], &mut resources)?;
        let mut audio = None; crate::game::SCRATCH_ARM.store(false, std::sync::atomic::Ordering::Relaxed);
        for tick in 0..500 {
            if game
                .vm
                .cursor()
                .is_some_and(|c| c.scene == 483 && c.address == 0x9cb8)
            {
                let actor = &game.objects[&324542];
                assert_eq!(actor.parent, Some(212259));
                assert_eq!(actor.character.position, [960, -440, 600]);
                assert_eq!(number(&actor.properties, "バンク角量", 0)?, -27);
                // The rotated face covers this point. Ignoring the actor's
                // bank leaves sky here, reproducing the reported screenshot.
                // The script's OM_SetShading (SC33) darkens this actor toward
                // black, so the skin tone (234, 200, 168) is shaded.
                let shade = crate::game::effects::actor_shade(actor).context("Shinji shading")?;
                let pixel = (150 * game.frame.width as usize + 550) * 4;
                let expected: Vec<u8> = [234u8, 200, 168]
                    .iter()
                    .zip(shade.0)
                    .map(|(&v, target)| {
                        (f64::from(v) + (target - f64::from(v)) * shade.1).round() as u8
                    })
                    .chain([255])
                    .collect();
                assert_eq!(&game.frame()[pixel..pixel + 4], expected.as_slice());
                if let Some(path) = std::env::var_os("RUGP_TEST_FRAME") {
                    let mut ppm = format!("P6\n{} {}\n255\n", game.frame.width, game.frame.height)
                        .into_bytes();
                    ppm.extend(
                        game.frame()
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .flat_map(|p| p[..3].iter().copied()),
                    );
                    std::fs::write(path, ppm)?;
                }
                return Ok(());
            }
            if game.dialogue_waits_for_input() || game.waiting_for_input() {
                game.activate_selection()?;
            }
            game.skip_video()?;
            game.step(100)
                .with_context(|| format!("Shinji tick {tick}"))?;
            crate::audio::mute_requests(&mut game, &mut audio)?;
        }
        bail!("Shinji scene was not reached")
    }

    #[test]
    #[ignore = "set RUGP_TEST_GAME and RUGP_TEST_UUC_DIR (contains vmr00001.uuc)"]
    fn retail_slot_one_continues_through_camera_transitions() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let saves = std::path::PathBuf::from(std::env::var_os("RUGP_TEST_UUC_DIR").context("UUC directory")?);
        let mut game = Game::open(root.as_ref())?;
        let mut resources = AudioResources::new(Arc::clone(&game.archive))?;
        game.import_uuc(&saves.join("vmr00001.uuc"), &[], &mut resources)?;
        let mut audio = None; crate::game::SCRATCH_ARM.store(false, std::sync::atomic::Ordering::Relaxed);
        // Before camera-space clipping, tick 403 fails at SC60:0xd3c while
        // OM_BuildParseImage captures a plane crossing the camera.
        for tick in 0..500 {
            if game.dialogue_waits_for_input() || game.waiting_for_input() {
                game.activate_selection()?;
            }
            game.skip_video()?;
            game.step(100)
                .with_context(|| format!("Slot 1 continuation tick {tick}"))?;
            crate::audio::mute_requests(&mut game, &mut audio)?;
            ensure!(
                !game.waiting_for_selection() && !game.ended,
                "Slot 1 continuation stopped at tick {tick}"
            );
        }
        assert_ne!(
            game.vm.cursor().map(|c| (c.scene, c.address)),
            Some((483, 0x6dd0))
        );
        Ok(())
    }

    #[test]
    #[ignore = "set RUGP_TEST_GAME and RUGP_TEST_UUC_DIR (contains vmr00001.uuc and vmr00002.uuc)"]
    fn retail_uuc_load_continuation_and_failed_import_are_atomic() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let saves = std::path::PathBuf::from(std::env::var_os("RUGP_TEST_UUC_DIR").context("UUC directory")?);
        let directory = std::env::temp_dir().join(format!(
            "rugp-native-save-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let mut game = Game::open_persistent(root.as_ref(), &directory)?;
        let mut resources = AudioResources::new(Arc::clone(&game.archive))?;
        for (name, address) in [("vmr00001.uuc", 0x6dd0), ("vmr00002.uuc", 0x8330)] {
            let original = std::fs::read(saves.join(name))?;
            game.import_uuc(&saves.join(name), &[], &mut resources)?;
            assert_eq!(std::fs::read(saves.join(name))?, original);
            if name == "vmr00001.uuc" {
                let actor = &game.objects[&357909];
                let geometry = actor
                    .character
                    .geometry
                    .context("sprite geometry missing")?;
                assert_eq!(actor.character.position, [80, 0, 4280]);
                assert_eq!(geometry.size, [899, 1956]);
                assert_eq!(geometry.anchor_q16, [26394404, 118929091]);
                assert_eq!(actor.parent, Some(212259));
                assert_eq!(game.objects[&212259].world_projection, 21);
            }
            assert!(game.waiting_for_dialogue());
            assert_eq!(
                game.vm.cursor().map(|c| (c.scene, c.address)),
                Some((483, address))
            );
            let cursor = game.vm.cursor().cloned();
            let pixels = game.frame().to_vec();
            let corrupt = directory.join("invalid.uuc");
            std::fs::write(&corrupt, b"invalid checkpoint")?;
            assert!(game.import_uuc(&corrupt, &[], &mut resources).is_err());
            assert_eq!(game.vm.cursor(), cursor.as_ref());
            assert_eq!(game.frame(), pixels);
            let mut audio = None; crate::game::SCRATCH_ARM.store(false, std::sync::atomic::Ordering::Relaxed);
            for _ in 0..10 {
                game.activate_selection()?;
                game.step(1000)?;
                crate::audio::mute_requests(&mut game, &mut audio)?;
                if game.vm.cursor() != cursor.as_ref() {
                    break;
                }
            }
            assert_ne!(game.vm.cursor(), cursor.as_ref());
            assert!(!game.ended);
        }
        // Change only a native variant record in a temporary UUC. This checks
        // scalar restoration through replay, including payloads of new sizes.
        // No original save or copied reference-cache identity is changed.
        use rugp_assets::uuc::{
            NativeArchive, reference_writer::ReferenceWriter, value::NativeValue,
        };
        let original = std::fs::read(saves.join("vmr00001.uuc"))?;
        let saved = NativeSave::read(original.as_slice(), game.archive.address_shift)?;
        let offset = game
            .archive
            .nodes
            .iter()
            .filter(|node| node.class.name == "CVmVarObj")
            .filter_map(|node| node.offset)
            .find(|offset| {
                saved
                    .objects
                    .get(offset)
                    .is_some_and(|data| data == &[2, 0, 0, 0])
            })
            .context("native scalar variant fixture missing")?;
        for value in [
            NativeValue::Integer(37),
            NativeValue::Integer(i32::MAX),
            NativeValue::Real(-1.5),
            NativeValue::Text("設定"),
            NativeValue::Object(None),
        ] {
            let mut archive = NativeArchive::read(original.as_slice())?;
            let mut needle = vec![1, 2, 0];
            needle.extend(((offset >> game.archive.address_shift) as u32).to_le_bytes());
            needle.extend(4u32.to_le_bytes());
            needle.extend([2, 0, 0, 0]);
            let positions: Vec<_> = archive
                .runtime
                .windows(needle.len())
                .enumerate()
                .filter_map(|(at, data)| (data == needle).then_some(at))
                .collect();
            ensure!(positions.len() == 1, "native variant fixture is not unique");
            let mut writer = ReferenceWriter::new(game.archive.address_shift)?;
            value.write(&mut writer)?;
            let mut body = (writer.data.len() as u32).to_le_bytes().to_vec();
            body.extend(writer.data);
            archive
                .runtime
                .splice(positions[0] + 7..positions[0] + needle.len(), body);
            let path = directory.join("variant.uuc");
            archive.write(File::create(&path)?)?;
            game.import_uuc(&path, &[], &mut resources)?;
            match (&value, &game.vm.variables[&offset]) {
                (NativeValue::Integer(expected), Value::Integer(actual)) => {
                    assert_eq!(expected, actual)
                }
                (NativeValue::Real(expected), Value::Real(actual)) => assert_eq!(expected, actual),
                (NativeValue::Text(expected), Value::Text(actual)) => assert_eq!(expected, actual),
                (NativeValue::Object(None), Value::Null) => {}
                _ => bail!("native variant type did not survive import"),
            }
        }
        assert_eq!(std::fs::read(saves.join("vmr00001.uuc"))?, original);
        game.import_uuc(&saves.join("vmr00001.uuc"), &[], &mut resources)?;
        assert!(game.waiting_for_dialogue());
        assert_eq!(
            game.vm.cursor().map(|c| (c.scene, c.address)),
            Some((483, 0x6dd0))
        );
        drop(game);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}

#[cfg(test)]
mod scratch_xs {
    use super::*;
    #[test]
    #[ignore]
    fn scratch_xs_from_save() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let saves = std::path::PathBuf::from(std::env::var_os("RUGP_TEST_UUC_DIR").context("UUC directory")?);
        let mut game = Game::open(root.as_ref())?;
        let mut resources = AudioResources::new(Arc::clone(&game.archive))?;
        game.import_uuc(&saves.join("vmr00001.uuc"), &[], &mut resources)?;
        let mut audio = None; crate::game::SCRATCH_ARM.store(false, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::var("SCRATCH_OUT")?;
        let dump = |game: &Game, name: &str| -> Result<()> {
            let mut ppm = format!("P6\n{} {}\n255\n", game.frame.width, game.frame.height).into_bytes();
            ppm.extend(game.frame().as_chunks::<4>().0.iter().flat_map(|p| p[..3].iter().copied()));
            std::fs::write(format!("{dir}/{name}.ppm"), ppm)?;
            Ok(())
        };
        let mut ring: Vec<Vec<u8>> = Vec::new();
        for i in 0..2000 {
            {
                let f = game.frame();
                let (w, h) = (game.frame.width as usize, game.frame.height as usize);
                let (mut r, mut g, mut n) = (0u64, 0u64, 0u64);
                for y in h * 3 / 4..h { for x in 0..w / 3 { let o = (y * w + x) * 4; r += f[o] as u64; g += f[o + 1] as u64; n += 1; } }
                if i > 5 && g / n > r / n + 35 { eprintln!("grass at {i}"); for (k, fr) in ring.iter().enumerate() { let mut ppm = format!("P6
{w} {h}
255
").into_bytes(); ppm.extend(fr.as_chunks::<4>().0.iter().flat_map(|p| p[..3].iter().copied())); std::fs::write(format!("{dir}/pt{k:02}.ppm"), ppm)?; } for k in 0..12 { dump(&game, &format!("pu{k:02}"))?; game.step(33)?; } return Ok(()); }
                ring.push(f.to_vec()); if ring.len() > 24 { ring.remove(0); }
            }
            if i % 10 == 0 { eprintln!("it {i} {:?} wait={} dlg={}", game.vm.cursor().map(|c| (c.scene, c.address)), game.waiting_for_input(), game.dialogue_waits_for_input()); dump(&game, &format!("prog{}", i/10))?; }
            if let Some(class) = game.objects.values().find_map(|o| o.properties.get("render.xs_fade.class").cloned()) {
                eprintln!("xs {class:?} at {:?}", game.vm.cursor().map(|c| (c.scene, c.address)));
                for k in 0..6 { dump(&game, &format!("pxs{k}"))?; game.step(300)?; }
                return Ok(());
            }
            if game.dialogue_waits_for_input() || game.waiting_for_input() { game.activate_selection()?; }
            game.skip_video()?;
            game.step(50)?;
            crate::audio::mute_requests(&mut game, &mut audio)?;
        }
        bail!("no xs")
    }
}
