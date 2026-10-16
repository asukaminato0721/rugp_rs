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
                _ => None,
            };
            if let Some(value) = value {
                scalars.insert(*offset, value);
            }
        }
        let mut replay = Game::open(directory)?;
        replay.font_file = self.font_file.clone();
        let mut audio = None;
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
    #[ignore = "set RUGP_TEST_GAME and RUGP_TEST_UUC_DIR (contains vmr00001.uuc and vmr00002.uuc)"]
    fn retail_uuc_load_continuation_and_failed_import_are_atomic() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let saves = PathBuf::from(std::env::var_os("RUGP_TEST_UUC_DIR").context("UUC directory")?);
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
            let mut audio = None;
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
