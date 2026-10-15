use crate::{
    image_manager::ImageManager,
    render::{Framebuffer, Texture},
};
use anyhow::{Context, Result, bail, ensure};
use rugp_assets::{RioArchive, native_schema::NativeSchema, object::ObjectRef};
use rugp_scene_vm::{
    Instruction,
    vm::{Message, Program, Step, Value, Vm},
};
use std::{collections::BTreeMap, path::Path, sync::Arc};

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
}
pub struct ObjectState {
    pub enabled: bool,
    pub visible: bool,
    pub input: bool,
    pub sprite: bool,
    pub auto_update: bool,
    pub rect: (i32, i32, u32, u32),
    pub properties: BTreeMap<String, Value>,
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
            layers: BTreeMap::new(),
        }
    }
}
struct Transition {
    from: Vec<u8>,
    to: Vec<u8>,
    elapsed: u32,
    duration: u32,
}
pub struct Game {
    pub vm: Vm,
    pub archive: Arc<RioArchive>,
    pub objects: BTreeMap<u64, ObjectState>,
    images: ImageManager,
    frame: Framebuffer,
    transition: Option<Transition>,
    wait_ms: Option<u32>,
    audio_wait: bool,
    sounds: Vec<SoundRequest>,
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
        Ok(Self {
            images: ImageManager::new(Arc::clone(&archive)),
            archive,
            vm,
            objects: BTreeMap::new(),
            frame: Framebuffer::new(1024, 600)?,
            transition: None,
            wait_ms: None,
            audio_wait: false,
            sounds: Vec::new(),
            navigation: String::new(),
            settings: BTreeMap::new(),
            ended: false,
        })
    }
    pub fn size(&self) -> (u32, u32) {
        (self.frame.width, self.frame.height)
    }
    pub fn frame(&self) -> &[u8] {
        &self.frame.rgba
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
            for layer in object.layers.values() {
                frame.blit(&layer.texture, object.rect, 255)?;
            }
        }
        Ok(frame)
    }
    fn update_frame(&mut self) {
        if let Some(transition) = &self.transition {
            let a = transition.elapsed.min(transition.duration) as u64;
            let d = transition.duration as u64;
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
                        && number(&fields, "スクリーン座標Ｙ", 0)? == 0
                        && number(&fields, "ズーム率", 720)? == 720,
                    "non-default screen camera is not implemented yet"
                );
                self.objects
                    .entry(id)
                    .or_default()
                    .properties
                    .extend(fields);
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
                } else {
                    object.layers.remove(&style);
                }
            }
            "OM_HumanIn2" => {
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
                ensure!(style == 512, "unimplemented world image style {style:#x}");
                let texture = self.images.load(image)?;
                self.objects
                    .entry(id)
                    .or_default()
                    .layers
                    .insert(style, Layer { texture });
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
                    });
                }
            }
            _ => bail!("unimplemented runtime message {}", message.class),
        }
        Ok(())
    }
    fn host(&mut self, instruction: &Instruction) -> Result<()> {
        match instruction.class.name.as_str() {
            "CVmGenericMsg" => self.message(self.vm.message(instruction)?)?,
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
                ensure!(
                    matches!(self.vm.evaluate(&instruction.arguments[0])?, Value::Null),
                    "object/video synchronization is not implemented yet"
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
                    if self.wait_ms.is_some() || self.audio_wait {
                        return Ok(RunState::Waiting);
                    }
                }
            }
        }
        Ok(RunState::Running)
    }
}
