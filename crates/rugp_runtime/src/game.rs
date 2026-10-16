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

pub mod actor_snapshot;
mod alpha;
mod async_image;
mod blit;
mod camera;
mod character;
#[cfg(test)]
mod checkpoint_test_wire;
mod dialogue;
#[cfg(test)]
mod ending_tests;
mod fast_forward;
mod gallery;
mod input_wait;
mod menu;
pub mod native_save;
mod parse_screen;
mod posture;
mod profile;
pub mod scene_snapshot;
mod scroll;
pub mod snapshot;
mod sound;
mod spatial_fade;
mod staffroll;
#[cfg(test)]
mod sync_tests;
#[cfg(test)]
mod test_support;
mod text_menu;
mod timer;
pub mod ui_snapshot;
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
    /// Resolved per-instance placement; never modifies the cached PCM source.
    pub stereo: crate::audio_spatial::StereoGains,
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
    parent: Option<u64>,
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
            parent: None,
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
    fn cache_world_center(&self, image: &WorldImage) -> Result<()> {
        if let Some(first) = image.planes.first() {
            self.camera_center_y.set(
                first
                    .plane
                    .projected_camera_center_y(&self.camera, self.rect)?,
            );
        }
        Ok(())
    }
    fn validate_projection(&self) -> Result<()> {
        for layer in self.layers.values() {
            if let Layer::World(image) = layer {
                self.cache_world_center(image)?;
                for plane in image.planes.iter().filter(|p| p.draw_flags & 2 == 0) {
                    plane.texture.validate()?;
                    plane.plane.quad(&self.camera, self.rect)?;
                }
            } else if let Layer::Screen(texture) = layer {
                texture.validate()?;
            }
        }
        self.character.validate_texture()
    }
    fn render(&self, frame: &mut Framebuffer) -> Result<()> {
        if let Some(backdrop) = self.backdrop {
            backdrop.paint(frame, self.rect);
        }
        for layer in self.layers.values() {
            match layer {
                Layer::World(image) => {
                    self.cache_world_center(image)?;
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
        self.character
            .render(frame, self.rect, self.opacity, &self.camera)?;
        Ok(())
    }
}
struct Transition {
    from: Vec<u8>,
    to: Vec<u8>,
    elapsed: u32,
    duration: u32,
    curve: Option<FadeCurve>,
    spatial: Option<spatial_fade::SpatialFade>,
}
pub struct Game {
    pub vm: Vm,
    pub archive: Arc<RioArchive>,
    pub objects: BTreeMap<u64, ObjectState>,
    images: ImageManager,
    frame: Framebuffer,
    rasterize_scene: bool,
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
    staffroll: Option<(u64, crate::staffroll::StaffRoll)>,
    staffroll_wait: bool,
    timers: BTreeMap<u64, timer::Timer>,
    timer_wait: Option<u64>,
    async_images: Vec<async_image::ImageFade>,
    image_wait: Option<rugp_assets::object::InlineIdentity>,
    profiles: crate::profile_store::ProfileStore,
    /// Immutable original-program gallery bindings, not rewindable save state.
    gallery: gallery::Gallery,
    menus: BTreeMap<u64, crate::menu::MenuState>,
    selection: Option<menu::SelectionWait>,
    text_menus: BTreeMap<u64, text_menu::TextMenu>,
    dialogue: Option<dialogue::DialogueWait>,
    input_wait: Option<input_wait::InputWait>,
    message_windows: BTreeMap<u64, crate::message_window::MessageWindow>,
    message_config: Option<crate::message_window::MessageConfig>,
    font: Option<crate::font::TextFont>,
    font_file: Option<(std::path::PathBuf, u32)>,
    pointer_inside: bool,
    dialogue_pressed: bool,
    pointer_armed: bool,
    pub navigation: String,
    script_save_enabled: bool,
    /// Native browse/load suspends its host instruction until load or cancel.
    load_requested: bool,
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
        let gallery = gallery::Gallery::from_program(&program)?;
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
            rasterize_scene: true,
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
            staffroll: None,
            staffroll_wait: false,
            timers: BTreeMap::new(),
            timer_wait: None,
            async_images: Vec::new(),
            image_wait: None,
            profiles: crate::profile_store::ProfileStore::default(),
            gallery,
            menus: BTreeMap::new(),
            selection: None,
            text_menus: BTreeMap::new(),
            dialogue: None,
            input_wait: None,
            message_windows: BTreeMap::new(),
            message_config: None,
            font: None,
            font_file: None,
            pointer_inside: false,
            dialogue_pressed: false,
            pointer_armed: false,
            navigation: String::new(),
            script_save_enabled: true,
            load_requested: false,
            settings: BTreeMap::new(),
            ended: false,
        })
    }
    pub fn size(&self) -> (u32, u32) {
        (self.frame.width, self.frame.height)
    }
    /// Script permission flag; the save operation additionally checks lifecycle/waits.
    pub fn script_save_enabled(&self) -> bool {
        self.script_save_enabled
    }
    pub fn waiting_for_load(&self) -> bool {
        self.load_requested
    }
    pub fn cancel_load_request(&mut self) -> Result<()> {
        if self.load_requested {
            self.vm.acknowledge_host()?;
            self.load_requested = false;
        }
        Ok(())
    }
    /// Host UI uses the user's trusted font preference, not paths from saves.
    pub fn interface_font(&self, size: u16) -> Result<crate::font::TextFont> {
        if let Some(font) = &self.font {
            return font.resized(size);
        }
        if let Some((path, index)) = &self.font_file {
            return crate::font::TextFont::from_file(path, *index, size);
        }
        let root = self.archive.volumes[0]
            .path
            .parent()
            .context("game directory missing")?;
        let config = crate::message_window::MessageConfig::load(root)?;
        crate::font::TextFont::system(&config.family, size)
    }
    /// Desktop sessions use a per-game user-data folder. The ordinary `open`
    /// remains volatile for deterministic diagnostics and tests.
    pub fn open_persistent(root: &Path, data_root: &Path) -> Result<Self> {
        let mut game = Self::open(root)?;
        let data_root = if data_root.is_absolute() {
            data_root.to_owned()
        } else {
            std::env::current_dir()?.join(data_root)
        };
        let directory = data_root.join(game.project_key());
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
    /// Diagnostic only: omit scene pixel composition, not host execution,
    /// projection/cache updates, resource decoding, input or media clocks.
    /// Applies to future compositions; this mode is not visual verification.
    pub fn set_scene_rasterization(&mut self, enabled: bool) {
        self.rasterize_scene = enabled;
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
        Self::render_objects_mode(&self.objects, self.size(), self.rasterize_scene)
    }
    fn render_objects_mode(
        objects: &BTreeMap<u64, ObjectState>,
        size: (u32, u32),
        rasterize: bool,
    ) -> Result<Framebuffer> {
        if rasterize {
            return Self::render_objects(objects, size);
        }
        let frame = Framebuffer::new(size.0, size.1)?;
        for object in objects.values().filter(|o| o.visible) {
            object.validate_projection()?;
        }
        Ok(frame)
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
        if let Some(transition) = &mut self.transition {
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
            if self.rasterize_scene {
                if let Some(spatial) = &mut transition.spatial {
                    if !spatial.keeps_previous_frame() {
                        self.frame.rgba.copy_from_slice(&transition.from);
                    }
                    spatial.render_q13(
                        &mut self.frame.rgba,
                        &transition.to,
                        (self.frame.width, self.frame.height),
                        transition
                            .curve
                            .as_ref()
                            .map_or((a * 8192 / d) as u16, |curve| {
                                curve.value(transition.elapsed, transition.duration)
                            }),
                    );
                } else {
                    for (out, (from, to)) in self
                        .frame
                        .rgba
                        .iter_mut()
                        .zip(transition.from.iter().zip(&transition.to))
                    {
                        *out = ((*from as u64 * (d - a) + *to as u64 * a + d / 2) / d) as u8;
                    }
                }
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
        if message.class == "OM_VmmFinishByUser" {
            ensure!(
                number(&fields, "終了コード", 0)? == 0,
                "unsupported nonzero engine exit code"
            );
            self.queue_sound(SoundRequest {
                ticket: None,
                source: None,
                kind: 255,
                flags: 0,
                stereo: Default::default(),
                parameters: [0; 3],
            })?;
            self.ended = true;
            return Ok(());
        }
        if message.class == "OM_SetUucSaveSwitch" {
            let enabled = number(&fields, "フラグ", 1)?;
            ensure!(
                matches!(enabled, 0 | 1),
                "invalid script save switch {enabled}"
            );
            self.script_save_enabled = enabled != 0;
            return Ok(());
        }
        if message.class == "OM_SetUucNavigateString" {
            let Some(Value::Text(text)) = fields.get("現在状況説明文") else {
                bail!("invalid navigation text");
            };
            self.navigation = text.clone();
            return Ok(());
        }
        if message.class == "OM_TryBrowseAndLoadUuc" {
            let null_target = matches!(&message.target, Value::Null)
                || matches!(&message.target, Value::Reference(r) if r.class.is_none()
                    && r.flags == 0 && r.offset.is_none() && r.inline.is_none());
            ensure!(
                null_target && fields.len() == 1 && number(&fields, "状態", -1)? == 0,
                "unsupported browse/load request"
            );
            ensure!(
                !self.load_requested && self.selection.is_none() && self.dialogue.is_none(),
                "overlapping browse/load request"
            );
            self.load_requested = true;
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
        if let Some(parent) = target.owner.as_ref().and_then(|r| r.offset) {
            self.objects.entry(id).or_default().parent = Some(parent);
        }
        match message.class.as_str() {
            "OM_ChangeAllBoxVisibility" => {
                let visible = number(&fields, "可視", 1)?;
                ensure!(matches!(visible, 0 | 1), "invalid group visibility");
                ensure!(
                    number(&fields, "モード", 0)? == 0,
                    "unsupported group visibility mode"
                );
                for object in self.objects.values_mut().filter(|o| o.parent == Some(id)) {
                    object.visible = visible != 0;
                }
            }
            "OM_QueueFace2Data" => self.queue_character_face(id, &fields)?,
            "OM_SetRefData"
                if target
                    .class
                    .as_ref()
                    .is_some_and(|c| matches!(c.name.as_str(), "CCharBox" | "CCharBox2")) =>
            {
                self.set_character_core(id, &fields)?
            }
            "OM_SetLayerData2D" => self.set_character_layer(id, &fields)?,
            "OM_SetCharMouthAnimeSetting" | "OM_SetCharBreath"
                if target
                    .class
                    .as_ref()
                    .is_some_and(|c| matches!(c.name.as_str(), "CCharBox" | "CCharBox2")) =>
            {
                // Portraits remain static; keep lip-sync/breath configuration
                // separate from placement/style and other material properties.
                let channel = if message.class == "OM_SetCharBreath" {
                    "breath"
                } else {
                    "mouth"
                };
                let properties = &mut self.objects.entry(id).or_default().properties;
                for (key, value) in fields {
                    properties.insert(format!("render.{channel}.{key}"), value);
                }
            }
            "OM_PlayMedia" | "OM_SetTimeInterval"
                if target
                    .class
                    .as_ref()
                    .is_some_and(|c| c.name == "CWaitableTimer") =>
            {
                self.timer_message(&target, &message.class, &fields)?;
            }
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
            "OM_SetColorData"
                if target
                    .class
                    .as_ref()
                    .is_some_and(|c| c.name == "CLayerSimpleFlush") =>
            {
                let key = number(&fields, "識別値", 0)?;
                let data = fields.get("データ").context("color data missing")?;
                self.objects
                    .entry(id)
                    .or_default()
                    .properties
                    .insert(format!("render.color.{key}"), data.clone());
            }
            "OM_AddPostureMomentum" | "OM_AddPostureMomentumEx" => {
                self.add_posture(&target, &fields)?
            }
            "OM_SetVarData" => self.set_var_data(&target, &fields)?,
            "ONM_SetMenberAsName"
                if target
                    .class
                    .as_ref()
                    .is_some_and(|c| c.name == "CNormalCamera") =>
            {
                self.set_effect_camera_member(id, &fields)?;
            }
            "OM_SetWeatherRainSnow"
                if target
                    .class
                    .as_ref()
                    .is_some_and(|c| c.name == "CParseBgBox") =>
            {
                self.set_weather(id, fields)?;
            }
            "OM_SetDrawRatioAsParseWnd" => self.set_draw_ratio(&target, &fields)?,
            "OM_SetSpecifyCmdLighting"
                if target
                    .class
                    .as_ref()
                    .is_some_and(|c| c.name == "CParseBgBox") =>
            {
                // Portable projection omits pulsing/recoloring of this world
                // surface. Retain its settings without inventing a VM wait.
                let properties = &mut self.objects.entry(id).or_default().properties;
                for (key, value) in fields {
                    properties.insert(format!("render.lighting.{key}"), value);
                }
            }
            "OM_AllDisableSelecter"
            | "OM_AllEnableSelecter"
            | "OM_EnableSelecter"
            | "OM_AllIgnoreSelecter" => self.configure_menu(&target, &message.class, &fields)?,
            "OM_SetSelecterByDatabase" => self.configure_gallery_selector(&target, &fields)?,
            "OM_LoadImageArea" => self.load_image_area(id, &fields)?,
            "OM_ParseImageBuild" => self.build_parse_image(id, &fields)?,
            "OM_VramAgesImageIn" => self.vram_image_in(id, &fields)?,
            "OM_ParseScreenTrigger_AllHumanOut" => {
                for object in self.objects.values_mut().filter(|o| o.parent == Some(id)) {
                    object.character.remove(0);
                }
            }
            "OM_UpdateAllView" | "OM_AgesRecalc" => self.present_scene()?,
            "OM_AskUserSelectEx" => self.ask_selection(id, &fields)?,
            "OM_AddSelecter" if self.message_windows.contains_key(&id) => {
                self.add_text_selector(id, &fields)?
            }
            "OM_AccessGetValue" | "OM_AccessWrite" => {
                self.access_profile(&target, &message.class, &fields)?
            }
            "OM_PlayDeviceStreamVideo" => self.play_video(&target, &fields)?,
            "OM_PlayTextStaffrollVideo" => self.play_staffroll(&target, &fields)?,
            "OM_EmbedAlpherChannel" => self.embed_alpha(&target, fields)?,
            "OM_ImageASyncFadeinout" => self.fade_image(&target, &fields)?,
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
            "OM_SetFitImage" if target.class.as_ref().is_some_and(|c| c.name == "CImgBox") => {
                // rvmm 1000d7f0 toggles CViewBox's fit flag; the next image
                // assignment (1002c8af) updates its body/box extent.
                self.objects
                    .entry(id)
                    .or_default()
                    .properties
                    .insert("render.image.fit".into(), Value::Integer(1));
            }
            "OM_BodyScroll"
                if number(&fields, "モード", 0)? == 1
                    && [0, 0x8000].contains(&number(&fields, "同期属性", 0)?) =>
            {
                self.retain_mode_one_body_scroll(&target, fields)?;
            }
            "OM_BodyScroll" | "OM_BoxScroll" => {
                scroll::apply(self.objects.entry(id).or_default(), &message.class, fields)?;
            }
            "OM_SetWorldPlayerHeight"
            | "OM_SetBBInfoForQueue"
            | "OM_SetDataCPostureMoment"
            | "OM_SetRmtInfoScaling"
            | "OM_SetShading"
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
            "OM_StopAutoAnim"
                if target
                    .class
                    .as_ref()
                    .is_some_and(|c| c.name == "CParseBgBox") =>
            {
                ensure!(
                    number(&fields, "停止モード", 0)? == 0,
                    "unsupported background auto-animation stop mode"
                );
                // Vm60 10032090 iterates owned character boxes; 10032140
                // releases their automatic animation/image references. Static
                // portraits do not start those channels. Explicit posture
                // resources and ParseScreen cameras keep their own clocks.
                for (key, value) in fields {
                    self.objects
                        .entry(id)
                        .or_default()
                        .properties
                        .insert(format!("render.auto_anim.{key}"), value);
                }
            }
            "OM_SetCacheExtra"
                if target
                    .class
                    .as_ref()
                    .is_some_and(|c| c.name == "CParseBgBox") =>
            {
                // Native enlarged backing/cache hints do not affect the
                // portable full-scene renderer's logical viewport.
                for (key, value) in fields {
                    self.objects
                        .entry(id)
                        .or_default()
                        .properties
                        .insert(format!("render.cache_extra.{key}"), value);
                }
            }
            "OM_StopXsFade"
                if target.class.as_ref().is_some_and(|c| {
                    ["CParseBgBox", "CImgAreaBox", "CImgBox"].contains(&c.name.as_str())
                }) =>
            {
                ensure!(
                    matches!(number(&fields, "モード", 0)?, 0 | 1),
                    "unsupported Xs-effect stop mode"
                );
                // Only the separately retained native window effect is stopped.
                // Do not cancel a framebuffer fade or a posture/camera motion.
                // The omitted distortion has no intermediate pixels to retain
                // versus finalize, so both stop variants clear its settings.
                self.objects
                    .entry(id)
                    .or_default()
                    .properties
                    .retain(|key, _| !key.starts_with("render.xs_fade."));
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
                let style = number(&fields, "スタイル", 0)?;
                if message.class == "OM_HumanIn2"
                    && style & 255 == 32
                    && target
                        .class
                        .as_ref()
                        .is_some_and(|c| c.name == "CParseBgBox")
                {
                    // Vm60 1003f39a -> 1003f4e9 clears the world image before
                    // inspecting the supplied reference. Null is valid here.
                    let object = self.objects.entry(id).or_default();
                    object.layers.clear();
                    object.menu = None;
                    self.buffers.remove(&id);
                    return Ok(());
                }
                let Some(Value::Reference(image)) = fields.get("ベース画像") else {
                    bail!("image message has no bound image");
                };
                let planar_model = image
                    .class
                    .as_ref()
                    .is_some_and(|c| c.name == "CDcAgesModelQsT");
                let mut placement = [0; 3];
                for (axis, field) in ["３ＤＸ", "３ＤＹ", "３ＤＺ"].into_iter().enumerate()
                {
                    if planar_model {
                        ensure!(
                            matches!(fields.get(field), None | Some(Value::Integer(_)))
                                || matches!(fields.get(field), Some(Value::Real(n)) if n.is_finite()),
                            "invalid textured-model placement {field}"
                        );
                    } else {
                        placement[axis] = number(&fields, field, 0)?;
                    }
                }
                if image.class.as_ref().is_some_and(|c| c.name == "CBg2d") {
                    ensure!(
                        placement == [0; 3],
                        "nonzero CBg2d world-group placement is not implemented yet"
                    );
                }
                ensure!(
                    style == 512 || ((message.class == "OM_HumanIn" || planar_model) && style == 0),
                    "unimplemented world image style {style:#x}"
                );
                ensure!(
                    matches!(fields.get("表情画像"), None | Some(Value::Null)),
                    "expression-image composition is not implemented yet"
                );
                let source = image.clone();
                let mut image = self.images.load_world(image)?;
                if !planar_model && placement != [0; 3] {
                    ensure!(
                        image.planes.len() == 1,
                        "plain image placement needs one bitmap plane"
                    );
                    let plane =
                        crate::world::Plane::bitmap_at(&image.planes[0].texture, placement)?;
                    image.planes[0].plane = plane;
                    image.boundary = plane;
                }
                self.buffers.remove(&id);
                self.objects.entry(id).or_default().menu = None;
                if planar_model {
                    let properties = &mut self.objects.entry(id).or_default().properties;
                    properties.insert("render.model.source".into(), Value::Reference(source));
                    for field in ["３ＤＸ", "３ＤＹ", "３ＤＺ"] {
                        if let Some(value) = fields.get(field) {
                            properties.insert(format!("render.model.{field}"), value.clone());
                        }
                    }
                }
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
                        spatial: None,
                    });
                }
            }
            _ => bail!("unimplemented runtime message {}", message.class),
        }
        Ok(())
    }
    fn host(&mut self, instruction: &Instruction) -> Result<()> {
        self.images.begin_observation()?;
        let result = self.host_inner(instruction);
        let images = self.images.end_observation();
        result?;
        self.record_gallery_images(&images)
    }
    fn host_inner(&mut self, instruction: &Instruction) -> Result<()> {
        match instruction.class.name.as_str() {
            "CVmImage" => self.load_vm_image(instruction)?,
            "CVmMsg" => return self.begin_dialogue(instruction),
            "CVmGenericMsg" => {
                let message = self.vm.message(instruction)?;
                let synchronous_parse = message.class == "OM_ParseScreenTrigger"
                    && number(&message.fields, "スタイル", 0)? & 0x8000 == 0;
                self.message(message)?;
                if self.ended {
                    return Ok(());
                }
                if synchronous_parse
                    && (self.parse_transition.is_some() || self.transition.is_some())
                {
                    self.parse_wait = true;
                    return Ok(());
                }
                if self.selection.is_some() || self.load_requested {
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
                let stereo = if matches!(kind, 2 | 4)
                    && flags & 4 != 0
                    && source
                        .as_ref()
                        .is_some_and(|r| r.class.as_ref().is_some_and(|c| c.name == "CPmEffect"))
                {
                    let extra = instruction
                        .arguments
                        .get(6)
                        .context("acoustic placement missing")?;
                    if flags & 8 != 0 {
                        let Value::Reference(reference) = self.vm.evaluate(extra)? else {
                            bail!("acoustic motion is not an object");
                        };
                        let class = reference
                            .class
                            .as_ref()
                            .context("acoustic motion class missing")?;
                        let inline = reference
                            .inline
                            .as_ref()
                            .context("stored acoustic motion is unsupported")?;
                        let curve = rugp_assets::motion::MotionCurve::from_inline(inline, class)?;
                        crate::audio_spatial::StereoGains::from_motion(curve)?
                    } else {
                        let rugp_scene_vm::Argument::Bytes(bytes) = extra else {
                            bail!("inline acoustic position missing");
                        };
                        crate::audio_spatial::StereoGains::from_position(flags, bytes)?
                    }
                } else {
                    Default::default()
                };
                let request = SoundRequest {
                    ticket: None,
                    source,
                    kind,
                    flags,
                    stereo,
                    parameters: [n(3)? as u16, n(4)? as u16, n(5)? as u16],
                };
                if self.queue_sound(request)? {
                    return Ok(());
                }
            }
            "CVmSync" => {
                let target = self.vm.evaluate(&instruction.arguments[0])?;
                let mode = self.vm.evaluate(&instruction.arguments[1])?.integer()?;
                let object_variable = matches!(&instruction.arguments[0], rugp_scene_vm::Argument::Reference(r)
                    if r.class.as_ref().is_some_and(|c| c.name == "CVmVarObj"));
                if object_variable && !matches!(target, Value::Reference(_)) {
                    // Vm60 10072b3d..10072b69 extracts only object-valued
                    // CBasicValue contents (UnivUI 100231c0). Initial scalar
                    // zero is not an event. Missing variable contents query
                    // false or return immediately, even with a time word.
                    return if mode & 0x30 == 0x20 {
                        self.vm.acknowledge_sync_query(false)
                    } else {
                        self.vm.acknowledge_host()
                    };
                }
                if matches!(target, Value::Null) && mode & 0x30 == 0x20 {
                    // A literal null differs from an empty object variable:
                    // 10072c75 reports true, without reading the time word.
                    return self.vm.acknowledge_sync_query(true);
                }
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
                        let timeout = if mode == 32 {
                            0 // native nonblocking media query does not read this operand
                        } else {
                            self.vm.evaluate(&instruction.arguments[2])?.integer()?
                        };
                        return self.synchronize_sound(&reference, mode, timeout);
                    }
                    if reference
                        .class
                        .as_ref()
                        .is_some_and(|c| c.name == "CTextStaffrollSeq")
                    {
                        let mode = self.vm.evaluate(&instruction.arguments[1])?.integer()?;
                        let timeout = self.vm.evaluate(&instruction.arguments[2])?.integer()?;
                        return self.synchronize_staffroll(&reference, mode, timeout);
                    }
                    if reference
                        .class
                        .as_ref()
                        .is_some_and(|c| c.name == "CWaitableTimer")
                    {
                        let mode = self.vm.evaluate(&instruction.arguments[1])?.integer()?;
                        let timeout = self.vm.evaluate(&instruction.arguments[2])?.integer()?;
                        return self.synchronize_timer(&reference, mode, timeout);
                    }
                    if reference
                        .class
                        .as_ref()
                        .is_some_and(|c| c.name == "CGenericUso")
                    {
                        let mode = self.vm.evaluate(&instruction.arguments[1])?.integer()?;
                        let timeout = self.vm.evaluate(&instruction.arguments[2])?.integer()?;
                        return self.synchronize_image(&reference, mode, timeout);
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
                let duration = self.vm.evaluate(&instruction.arguments[2])?.integer()? as u32;
                if mode & 0x30 == 0x10 {
                    // Vm60 10072cc2..10072d56: input pump flags 0x1c.
                    // Literal-null, mode 16, time zero is an INDEFINITE user
                    // confirmation, not a one-millisecond generic delay.
                    self.input_wait = Some(input_wait::InputWait::new(duration));
                } else if duration != 0 {
                    self.wait_ms = Some(duration);
                } else {
                    return self.vm.acknowledge_host();
                }
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
        if self.load_requested {
            return Ok(RunState::Waiting);
        }
        self.audio_state.advance(dt_ms);
        self.advance_timers(dt_ms)?;
        self.advance_async_images(dt_ms)?;
        self.advance_parse_transition(dt_ms)?;
        self.advance_postures(dt_ms)?;
        self.finish_parse_transition();
        if let Some(t) = &mut self.transition {
            t.elapsed = t.elapsed.saturating_add(dt_ms).min(t.duration);
        }
        self.update_frame();
        self.advance_staffroll(dt_ms)?;
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
        if self.advance_input_wait(dt_ms)? {
            return Ok(RunState::Waiting);
        }
        if self.video_wait
            || self.staffroll_wait
            || self.timer_wait.is_some()
            || self.image_wait.is_some()
            || self.selection.is_some()
        {
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
                    if self.ended {
                        return Ok(RunState::Ended);
                    }
                    if self.wait_ms.is_some()
                        || self.parse_wait
                        || self.audio_wait.is_some()
                        || self.posture_wait.is_some()
                        || self.video_wait
                        || self.staffroll_wait
                        || self.timer_wait.is_some()
                        || self.image_wait.is_some()
                        || self.selection.is_some()
                        || self.dialogue.is_some()
                        || self.input_wait.is_some()
                        || self.load_requested
                    {
                        return Ok(RunState::Waiting);
                    }
                }
            }
        }
        Ok(RunState::Running)
    }
}
