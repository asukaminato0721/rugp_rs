use super::*;
pub(super) struct SelectionWait {
    target: u64,
    result: ObjectRef,
}
impl Game {
    fn ensure_menu(&mut self, reference: &ObjectRef) -> Result<u64> {
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
        self.menus
            .get_mut(&id)
            .unwrap()
            .set_enabled(selection_id, class != "OM_AllDisableSelecter");
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
    fn install_menu_texture(&mut self, target: u64, texture: Arc<Texture>) -> Result<()> {
        let old = self.buffers.get(&target).map_or(0, |t| t.rgba.len());
        let total: usize = self.buffers.values().map(|t| t.rgba.len()).sum();
        ensure!(
            total - old + texture.rgba.len() <= 128 * 1024 * 1024,
            "menu framebuffer budget exceeded"
        );
        self.buffers.insert(target, Arc::clone(&texture));
        self.objects.entry(target).or_default().layers.insert(
            0,
            Layer {
                texture,
                plane: None,
            },
        );
        Ok(())
    }
    fn refresh_menu(&mut self, id: u64, present: bool) -> Result<()> {
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
            self.present_menu_scene()?;
        }
        Ok(())
    }
    fn present_menu_scene(&mut self) -> Result<()> {
        let next = self.scene_frame()?;
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
            self.present_menu_scene()?;
        }
        Ok(())
    }
    pub(super) fn ask_selection(
        &mut self,
        target: u64,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
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
        if let Some(id) = self.selection_menu() {
            self.menus.get_mut(&id).unwrap().navigate(delta.signum());
            self.refresh_menu(id, true)?;
        }
        Ok(())
    }
    pub fn activate_selection(&mut self) -> Result<()> {
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
