use crate::{
    image_manager::{ImageManager, WorldImage},
    render::{Framebuffer, Texture},
    world::Camera,
};
use anyhow::{Context, Result, bail, ensure};
use rugp_assets::{RioArchive, fade::FadeCurve, native_schema::NativeSchema, object::ObjectRef};
use rugp_scene_vm::{
    Instruction,
    vm::{Message, Program, Step, Value, Vm},
};
use std::{cell::Cell, collections::BTreeMap, path::Path, sync::Arc};

mod blit;
mod camera;
mod character;
mod dialogue;
mod menu;
mod parse_screen;
mod posture;
mod profile;
mod sound;
mod var_data;
mod video;
#[cfg(test)]
mod world_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Running,
    Waiting,
    Ended,
}
#[derive(Debug)]
pub struct SoundRequest {
    /// Runtime playback identity, present for play operations only.
    pub ticket: Option<u64>,
    pub source: Option<ObjectRef>,
    pub kind: u8,
    pub flags: u8,
    /// Kind 4: initial gain, final gain (4096 = unity), transition milliseconds.
    /// Ordinary play (kind 2) uses source defaults instead.
    pub parameters: [u16; 3],
}
#[derive(Clone)]
enum Layer {
    Screen(Arc<Texture>),
    World(WorldImage),
}
#[derive(Clone)]
pub struct ObjectState {
    pub enabled: bool,
    pub visible: bool,
    pub input: bool,
    pub sprite: bool,
    pub auto_update: bool,
    pub rect: (i32, i32, u32, u32),
    pub properties: BTreeMap<String, Value>,
    pub camera: Camera,
    /// Native camera command style 0x800 records a start without applying it.
    camera_start: Option<Camera>,
    pending_camera: Option<crate::world::CameraTween>,
    postures: Vec<posture::PostureInstance>,
    /// Native first slot's cached projected center (+56), updated on render.
    camera_center_y: Cell<i16>,
    pub world_layers: crate::world::WorldLayerFlags,
    pub opacity: u8,
    pending_draw_ratio: Option<rugp_assets::mime::MimeCurve>,
    menu: Option<u64>,
    backdrop: Option<crate::message_window::Backdrop>,
    layers: BTreeMap<i32, Layer>,
    character: character::CharacterState,
}
impl Default for ObjectState {
    fn default() -> Self {
        Self {
            enabled: true,
            visible: true,
            input: false,
            sprite: false,
            auto_update: true,
            rect: (0, 0, 1024, 600),
            properties: BTreeMap::new(),
            camera: Camera::default(),
            camera_start: None,
            pending_camera: None,
            postures: Vec::new(),
            camera_center_y: Cell::new(0),
            world_layers: crate::world::WorldLayerFlags::default(),
            opacity: 255,
            pending_draw_ratio: None,
            menu: None,
            backdrop: None,
            layers: BTreeMap::new(),
            character: character::CharacterState::default(),
        }
    }
}
impl ObjectState {
    pub fn has_character_image(&self) -> bool {
        self.character.has_image()
    }
    pub fn active_posture_count(&self) -> usize {
        self.postures.len()
    }
    fn render(&self, frame: &mut Framebuffer) -> Result<()> {
        if let Some(backdrop) = self.backdrop {
            backdrop.paint(frame, self.rect);
        }
        for layer in self.layers.values() {
            match layer {
                Layer::World(image) => {
                    if let Some(first) = image.planes.first() {
                        self.camera_center_y.set(
                            first
                                .plane
                                .projected_camera_center_y(&self.camera, self.rect)?,
                        );
                    }
                    for layer in &image.planes {
                        if layer.draw_flags & 2 != 0 {
                            continue;
                        }
                        frame.blit_projective(
                            &layer.texture,
                            layer.plane.quad(&self.camera, self.rect)?,
                            self.rect,
                            self.opacity,
                        )?;
                    }
                }
                Layer::Screen(texture) => frame.blit(texture, self.rect, self.opacity)?,
            }
        }
        self.character.render(frame, self.rect, self.opacity)?;
        Ok(())
    }
}
struct Transition {
    from: Vec<u8>,
    to: Vec<u8>,
    elapsed: u32,
    duration: u32,
    curve: Option<FadeCurve>,
}
pub struct Game {
    pub vm: Vm,
    pub archive: Arc<RioArchive>,
    pub objects: BTreeMap<u64, ObjectState>,
    images: ImageManager,
    frame: Framebuffer,
    primary_buffer: u64,
    buffers: BTreeMap<u64, Arc<Texture>>,
    transition: Option<Transition>,
    parse_transition: Option<parse_screen::ParseTransition>,
    /// A synchronous OM_ParseScreenTrigger remains pending on the VM.
    parse_wait: bool,
    wait_ms: Option<u32>,
    audio_wait: Option<Vec<u64>>,
    posture_wait: Option<posture::PostureWait>,
    audio_state: crate::audio_state::PlaybackRegistry,
    sounds: Vec<SoundRequest>,
    video: Option<(u64, crate::video::VideoPlayer)>,
    video_wait: bool,
    profiles: crate::profile_store::ProfileStore,
    menus: BTreeMap<u64, crate::menu::MenuState>,
    selection: Option<menu::SelectionWait>,
    dialogue: Option<dialogue::DialogueWait>,
    message_windows: BTreeMap<u64, crate::message_window::MessageWindow>,
    message_config: Option<crate::message_window::MessageConfig>,
    font: Option<crate::font::TextFont>,
    font_file: Option<(std::path::PathBuf, u32)>,
    pointer_inside: bool,
    dialogue_pressed: bool,
    pub navigation: String,
    pub settings: BTreeMap<(String, i32), BTreeMap<String, Value>>,
    ended: bool,
}
fn number(fields: &BTreeMap<String, Value>, name: &str, default: i32) -> Result<i32> {
    fields.get(name).map_or(Ok(default), Value::integer)
}
impl Game {
    pub fn open(root: &Path) -> Result<Self> {
        let archive = Arc::new(RioArchive::open(root)?);
        let directory = archive.volumes[0]
            .path
            .parent()
            .context("game directory missing")?;
        let native = NativeSchema::from_directory(directory)?;
        let program = Program::load(&archive, &native)?;
        let entry = program.startup(&archive)?;
        let vm = Vm::new(Arc::new(program), entry)?;
        let buffers: Vec<_> = archive
            .nodes
            .iter()
            .filter(|n| n.class.name == "CFrameBuffer")
            .collect();
        ensure!(buffers.len() == 1, "expected one primary framebuffer");
        let primary_buffer = buffers[0].offset.context("framebuffer has no extent")?;
        Ok(Self {
            images: ImageManager::new(Arc::clone(&archive)),
            archive,
            vm,
            objects: BTreeMap::new(),
            frame: Framebuffer::new(1024, 600)?,
            primary_buffer,
            buffers: BTreeMap::new(),
            transition: None,
            parse_transition: None,
            parse_wait: false,
            wait_ms: None,
            audio_wait: None,
            posture_wait: None,
            audio_state: crate::audio_state::PlaybackRegistry::default(),
            sounds: Vec::new(),
            video: None,
            video_wait: false,
            profiles: crate::profile_store::ProfileStore::default(),
            menus: BTreeMap::new(),
            selection: None,
            dialogue: None,
            message_windows: BTreeMap::new(),
            message_config: None,
            font: None,
            font_file: None,
            pointer_inside: false,
            dialogue_pressed: false,
            navigation: String::new(),
            settings: BTreeMap::new(),
            ended: false,
        })
    }
    pub fn size(&self) -> (u32, u32) {
        (self.frame.width, self.frame.height)
    }
    /// Desktop sessions use a per-game user-data folder. The ordinary `open`
    /// remains volatile for deterministic diagnostics and tests.
    pub fn open_persistent(root: &Path, data_root: &Path) -> Result<Self> {
        use sha1::{Digest, Sha1};
        let mut game = Self::open(root)?;
        let mut digest = Sha1::new();
        digest.update(game.archive.manifest.title.as_bytes());
        digest.update(&game.archive.project_data);
        for volume in &game.archive.volumes {
            digest.update(volume.size.to_le_bytes());
        }
        let data_root = if data_root.is_absolute() {
            data_root.to_owned()
        } else {
            std::env::current_dir()?.join(data_root)
        };
        let directory = data_root.join(format!("{:x}", digest.finalize()));
        let original = game.archive.volumes[0]
            .path
            .parent()
            .context("game root missing")?
            .canonicalize()?;
        let mut ancestor = directory.as_path();
        while !ancestor.exists() {
            ancestor = ancestor.parent().context("invalid user-data directory")?;
        }
        ensure!(
            !ancestor.canonicalize()?.starts_with(&original),
            "user data must be outside the original game directory"
        );
        game.profiles = crate::profile_store::ProfileStore::persistent(&directory)?;
        Ok(game)
    }
    pub fn user_data_dir(&self) -> Option<&Path> {
        self.profiles.directory()
    }
    pub fn frame(&self) -> &[u8] {
        &self.frame.rgba
    }
    pub fn video(&self) -> Option<&crate::video::VideoPlayer> {
        self.video.as_ref().map(|(_, player)| player)
    }
    pub fn title(&self) -> &str {
        &self.archive.manifest.title
    }
    pub fn take_sounds(&mut self) -> Vec<SoundRequest> {
        std::mem::take(&mut self.sounds)
    }
    /// Inspect queued requests without consuming playback tickets.
    pub fn pending_sounds(&self) -> &[SoundRequest] {
        &self.sounds
    }
    pub fn finish_audio_ticket(&mut self, ticket: u64) {
        self.audio_state.finish(ticket);
    }
    pub fn time_audio_ticket(&mut self, ticket: u64, duration_ms: u32) {
        self.audio_state.set_duration(ticket, duration_ms);
    }
    fn scene_frame(&self) -> Result<Framebuffer> {
        Self::render_objects(&self.objects, self.size())
    }
    fn render_objects(
        objects: &BTreeMap<u64, ObjectState>,
        size: (u32, u32),
    ) -> Result<Framebuffer> {
        let mut frame = Framebuffer::new(size.0, size.1)?;
        // Message-window backdrops/text stay above standing portraits, even
        // when the character's serialized object address sorts after the UI.
        for object in objects
            .values()
            .filter(|o| o.visible && o.backdrop.is_none())
            .chain(
                objects
                    .values()
                    .filter(|o| o.visible && o.backdrop.is_some()),
            )
        {
            object.render(&mut frame)?;
        }
        Ok(frame)
    }
    fn update_frame(&mut self) {
        if let Some(transition) = &self.transition {
            let (a, d) = transition.curve.as_ref().map_or_else(
                || {
                    (
                        transition.elapsed.min(transition.duration) as u64,
                        transition.duration as u64,
                    )
                },
                |curve| {
                    (
                        u64::from(curve.alpha(transition.elapsed, transition.duration)),
                        256,
                    )
                },
            );
            for (out, (from, to)) in self
                .frame
                .rgba
                .iter_mut()
                .zip(transition.from.iter().zip(&transition.to))
            {
                *out = ((*from as u64 * (d - a) + *to as u64 * a + d / 2) / d) as u8;
            }
            if transition.elapsed >= transition.duration {
                let parse = transition.curve.is_none();
                self.transition = None;
                if parse {
                    for object in self.objects.values_mut() {
                        object.camera_start = None;
                    }
                }
            }
        }
    }
    fn message(&mut self, message: Message) -> Result<()> {
        let fields = message.fields;
        if message.class == "OM_SetUucNavigateString" {
            let Some(Value::Text(text)) = fields.get("現在状況説明文") else {
                bail!("invalid navigation text");
            };
            self.navigation = text.clone();
            return Ok(());
        }
        if message.class == "OM_ChangeSetting" {
            let Some(Value::Text(key)) = fields.get("キー") else {
                bail!("setting key is not text");
            };
            let kind = number(&fields, "タイプ", 0)?;
            self.settings.insert((key.clone(), kind), fields);
            return Ok(());
        }
        let Value::Reference(target) = message.target else {
            bail!("message target is not an object");
        };
        let id = target
            .offset
            .context("message target is not bound to a stored object")?;
        if target
            .class
            .as_ref()
            .is_some_and(|c| c.name == "CMsgSelBox")
        {
            self.ensure_message_window(&target)?;
        }
        match message.class.as_str() {
            "OM_QueueFace2Data" => self.queue_character_face(id, &fields)?,
            "OM_SetLayerData2D" => self.set_character_layer(id, &fields)?,
            "OM_PlayMedia"
                if target
                    .class
                    .as_ref()
                    .is_some_and(|c| c.name == "CLayerSimpleFlush") =>
            {
                // Omit this visual-only layer effect, with an instantaneous
                // completion event. It must not leave a subsequent sync stuck.
                self.objects
                    .entry(id)
                    .or_default()
                    .properties
                    .extend(fields);
            }
            "OM_AddPostureMomentum" => self.add_posture(&target, &fields)?,
            "OM_SetVarData" => self.set_var_data(&target, &fields)?,
            "OM_SetDrawRatioAsParseWnd" => self.set_draw_ratio(&target, &fields)?,
            "OM_AllDisableSelecter" | "OM_AllEnableSelecter" | "OM_EnableSelecter" => {
                self.configure_menu(&target, &message.class, &fields)?
            }
            "OM_LoadImageArea" => self.load_image_area(id, &fields)?,
            "OM_AskUserSelectEx" => self.ask_selection(id, &fields)?,
            "OM_AccessGetValue" | "OM_AccessWrite" => {
                self.access_profile(&target, &message.class, &fields)?
            }
            "OM_PlayDeviceStreamVideo" => self.play_video(&target, &fields)?,
            "OM_Enable" => self.objects.entry(id).or_default().enabled = true,
            "OM_Disable" => self.objects.entry(id).or_default().enabled = false,
            "OM_Visible" => self.objects.entry(id).or_default().visible = true,
            "OM_Invisible" => self.objects.entry(id).or_default().visible = false,
            "OM_InputEnable" => self.objects.entry(id).or_default().input = true,
            "OM_InputDisable" => self.objects.entry(id).or_default().input = false,
            "OM_SetSprite" => self.objects.entry(id).or_default().sprite = true,
            "OM_RemoveSprite" => self.objects.entry(id).or_default().sprite = false,
            "OM_RemoveOffAutoUpdate" => self.objects.entry(id).or_default().auto_update = true,
            "OM_OffAutoUpdate" => self.objects.entry(id).or_default().auto_update = false,
            "OM_SetBoxPos" => {
                let x = number(&fields, "Ｘ座標", 0)?;
                let y = number(&fields, "Ｙ座標", 0)?;
                let w = number(&fields, "幅", 0)?;
                let h = number(&fields, "高さ", 0)?;
                ensure!(w >= 0 && h >= 0, "negative box dimensions");
                self.objects.entry(id).or_default().rect = (x, y, w as u32, h as u32);
            }
            "OM_SetBodySize" => {
                let w = number(&fields, "幅", 0)?;
                let h = number(&fields, "高さ", 0)?;
                ensure!(w >= 0 && h >= 0, "negative body dimensions");
                let object = self.objects.entry(id).or_default();
                object.rect.2 = w as u32;
                object.rect.3 = h as u32;
            }
            "OM_SetWorldPlayerHeight"
            | "OM_SetBBInfoForQueue"
            | "OM_SetDataCPostureMoment"
            | "OM_SetRmtInfoScaling"
            | "OM_SetExclusiveCamera"
            | "OM_SetWorldSXY"
            | "OM_StoreFrameImage"
            | "OM_SetMsgTextLayout"
            | "OM_SetMsgOutputSpeed" => {
                self.objects
                    .entry(id)
                    .or_default()
                    .properties
                    .extend(fields);
            }
            "OM_SetWorldCameraAsScreen" => {
                self.set_camera(id, fields, true)?;
            }
            "OM_SetWorldCamera" => {
                self.set_camera(id, fields, false)?;
            }
            "OM_HumanOut" => {
                let style = number(&fields, "スタイル", 0)?;
                let object = self.objects.entry(id).or_default();
                object.character.remove(style);
                if style == 0 {
                    object.layers.clear();
                    object.menu = None;
                    self.buffers.remove(&id);
                } else {
                    object.layers.remove(&style);
                }
            }
            "OM_HumanIn" | "OM_HumanIn2" => {
                if target
                    .class
                    .as_ref()
                    .is_some_and(|c| matches!(c.name.as_str(), "CCharBox" | "CCharBox2"))
                {
                    return self.show_character(id, &fields);
                }
                for field in ["３ＤＸ", "３ＤＹ", "３ＤＺ"] {
                    ensure!(
                        number(&fields, field, 0)? == 0,
                        "3D placement {field} is not implemented yet"
                    );
                }
                let Some(Value::Reference(image)) = fields.get("ベース画像") else {
                    bail!("image message has no bound image");
                };
                let style = number(&fields, "スタイル", 0)?;
                ensure!(
                    style == 512 || (message.class == "OM_HumanIn" && style == 0),
                    "unimplemented world image style {style:#x}"
                );
                ensure!(
                    matches!(fields.get("表情画像"), None | Some(Value::Null)),
                    "expression-image composition is not implemented yet"
                );
                let image = self.images.load_world(image)?;
                self.buffers.remove(&id);
                self.objects.entry(id).or_default().menu = None;
                self.objects
                    .entry(id)
                    .or_default()
                    .layers
                    .insert(style, Layer::World(image));
            }
            "OM_ParseScreenTrigger" => {
                parse_screen::validate_trigger(&fields)?;
                ensure!(
                    number(&fields, "方式ID", 0)? == 0,
                    "transition effect is not implemented yet"
                );
                let duration = number(&fields, "総処理時間基", 0)?.max(0) as u32;
                if self.start_parse_transition(duration)? {
                    return Ok(());
                }
                let next = self.scene_frame()?;
                if duration == 0 {
                    self.frame = next;
                    self.transition = None;
                    for object in self.objects.values_mut() {
                        object.camera_start = None;
                    }
                } else {
                    self.transition = Some(Transition {
                        from: self.frame.rgba.clone(),
                        to: next.rgba,
                        elapsed: 0,
                        duration,
                        curve: None,
                    });
                }
            }
            _ => bail!("unimplemented runtime message {}", message.class),
        }
        Ok(())
    }
    fn host(&mut self, instruction: &Instruction) -> Result<()> {
        match instruction.class.name.as_str() {
            "CVmMsg" => return self.begin_dialogue(instruction),
            "CVmGenericMsg" => {
                let message = self.vm.message(instruction)?;
                let synchronous_parse = message.class == "OM_ParseScreenTrigger"
                    && number(&message.fields, "スタイル", 0)? & 0x8000 == 0;
                self.message(message)?;
                if synchronous_parse
                    && (self.parse_transition.is_some() || self.transition.is_some())
                {
                    self.parse_wait = true;
                    return Ok(());
                }
                if self.selection.is_some() {
                    return Ok(());
                }
            }
            "CVmBlt" => {
                if self.blit(instruction)? {
                    return Ok(());
                }
            }
            "CVmSound" => {
                let source = match self.vm.evaluate(&instruction.arguments[0])? {
                    Value::Null => None,
                    Value::Reference(r) => Some(r),
                    _ => bail!("invalid sound source"),
                };
                let n = |i: usize| self.vm.evaluate(&instruction.arguments[i])?.integer();
                let flags = n(2)? as u8;
                let kind = n(1)? as u8;
                let request = SoundRequest {
                    ticket: None,
                    source,
                    kind,
                    flags,
                    parameters: [n(3)? as u16, n(4)? as u16, n(5)? as u16],
                };
                if self.queue_sound(request)? {
                    return Ok(());
                }
            }
            "CVmSync" => {
                let target = self.vm.evaluate(&instruction.arguments[0])?;
                if let Value::Reference(reference) = target {
                    if reference
                        .class
                        .as_ref()
                        .is_some_and(|c| c.name == "CLayerSimpleFlush")
                    {
                        let mode = self.vm.evaluate(&instruction.arguments[1])?.integer()?;
                        ensure!(
                            matches!(mode, 16 | 17 | 32 | 33),
                            "unsupported visual-effect synchronization mode {mode}"
                        );
                        return if mode & 0x30 == 0x20 {
                            self.vm.acknowledge_sync_query(true)
                        } else {
                            self.vm.acknowledge_host()
                        };
                    }
                    if reference
                        .class
                        .as_ref()
                        .is_some_and(|c| c.name == "CPostureMoment")
                    {
                        let mode = self.vm.evaluate(&instruction.arguments[1])?.integer()?;
                        let timeout = self.vm.evaluate(&instruction.arguments[2])?.integer()?;
                        return self.synchronize_posture(&reference, mode, timeout);
                    }
                    if reference.class.as_ref().is_some_and(|c| {
                        ["CPmBgm", "CPmEffect", "COptimizedObs"].contains(&c.name.as_str())
                    }) {
                        let mode = self.vm.evaluate(&instruction.arguments[1])?.integer()?;
                        let timeout = self.vm.evaluate(&instruction.arguments[2])?.integer()?;
                        return self.synchronize_sound(&reference, mode, timeout);
                    }
                    ensure!(
                        reference
                            .class
                            .as_ref()
                            .is_some_and(|c| c.name == "CVideoSeq"),
                        "unsupported synchronization target"
                    );
                    let mode = self.vm.evaluate(&instruction.arguments[1])?.integer()?;
                    let timeout = self.vm.evaluate(&instruction.arguments[2])?.integer()?;
                    ensure!(
                        mode == 16 && timeout == 0,
                        "unsupported video synchronization mode/timeout"
                    );
                    let (id, player) = self
                        .video
                        .as_ref()
                        .context("no video for synchronization target")?;
                    ensure!(
                        reference.offset == Some(*id),
                        "video synchronization target mismatch"
                    );
                    if !player.finished() {
                        self.video_wait = true;
                        return Ok(());
                    }
                    self.vm.acknowledge_host()?;
                    return Ok(());
                }
                ensure!(
                    matches!(target, Value::Null),
                    "invalid synchronization target"
                );
                let duration = self
                    .vm
                    .evaluate(&instruction.arguments[2])?
                    .integer()?
                    .max(0) as u32;
                self.wait_ms = Some(duration.max(1));
                return Ok(());
            }
            other => bail!("unimplemented runtime instruction {other}"),
        }
        self.vm.acknowledge_host()?;
        Ok(())
    }
    pub fn step(&mut self, dt_ms: u32) -> Result<RunState> {
        if self.ended {
            return Ok(RunState::Ended);
        }
        self.audio_state.advance(dt_ms);
        self.advance_parse_transition(dt_ms)?;
        self.advance_postures(dt_ms)?;
        if let Some(t) = &mut self.transition {
            t.elapsed = t.elapsed.saturating_add(dt_ms).min(t.duration);
        }
        self.update_frame();
        if let Some((_, player)) = &mut self.video {
            if player.advance(dt_ms)? {
                self.frame.rgba.copy_from_slice(&player.image().rgba);
            }
            if self.video_wait && player.finished() {
                self.video_wait = false;
                self.vm.acknowledge_host()?;
            }
        }
        if self.advance_posture_wait(dt_ms)? {
            return Ok(RunState::Waiting);
        }
        if self.parse_wait {
            if self.parse_transition.is_some() || self.transition.is_some() {
                return Ok(RunState::Waiting);
            }
            self.parse_wait = false;
            self.vm.acknowledge_host()?;
        }
        if self.video_wait || self.selection.is_some() {
            return Ok(RunState::Waiting);
        }
        if self.dialogue.is_some() {
            self.tick_dialogue(dt_ms)?;
            if self.dialogue.is_some() {
                return Ok(RunState::Waiting);
            }
        }
        if let Some(tickets) = &self.audio_wait {
            if tickets.iter().any(|&id| !self.audio_state.finished(id)) {
                return Ok(RunState::Waiting);
            }
            self.audio_wait = None;
            self.vm.acknowledge_host()?;
        }
        if let Some(wait) = &mut self.wait_ms {
            *wait = wait.saturating_sub(dt_ms);
            if *wait > 0 {
                return Ok(RunState::Waiting);
            }
            self.wait_ms = None;
            self.vm.acknowledge_host()?;
        }
        for _ in 0..10000 {
            let step = self
                .vm
                .step()
                .with_context(|| format!("VM at {:?}", self.vm.cursor()))?;
            match step {
                Step::Advanced => {}
                Step::Halted => {
                    self.ended = true;
                    return Ok(RunState::Ended);
                }
                Step::Host {
                    cursor,
                    instruction,
                } => {
                    self.host(&instruction).with_context(|| {
                        format!("runtime at {}:{:#x}", cursor.scene, cursor.address)
                    })?;
                    if self.wait_ms.is_some()
                        || self.parse_wait
                        || self.audio_wait.is_some()
                        || self.posture_wait.is_some()
                        || self.video_wait
                        || self.selection.is_some()
                        || self.dialogue.is_some()
                    {
                        return Ok(RunState::Waiting);
                    }
                }
            }
        }
        Ok(RunState::Running)
    }
}
