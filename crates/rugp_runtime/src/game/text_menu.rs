//! Story text selectors. Native selection ordinals are zero-based (Vm60
//! 0x1002b244); confirming consumes the list (0x1002b9b5). Presentation uses
//! portable outlined text, not the native markup/highlight renderer.
use super::*;

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TextMenu {
    pub entries: Vec<String>,
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    pub hits: Vec<(i32, i32, u32, u32)>,
}
impl TextMenu {
    pub fn navigate(&mut self, delta: i32) {
        if self.entries.is_empty() || delta == 0 {
            return;
        }
        self.hover = Some(match self.hover {
            Some(index) => {
                (index as i32 + delta.signum()).rem_euclid(self.entries.len() as i32) as usize
            }
            None if delta > 0 => 0,
            None => self.entries.len() - 1,
        });
    }
    pub fn hit(&self, x: i32, y: i32) -> Option<usize> {
        self.hits.iter().position(|&(left, top, w, h)| {
            x >= left
                && y >= top
                && i64::from(x) < i64::from(left) + i64::from(w)
                && i64::from(y) < i64::from(top) + i64::from(h)
        })
    }
}
impl Game {
    pub(super) fn add_text_selector(
        &mut self,
        target: u64,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        ensure!(
            self.selection.is_none(),
            "cannot add selectors during a selection wait"
        );
        let Some(Value::Text(text)) = fields.get("項目文章") else {
            bail!("text selector label is not text");
        };
        ensure!(
            !text.is_empty() && text.len() <= 16 * 1024,
            "invalid text selector label size"
        );
        ensure!(
            text.chars().all(|c| !c.is_control() || c == '\n'),
            "text selector markup controls are not implemented"
        );
        ensure!(
            matches!(fields.get("選択肢ＩＤ"), None | Some(Value::Null)),
            "object-valued text selector IDs are not implemented"
        );
        let menu = self.text_menus.entry(target).or_default();
        ensure!(menu.entries.len() < 256, "too many text selectors");
        menu.entries.push(text.clone());
        Ok(())
    }
    pub(super) fn selection_text_target(&self) -> Option<u64> {
        self.selection
            .as_ref()
            .map(|s| s.target)
            .filter(|id| self.text_menus.contains_key(id))
    }
    pub(super) fn refresh_text_menu(&mut self, target: u64) -> Result<()> {
        self.ensure_text_font()?;
        let rect = self.objects[&target].rect;
        let menu = self
            .text_menus
            .get_mut(&target)
            .context("text menu missing")?;
        let (texture, hits) = self.message_windows[&target].compose_selectors(
            self.font.as_mut().unwrap(),
            rect.2,
            rect.3,
            &menu.entries,
            menu.hover,
            menu.pressed,
        )?;
        menu.hits = hits;
        let object = self.objects.get_mut(&target).unwrap();
        object.visible = true;
        object.backdrop = Some(self.message_config.as_ref().unwrap().backdrop);
        self.install_menu_texture(target, Arc::new(texture))?;
        self.present_scene()
    }
    pub(super) fn finish_text_selection(&mut self, target: u64, index: usize) -> Result<()> {
        ensure!(
            index < self.text_menus[&target].entries.len(),
            "invalid text selection index"
        );
        // Restore the retained page rather than leaving stale choice pixels
        // under later dialogue. A completed native selector list is consumed.
        let rect = self.objects[&target].rect;
        let window = &self.message_windows[&target];
        let texture = window.compose(self.font.as_mut().unwrap(), rect.2, rect.3, &window.page)?;
        let result = self
            .selection
            .as_ref()
            .context("no pending selection")?
            .result
            .clone();
        self.vm
            .write_object(&result, Value::Integer(index as i32))?;
        self.vm.acknowledge_host()?;
        self.selection = None;
        self.text_menus.remove(&target);
        self.install_menu_texture(target, Arc::new(texture))?;
        self.present_scene()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_text_selection_waits_for_input_and_returns_each_branch() -> Result<()> {
        use rugp_assets::{object::ClassId, script::Script};
        use rugp_scene_vm::{Argument, Scenario};
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let script = Script::load(&game.archive, 470)?;
        let scene =
            Scenario::decode_with_schema(&script, game.archive.address_shift, Some(&native))?;
        let mut instructions: Vec<_> = scene
            .instructions
            .into_iter()
            .filter(|i| [0x5dd4, 0x5df0, 0x5e0c].contains(&i.address))
            .collect();
        assert_eq!(instructions.len(), 3);
        for (index, instruction) in instructions.iter_mut().enumerate() {
            instruction.address = index as u32 * 28;
            instruction.next_address = (index as u32 + 1) * 28;
            if index < 2 {
                let Argument::Message { arguments, .. } = &mut instruction.arguments[1] else {
                    bail!("missing retail selector message");
                };
                for field in arguments {
                    if let Argument::Field { name, argument, .. } = field
                        && name == "項目文章"
                    {
                        **argument = Argument::Text(format!("Synthetic choice {index}"));
                    }
                }
            }
        }
        instructions.push(Instruction {
            index: 3,
            stream_offset: 0,
            class: ClassId {
                name: "CVmRet".into(),
                schema: 17,
            },
            address: 84,
            next_address: u32::MAX,
            flags: 0,
            arguments: vec![],
        });
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 88,
                instructions,
                null_records: vec![],
                trailing_bytes: 0,
            },
        )?;
        let program = Arc::new(program);
        for branch in [0, 1] {
            game.vm = Vm::new(Arc::clone(&program), 1)?;
            game.ended = false;
            assert_eq!(game.step(0)?, RunState::Waiting);
            let target = game
                .selection_text_target()
                .context("text selection not waiting")?;
            let result = game.selection.as_ref().unwrap().result.clone();
            assert_eq!(game.text_menus[&target].entries.len(), 2);
            assert_eq!(game.step(100000)?, RunState::Waiting);
            assert_eq!(game.vm.cursor().unwrap().address, 56);
            super::ui_snapshot::test_roundtrip(&mut game)?;
            game.activate_selection()?; // No hover: no implicit default choice.
            assert!(game.waiting_for_selection());
            if branch == 0 {
                let frame = game.frame().to_vec();
                game.navigate_selection(1)?;
                assert_ne!(game.frame(), frame);
                super::ui_snapshot::test_roundtrip(&mut game)?;
                game.activate_selection()?;
            } else {
                let rect = game.objects[&target].rect;
                let second = game.text_menus[&target].hits[1];
                let (x, y) = (rect.0 + second.0 + 1, rect.1 + second.1 + 1);
                game.pointer_move(x, y)?;
                game.pointer_button(false)?; // Release without press does nothing.
                assert!(game.waiting_for_selection());
                game.pointer_button(true)?;
                game.pointer_move(-1, -1)?;
                game.pointer_button(false)?; // Drag outside cancels the click.
                assert!(game.waiting_for_selection());
                game.pointer_move(x, y)?;
                game.pointer_button(true)?;
                game.pointer_button(false)?;
            }
            assert!(!game.waiting_for_selection());
            assert!(game.text_menus.is_empty());
            assert!(
                matches!(game.vm.evaluate(&Argument::Reference(result))?, Value::Integer(value) if value == branch)
            );
            assert_eq!(game.step(0)?, RunState::Ended);
        }
        Ok(())
    }
    #[test]
    fn navigation_and_hit_rectangles_use_zero_based_entries() {
        let mut menu = TextMenu {
            entries: vec!["first".into(), "second".into()],
            hits: vec![(5, 10, 100, 30), (5, 45, 100, 30)],
            ..TextMenu::default()
        };
        assert_eq!(menu.hit(5, 10), Some(0));
        assert_eq!(menu.hit(104, 74), Some(1));
        assert_eq!(menu.hit(105, 74), None);
        assert_eq!(menu.hit(5, 40), None);
        assert_eq!(menu.hit(-1, 10), None);
        menu.navigate(0);
        assert_eq!(menu.hover, None);
        menu.navigate(1);
        assert_eq!(menu.hover, Some(0));
        menu.navigate(-1);
        assert_eq!(menu.hover, Some(1));
        menu.navigate(1);
        assert_eq!(menu.hover, Some(0));
        menu.navigate(1);
        assert_eq!(menu.hover, Some(1));
    }
}
