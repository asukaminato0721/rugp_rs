use crate::{
    image_manager::ImageManager,
    render::{Framebuffer, Texture},
    world::{Camera, Plane},
};
use anyhow::{Context, Result, bail, ensure};
use rugp_assets::{RioArchive, fade::FadeCurve, native_schema::NativeSchema, object::ObjectRef};
use rugp_scene_vm::{
    Instruction,
    vm::{Message, Program, Step, Value, Vm},
};
use std::{collections::BTreeMap, path::Path, sync::Arc};

mod blit;
mod menu;
mod profile;
mod video;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Running,
    Waiting,
    Ended,
}
#[derive(Debug)]
pub struct SoundRequest {
    pub source: Option<ObjectRef>,
    pub kind: u8,
    pub flags: u8,
    /// Native mode-specific words; ordinary play (kind 2) uses source defaults.
    pub parameters: [u16; 3],
}
struct Layer {
    texture: Arc<Texture>,
    plane: Option<Plane>,
}
pub struct ObjectState {
    pub enabled: bool,
    pub visible: bool,
    pub input: bool,
    pub sprite: bool,
    pub auto_update: bool,
    pub rect: (i32, i32, u32, u32),
    pub properties: BTreeMap<String, Value>,
    pub camera: Camera,
    menu: Option<u64>,
    layers: BTreeMap<i32, Layer>,
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
            menu: None,
            layers: BTreeMap::new(),
        }
    }
}
impl ObjectState {
    fn render(&self, frame: &mut Framebuffer) -> Result<()> {
        for layer in self.layers.values() {
            let rect = layer
                .plane
                .as_ref()
                .map_or(Ok(self.rect), |plane| plane.rect(&self.camera, self.rect))?;
            frame.blit(&layer.texture, rect, 255)?;
        }
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
    wait_ms: Option<u32>,
    audio_wait: bool,
    sounds: Vec<SoundRequest>,
    video: Option<(u64, crate::video::VideoPlayer)>,
    video_wait: bool,
    profiles: crate::profile_store::ProfileStore,
    menus: BTreeMap<u64, crate::menu::MenuState>,
    selection: Option<menu::SelectionWait>,
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
            wait_ms: None,
            audio_wait: false,
            sounds: Vec::new(),
            video: None,
            video_wait: false,
            profiles: crate::profile_store::ProfileStore::default(),
            menus: BTreeMap::new(),
            selection: None,
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
    pub fn finish_audio(&mut self) -> Result<()> {
        ensure!(self.audio_wait, "no pending audio wait");
        self.audio_wait = false;
        self.vm.acknowledge_host()
    }
    pub fn time_audio_wait(&mut self, duration_ms: u32) -> Result<()> {
        ensure!(self.audio_wait, "no pending audio wait");
        self.audio_wait = false;
        self.wait_ms = Some(duration_ms.max(1));
        Ok(())
    }
    fn scene_frame(&self) -> Result<Framebuffer> {
        let mut frame = Framebuffer::new(self.frame.width, self.frame.height)?;
        for object in self.objects.values().filter(|o| o.visible) {
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
                self.transition = None;
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
        match message.class.as_str() {
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
            | "OM_SetExclusiveCamera"
            | "OM_SetWorldSXY"
            | "OM_StoreFrameImage"
            | "OM_SetMsgTextLayout" => {
                self.objects
                    .entry(id)
                    .or_default()
                    .properties
                    .extend(fields);
            }
            "OM_SetWorldCameraAsScreen" => {
                ensure!(
                    number(&fields, "スクリーン座標Ｘ", 0)? == 0
                        && number(&fields, "スクリーン座標Ｙ", 0)? == 0,
                    "translated screen camera is not implemented yet"
                );
                let focal = number(&fields, "ズーム率", 720)?;
                ensure!(
                    (1..=32767).contains(&focal),
                    "invalid camera focal length {focal}"
                );
                let object = self.objects.entry(id).or_default();
                object.camera.focal_length = focal as u16;
                object.properties.extend(fields);
            }
            "OM_SetWorldCamera" => {
                for field in [
                    "ロール角",
                    "ピッチ角",
                    "バンク角",
                    "３ＤＸ",
                    "３ＤＹ",
                    "３ＤＺ",
                ] {
                    ensure!(
                        number(&fields, field, 0)? == 0,
                        "3D camera {field} is not implemented yet"
                    );
                }
                self.objects
                    .entry(id)
                    .or_default()
                    .properties
                    .extend(fields);
            }
            "OM_HumanOut" => {
                let style = number(&fields, "スタイル", 0)?;
                let object = self.objects.entry(id).or_default();
                if style == 0 {
                    object.layers.clear();
                    object.menu = None;
                    self.buffers.remove(&id);
                } else {
                    object.layers.remove(&style);
                }
            }
            "OM_HumanIn" | "OM_HumanIn2" => {
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
                let texture = self.images.load(image)?;
                let plane = Some(Plane::bitmap(&texture)?);
                self.buffers.remove(&id);
                self.objects.entry(id).or_default().menu = None;
                self.objects
                    .entry(id)
                    .or_default()
                    .layers
                    .insert(style, Layer { texture, plane });
            }
            "OM_ParseScreenTrigger" => {
                ensure!(
                    number(&fields, "方式ID", 0)? == 0,
                    "transition effect is not implemented yet"
                );
                let duration = number(&fields, "総処理時間基", 0)?.max(0) as u32;
                let next = self.scene_frame()?;
                if duration == 0 {
                    self.frame = next;
                    self.transition = None;
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
            "CVmGenericMsg" => {
                self.message(self.vm.message(instruction)?)?;
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
                let waiting = source.is_some() && [2, 4].contains(&kind) && flags & 1 != 0;
                self.sounds.push(SoundRequest {
                    source,
                    kind,
                    flags,
                    parameters: [n(3)? as u16, n(4)? as u16, n(5)? as u16],
                });
                if waiting {
                    self.audio_wait = true;
                    return Ok(());
                }
            }
            "CVmSync" => {
                let target = self.vm.evaluate(&instruction.arguments[0])?;
                if let Value::Reference(reference) = target {
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
        if self.video_wait || self.selection.is_some() {
            return Ok(RunState::Waiting);
        }
        if self.audio_wait {
            return Ok(RunState::Waiting);
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
                        || self.audio_wait
                        || self.video_wait
                        || self.selection.is_some()
                    {
                        return Ok(RunState::Waiting);
                    }
                }
            }
        }
        Ok(RunState::Running)
    }
}
