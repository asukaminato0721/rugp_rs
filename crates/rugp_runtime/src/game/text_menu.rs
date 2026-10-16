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

    /// Local control-flow overapproximation: every scene start and every label
    /// is a possible entry, both sides of every scalar branch are included,
    /// and calls may always return. Across the complete program this cannot
    /// omit a direct CRsa/CLabel jump destination. It only excludes unlabelled
    /// code after an unconditional jump/return with no other incoming edge.
    fn static_reachable(
        scene: &rugp_scene_vm::Scenario,
    ) -> Result<std::collections::BTreeSet<u32>> {
        use rugp_scene_vm::Argument;
        let instructions = scene
            .instructions
            .iter()
            .map(|ins| (ins.address, ins))
            .collect::<BTreeMap<_, _>>();
        ensure!(
            instructions.len() == scene.instructions.len(),
            "duplicate static PC"
        );
        let mut pending = vec![0];
        pending.extend(
            scene
                .instructions
                .iter()
                .filter_map(|ins| (ins.class.name == "CVmLabel").then_some(ins.address)),
        );
        let mut reached = std::collections::BTreeSet::new();
        while let Some(address) = pending.pop() {
            if address == u32::MAX || !reached.insert(address) {
                continue;
            }
            let ins = instructions
                .get(&address)
                .context("invalid static branch PC")?;
            match ins.class.name.as_str() {
                "CVmRet" => continue,
                "CVmJump" => {
                    ensure!(
                        matches!(ins.arguments.first(), Some(Argument::Reference(r))
                            if r.selector == 0 && r.offset.is_some()
                            && r.class.as_ref().is_some_and(|c| matches!(c.name.as_str(), "CRsa" | "CLabel"))),
                        "unresolved static jump target"
                    );
                    continue; // Its scene/label target is a separate entry.
                }
                "CVmFlagOp" => {
                    let Some(Argument::Number(destination)) = ins.arguments.get(5) else {
                        bail!("unresolved static flag branch target");
                    };
                    pending.push(*destination);
                }
                _ => {}
            }
            pending.push(ins.next_address);
        }
        Ok(reached)
    }

    #[test]
    #[ignore = "set RUGP_TEST_GAME; audits original static story-choice labels and hit rectangles"]
    fn retail_static_text_selector_blocks_fit_native_default_windows() -> Result<()> {
        use rugp_scene_vm::Argument;
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        game.set_scene_rasterization(false);
        if let Some(path) = std::env::var_os("RUGP_TEST_FONT") {
            game.set_font_file(path.as_ref(), 0)?;
        }
        let native = NativeSchema::from_directory(root.as_ref())?;
        let program = Arc::new(Program::load(&game.archive, &native)?);
        let mut blocks = 0usize;
        let mut labels = 0usize;
        let mut unreachable_packets = 0usize;
        for (&id, scene) in &program.scenes {
            let reachable = static_reachable(scene)
                .with_context(|| format!("static local reachability in {id}"))?;
            game.vm = Vm::new(Arc::clone(&program), id)?;
            for instruction in &scene.instructions {
                let Some(Argument::Message { class, .. }) = instruction.arguments.get(1) else {
                    continue;
                };
                if instruction.class.name != "CVmGenericMsg"
                    || !["OM_AddSelecter", "OM_AskUserSelectEx"]
                        .contains(&class.name.trim_start_matches(['&', '-']))
                {
                    continue;
                }
                if !reachable.contains(&instruction.address) {
                    ensure!(
                        id == 242 && instruction.address >= 17868,
                        "unexpected dead selector packet"
                    );
                    if class.name.ends_with("OM_AskUserSelectEx") {
                        let message = game.vm.message(instruction)?;
                        ensure!(
                            number(&message.fields, "スタイル", 0)? == 2,
                            "unexpected unreachable selector style"
                        );
                    }
                    unreachable_packets += 1;
                    continue;
                }
                let message = game.vm.message(instruction)?;
                let Value::Reference(target) = &message.target else {
                    bail!("selection audit target is not an object");
                };
                if !target
                    .class
                    .as_ref()
                    .is_some_and(|c| c.name == "CMsgSelBox")
                {
                    continue; // Image menus have separate retail coverage.
                }
                let target = target.offset.context("text selector target identity")?;
                let asking = message.class == "OM_AskUserSelectEx";
                // Exercise the original Add/Ask fields against real host text
                // rendering and native window defaults. Conditional labels in
                // a static block are included together, conservatively checking
                // size/font coverage; VM branch execution and selection results
                // are covered separately by actual story/replay playback.
                game.message(message).with_context(|| {
                    format!(
                        "static text selector packet at {id}:{}",
                        instruction.address
                    )
                })?;
                if !asking {
                    labels += 1;
                    continue;
                }
                ensure!(game.waiting_for_selection(), "text choice did not wait");
                let menu = &game.text_menus[&target];
                let rect = game.objects[&target].rect;
                ensure!(
                    !menu.entries.is_empty() && menu.hits.len() == menu.entries.len(),
                    "selector label was not laid out"
                );
                ensure!(
                    menu.hits.iter().all(|&(x, y, w, h)| {
                        x >= 0
                            && y >= 0
                            && w > 0
                            && h > 0
                            && i64::from(x) + i64::from(w) <= i64::from(rect.2)
                            && i64::from(y) + i64::from(h) <= i64::from(rect.3)
                    }),
                    "selector hit rectangle escaped native window"
                );
                blocks += 1;
                // Isolate the next static packet block, without inventing a
                // user selection, acknowledging a VM instruction or setting
                // the script's result variable.
                game.selection = None;
                game.text_menus.remove(&target);
            }
            ensure!(
                game.text_menus.is_empty(),
                "unfinished selector block in {id}"
            );
        }
        ensure!(
            (blocks, labels, unreachable_packets) == (132, 273, 40),
            "incomplete retail text selector inventory"
        );
        eprintln!(
            "static text selector audit finished: blocks={blocks}, labels={labels}, unreachable_packets={unreachable_packets}"
        );
        Ok(())
    }

    #[test]
    fn static_choice_audit_includes_branch_sides_labels_and_call_returns_but_not_dead_suffixes()
    -> Result<()> {
        use rugp_assets::object::ClassId;
        use rugp_scene_vm::{Argument, Scenario};
        let target = |class: &str, offset| {
            Argument::Reference(ObjectRef {
                class: Some(ClassId {
                    name: class.into(),
                    schema: 0,
                }),
                cache_index: None,
                flags: 0xc108,
                offset: Some(offset),
                size: Some(4),
                owner: None,
                selector: 0,
                inline: None,
            })
        };
        let packet = |class: &str, address, next_address, arguments| Instruction {
            index: address,
            stream_offset: 0,
            class: ClassId {
                name: class.into(),
                schema: 0,
            },
            address,
            next_address,
            flags: 0,
            arguments,
        };
        let mut branch = vec![Argument::Number(0); 6];
        branch[5] = Argument::Number(30);
        let mut scene = Scenario {
            object_id: 1,
            memory_size: 70,
            instructions: vec![
                packet("CVmFlagOp", 0, 10, branch),
                packet("CVmJump", 10, 20, vec![target("CRsa", 1000)]),
                packet("CVmGenericMsg", 20, 30, vec![]),
                packet("CVmRet", 30, u32::MAX, vec![]),
                packet("CVmLabel", 40, 50, vec![target("CLabel", 2000)]),
                packet("CVmCall", 50, 60, vec![target("CRsa", 3000)]),
                packet("CVmRet", 60, u32::MAX, vec![]),
            ],
            null_records: vec![],
            trailing_bytes: 0,
        };
        assert_eq!(static_reachable(&scene)?, [0, 10, 30, 40, 50, 60].into());
        scene.instructions[0].arguments[5] = Argument::Number(999);
        assert!(static_reachable(&scene).is_err());
        scene.instructions[0].arguments[5] = Argument::Value {
            descriptor: 0,
            arguments: vec![],
        };
        assert!(static_reachable(&scene).is_err());
        Ok(())
    }

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
