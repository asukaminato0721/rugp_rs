use super::*;
pub(super) struct SelectionWait {
    pub(super) target: u64,
    pub(super) result: ObjectRef,
}

impl Game {
    pub(super) fn ensure_menu(&mut self, reference: &ObjectRef) -> Result<u64> {
        let id = reference
            .offset
            .context("menu reference has no stored identity")?;
        if !self.menus.contains_key(&id) {
            let menu = crate::menu::MenuState::load(&self.archive, reference)?;
            ensure!(
                self.menus.values().map(|m| m.mask_bytes()).sum::<usize>() + menu.mask_bytes()
                    <= 64 * 1024 * 1024,
                "menu hit-map budget exceeded"
            );
            self.menus.insert(id, menu);
        }
        Ok(id)
    }
    pub(super) fn configure_menu(
        &mut self,
        reference: &ObjectRef,
        class: &str,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let id = self.ensure_menu(reference)?;
        let selection_id = if class == "OM_EnableSelecter" {
            let Some(Value::Text(name)) = fields.get("選択肢名") else {
                bail!("menu selection name is not text");
            };
            Some(super::profile::atoi(name) as u32)
        } else {
            None
        };
        let menu = self.menus.get_mut(&id).unwrap();
        if class == "OM_AllIgnoreSelecter" {
            menu.ignore_all();
        } else {
            menu.set_enabled(selection_id, class != "OM_AllDisableSelecter");
        }
        self.refresh_menu(id, false)
    }
    fn menu_texture(&mut self, id: u64) -> Result<Arc<Texture>> {
        Ok(Arc::new(
            self.menus
                .get(&id)
                .context("menu not loaded")?
                .compose(&mut self.images)?,
        ))
    }
    pub(super) fn install_menu_texture(
        &mut self,
        target: u64,
        texture: Arc<Texture>,
    ) -> Result<()> {
        let old = self.buffers.get(&target).map_or(0, |t| t.rgba.len());
        let total: usize = self.buffers.values().map(|t| t.rgba.len()).sum();
        ensure!(
            total - old + texture.rgba.len() <= 128 * 1024 * 1024,
            "menu framebuffer budget exceeded"
        );
        self.buffers.insert(target, Arc::clone(&texture));
        self.objects
            .entry(target)
            .or_default()
            .layers
            .insert(0, Layer::Screen(texture));
        Ok(())
    }
    pub(super) fn refresh_menu(&mut self, id: u64, present: bool) -> Result<()> {
        let targets: Vec<_> = self
            .objects
            .iter()
            .filter(|(_, o)| o.menu == Some(id))
            .map(|(&id, _)| id)
            .collect();
        if targets.is_empty() {
            return Ok(());
        }
        let texture = self.menu_texture(id)?;
        for target in targets {
            self.install_menu_texture(target, Arc::clone(&texture))?;
        }
        if present {
            self.present_scene()?;
        }
        Ok(())
    }
    pub(super) fn present_scene(&mut self) -> Result<()> {
        for object in self.objects.values_mut() {
            object.properties.remove("character.angles_pending");
        }
        let next = self.scene_frame()?;
        if let Some(parse) = &mut self.parse_transition {
            // An explicit presentation (e.g. live dialogue) refreshes the
            // captured scene, including projection caches updated by rendering;
            // ordinary staged image commands do not.
            parse.scene = self.objects.clone();
        }
        if let Some(transition) = &mut self.transition {
            transition.to = next.rgba;
            self.update_frame();
        } else {
            self.frame = next;
        }
        Ok(())
    }
    pub(super) fn load_image_area(
        &mut self,
        target: u64,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        ensure!(
            number(fields, "Ｘ座標", 0)? == 0 && number(fields, "Ｙ座標", 0)? == 0,
            "offset image-area loading is not implemented"
        );
        let immediate = number(fields, "スクリーンへすぐに反映？", 0)?;
        ensure!(
            [0, 1].contains(&immediate),
            "invalid immediate image-area flag"
        );
        let Some(Value::Reference(image)) = fields.get("画像") else {
            bail!("image-area source is not a reference");
        };
        let is_menu = image.class.as_ref().is_some_and(|c| c.name == "CRbx");
        let (menu, texture) = if is_menu {
            let id = self.ensure_menu(image)?;
            (Some(id), self.menu_texture(id)?)
        } else {
            (None, self.images.load(image)?)
        };
        let object = self.objects.entry(target).or_default();
        object.layers.clear();
        object.menu = menu;
        self.install_menu_texture(target, texture)?;
        if immediate != 0 {
            self.present_scene()?;
        }
        Ok(())
    }
    pub(super) fn ask_selection(
        &mut self,
        target: u64,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        ensure!(
            self.dialogue.is_none() && self.selection.is_none(),
            "overlapping dialogue/menu waits"
        );
        ensure!(
            number(fields, "スタイル", 0)? == 0,
            "unsupported image-menu selection style"
        );
        ensure!(
            matches!(
                fields.get("結果を示すオブジェクト"),
                None | Some(Value::Null)
            ),
            "object-valued selection output is not implemented"
        );
        let Some(Value::Reference(result)) = fields.get("結果の識別値") else {
            bail!("menu selection result has no destination");
        };
        ensure!(
            result.class.as_ref().is_some_and(|c| c.name == "CInt"),
            "unsupported menu result type"
        );
        if let Some(menu) = self.text_menus.get(&target) {
            ensure!(!menu.entries.is_empty(), "empty text selection");
            self.selection = Some(SelectionWait {
                target,
                result: result.clone(),
            });
            return self.refresh_text_menu(target);
        }
        let id = self
            .objects
            .get(&target)
            .and_then(|o| o.menu)
            .context("selection target has no loaded image menu")?;
        self.selection = Some(SelectionWait {
            target,
            result: result.clone(),
        });
        self.refresh_menu(id, true)
    }
    pub fn waiting_for_selection(&self) -> bool {
        self.selection.is_some()
    }
    fn selection_menu(&self) -> Option<u64> {
        self.selection
            .as_ref()
            .and_then(|s| self.objects.get(&s.target))
            .and_then(|o| o.menu)
    }
    /// Pointer positions are in the game's logical framebuffer coordinates.
    pub fn pointer_move(&mut self, x: i32, y: i32) -> Result<()> {
        self.pointer_inside =
            x >= 0 && y >= 0 && x < self.frame.width as i32 && y < self.frame.height as i32;
        if let Some(target) = self.selection_text_target() {
            let rect = self.objects[&target].rect;
            let menu = self.text_menus.get_mut(&target).unwrap();
            let hit = self
                .pointer_inside
                .then(|| x.checked_sub(rect.0).zip(y.checked_sub(rect.1)))
                .flatten()
                .and_then(|(x, y)| menu.hit(x, y));
            if menu.hover != hit {
                menu.hover = hit;
                self.refresh_text_menu(target)?;
            }
            return Ok(());
        }
        let Some(id) = self.selection_menu() else {
            return Ok(());
        };
        let target = self.selection.as_ref().unwrap().target;
        let rect = self.objects[&target].rect;
        let menu = self.menus.get_mut(&id).unwrap();
        let hit = if x >= rect.0
            && y >= rect.1
            && i64::from(x) < i64::from(rect.0) + i64::from(rect.2)
            && i64::from(y) < i64::from(rect.1) + i64::from(rect.3)
            && rect.2 > 0
            && rect.3 > 0
        {
            menu.hit(
                ((i64::from(x) - i64::from(rect.0)) * i64::from(menu.data.width)
                    / i64::from(rect.2)) as i32,
                ((i64::from(y) - i64::from(rect.1)) * i64::from(menu.data.height)
                    / i64::from(rect.3)) as i32,
            )
        } else {
            None
        };
        if hit != menu.hover {
            menu.hover = hit;
            self.refresh_menu(id, true)?;
        }
        Ok(())
    }
    pub fn pointer_button(&mut self, pressed: bool) -> Result<()> {
        let armed = if pressed {
            self.pointer_armed = true;
            true
        } else {
            std::mem::take(&mut self.pointer_armed)
        };
        if !pressed && !armed {
            self.dialogue_pressed = false;
            if let Some(target) = self.selection_text_target() {
                self.text_menus.get_mut(&target).unwrap().pressed = None;
                self.refresh_text_menu(target)?;
            } else if let Some(id) = self.selection_menu() {
                self.menus.get_mut(&id).unwrap().pressed = None;
                self.refresh_menu(id, true)?;
            }
            return Ok(());
        }
        if self
            .video
            .as_ref()
            .is_some_and(|(_, player)| !player.finished())
        {
            if pressed {
                self.dialogue_pressed = self.pointer_inside;
            } else if std::mem::take(&mut self.dialogue_pressed) && self.pointer_inside {
                self.skip_video()?;
            }
            return Ok(());
        }
        if self.input_wait.is_some() {
            if pressed {
                self.dialogue_pressed = self.pointer_inside;
            } else if std::mem::take(&mut self.dialogue_pressed) && self.pointer_inside {
                self.finish_input_wait()?;
            }
            return Ok(());
        }
        if self.dialogue.is_some() {
            if pressed {
                self.dialogue_pressed = self.pointer_inside;
            } else if std::mem::take(&mut self.dialogue_pressed) && self.pointer_inside {
                self.advance_dialogue()?;
            }
            return Ok(());
        }
        if let Some(target) = self.selection_text_target() {
            let menu = self.text_menus.get_mut(&target).unwrap();
            if pressed {
                menu.pressed = menu.hover;
                self.refresh_text_menu(target)?;
            } else {
                let key = menu.pressed.take().filter(|key| Some(*key) == menu.hover);
                if let Some(key) = key {
                    self.finish_text_selection(target, key)?;
                } else {
                    self.refresh_text_menu(target)?;
                }
            }
            return Ok(());
        }
        let Some(id) = self.selection_menu() else {
            return Ok(());
        };
        let menu = self.menus.get_mut(&id).unwrap();
        if pressed {
            menu.pressed = menu.hover;
            self.refresh_menu(id, true)?;
        } else {
            let key = menu.pressed.take().filter(|key| Some(*key) == menu.hover);
            self.refresh_menu(id, true)?;
            if let Some(key) = key {
                self.finish_selection(id, key)?;
            }
        }
        Ok(())
    }
    pub fn navigate_selection(&mut self, delta: i32) -> Result<()> {
        if let Some(target) = self.selection_text_target() {
            let menu = self.text_menus.get_mut(&target).unwrap();
            menu.navigate(delta);
            menu.pressed = None;
            return self.refresh_text_menu(target);
        }
        if let Some(id) = self.selection_menu() {
            self.menus.get_mut(&id).unwrap().navigate(delta.signum());
            self.refresh_menu(id, true)?;
        }
        Ok(())
    }
    pub fn activate_selection(&mut self) -> Result<()> {
        if self.skip_video()? {
            return Ok(());
        }
        if self.input_wait.is_some() {
            return self.finish_input_wait();
        }
        if self.dialogue.is_some() {
            return self.advance_dialogue();
        }
        if let Some(target) = self.selection_text_target() {
            if let Some(key) = self.text_menus[&target].hover {
                self.finish_text_selection(target, key)?;
            }
            return Ok(());
        }
        if let Some(id) = self.selection_menu()
            && let Some(key) = self.menus[&id].hover
        {
            self.finish_selection(id, key)?;
        }
        Ok(())
    }
    fn finish_selection(&mut self, id: u64, key: u32) -> Result<()> {
        let Some(result) = self.menus[&id].result(key) else {
            return Ok(());
        };
        let destination = self
            .selection
            .as_ref()
            .context("no pending selection")?
            .result
            .clone();
        self.vm
            .write_object(&destination, Value::Integer(result as i32))?;
        self.vm.acknowledge_host()?;
        self.selection = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugp_assets::script::Script;
    use rugp_scene_vm::Scenario;
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_ignore_all_hides_selector_states_without_discarding_menu_resources() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let scene = Scenario::decode_with_schema(
            &Script::load(&game.archive, 239)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let instruction = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x6c)
            .context("ignore-all packet")?;
        let message = game.vm.message(instruction)?;
        let Value::Reference(reference) = &message.target else {
            bail!("menu target missing");
        };
        let id = game.ensure_menu(reference)?;
        let menu = game.menus.get_mut(&id).unwrap();
        let original = menu.data.selectors.clone();
        let first = original
            .iter()
            .find(|s| s.id != 0)
            .context("selector missing")?;
        menu.hover = Some(first.header);
        menu.pressed = Some(first.header);
        let before = game.frame().to_vec();
        game.message(message)?;
        assert_eq!(game.frame(), before); // staged, no implicit presentation
        let menu = &game.menus[&id];
        assert_eq!(menu.hover, None);
        assert_eq!(menu.pressed, None);
        for (old, new) in original.iter().zip(&menu.data.selectors) {
            assert_eq!(new.status, (old.status & !3) | 4);
            assert_eq!(new.id, old.id);
            assert_eq!(new.flags, old.flags);
            assert!(menu.result(new.header).is_none());
            for (a, b) in old.images.iter().zip(&new.images) {
                assert_eq!(a.offset, b.offset);
                assert_eq!(a.size, b.size);
            }
        }
        let mut expected =
            Framebuffer::new(u32::from(menu.data.width), u32::from(menu.data.height))?;
        let source = &menu.data.overlay;
        if source.offset.is_some() || source.inline.is_some() {
            let texture = game.images.load(source)?;
            expected.blit(&texture, (0, 0, expected.width, expected.height), 255)?;
        }
        let surface = menu.compose(&mut game.images)?;
        assert_eq!(surface.rgba, expected.rgba);
        let menu = game.menus.get_mut(&id).unwrap();
        menu.set_enabled(None, true);
        assert_eq!(menu.result(first.header), Some(first.id));
        menu.set_enabled(None, false);
        menu.set_enabled(Some(first.id), true);
        assert_eq!(menu.result(first.header), Some(first.id));
        Ok(())
    }
}
