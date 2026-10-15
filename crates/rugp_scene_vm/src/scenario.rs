use anyhow::{Context, Result, bail, ensure};
use rugp_assets::native_schema::{FieldKind, NativeSchema};
use rugp_assets::{
    object::{ClassId, ObjectReader, ObjectRef},
    script::Script,
};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "value")]
pub enum Argument {
    Number(u32),
    Text(String),
    Bytes(Vec<u8>),
    Reference(ObjectRef),
    Value {
        descriptor: u32,
        arguments: Vec<Argument>,
    },
    Message {
        class: ClassId,
        arguments: Vec<Argument>,
    },
    Parameter(Vec<Argument>),
    Field {
        name: String,
        offset: u32,
        argument: Box<Argument>,
    },
    Literal {
        type_name: String,
        data: Vec<u8>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct Instruction {
    pub index: u32,
    pub stream_offset: usize,
    pub class: ClassId,
    /// Offset in the engine's in-memory instruction array (not a file offset).
    pub address: u32,
    pub flags: u32,
    pub arguments: Vec<Argument>,
}

#[derive(Debug, Serialize)]
pub struct Scenario {
    pub object_id: usize,
    pub memory_size: u32,
    pub instructions: Vec<Instruction>,
    pub null_records: Vec<u32>,
    pub trailing_bytes: usize,
}

impl Scenario {
    pub fn visit_references(&self, visitor: &mut impl FnMut(&ObjectRef)) {
        for instruction in &self.instructions {
            for argument in &instruction.arguments {
                argument.visit_references(visitor);
            }
        }
    }
    pub fn decode(script: &Script, address_shift: u32) -> Result<Self> {
        Self::decode_with_schema(script, address_shift, None)
    }
    pub fn decode_with_schema(
        script: &Script,
        address_shift: u32,
        native: Option<&NativeSchema>,
    ) -> Result<Self> {
        ensure!(
            script.format == 0x14,
            "unsupported CRsa format {:#x}",
            script.format
        );
        let mut r = ObjectReader::new(&script.data);
        r.scrambled = true;
        r.encrypted = true;
        r.input.skip(24)?;
        let mut decoder = Decoder {
            r,
            shift: address_shift,
            native,
        };
        let mut instructions = Vec::new();
        let mut null_records = Vec::new();
        for index in 0..script.instruction_count {
            let stream_offset = decoder.r.input.position;
            let Some(class) = decoder.r.class().with_context(|| {
                format!(
                    "instruction {index}, offset {stream_offset:#x}, previous: {:?}",
                    instructions
                        .last()
                        .map(|i: &Instruction| (&i.class.name, i.stream_offset))
                )
            })?
            else {
                null_records.push(index);
                continue;
            };
            let address = decoder.r.input.u32()?;
            let flags = decoder.r.input.u32()?;
            let arguments = decoder.instruction(&class, flags).with_context(|| {
                format!(
                    "{} instruction {index}, offset {stream_offset:#x}, address {address:#x}",
                    class.name
                )
            })?;
            instructions.push(Instruction {
                index,
                stream_offset,
                class,
                address,
                flags,
                arguments,
            });
        }
        ensure!(
            decoder.r.input.remaining() == 0,
            "unparsed scene data: {} bytes at {:#x}",
            decoder.r.input.remaining(),
            decoder.r.input.position
        );
        Ok(Self {
            object_id: script.object_id,
            memory_size: script.memory_size,
            instructions,
            null_records,
            trailing_bytes: decoder.r.input.remaining(),
        })
    }
}

impl Argument {
    pub fn visit_references(&self, visitor: &mut impl FnMut(&ObjectRef)) {
        match self {
            Self::Reference(reference) => reference.visit(visitor),
            Self::Value { arguments, .. }
            | Self::Message { arguments, .. }
            | Self::Parameter(arguments) => {
                for argument in arguments {
                    argument.visit_references(visitor);
                }
            }
            Self::Field { argument, .. } => argument.visit_references(visitor),
            _ => {}
        }
    }
}

struct Decoder<'a> {
    r: ObjectReader<'a>,
    shift: u32,
    native: Option<&'a NativeSchema>,
}
impl Decoder<'_> {
    fn reference(&mut self) -> Result<Argument> {
        Ok(Argument::Reference(self.r.reference(self.shift)?))
    }
    fn number(&mut self, bytes: usize) -> Result<Argument> {
        Ok(Argument::Number(match bytes {
            1 => self.r.input.u8()? as u32,
            2 => self.r.input.u16()? as u32,
            4 => self.r.input.u32()?,
            _ => bail!("invalid numeric width"),
        }))
    }
    fn value(&mut self) -> Result<Argument> {
        let descriptor = self.r.input.u32()?;
        if descriptor == 4 {
            let kind = self.r.input.u16()?;
            ensure!(
                kind == 0x2f1a || kind == 0x2d6b,
                "unsupported literal type tag {kind:#x}"
            );
            self.r.input.u16()?;
            let length = self.r.input.u16()? as usize;
            let type_name = encoding_text(self.r.input.bytes(length)?);
            let width = match type_name.as_str() {
                "フラグ" | "バイト" => 1,
                "短整数" | "短正整数" => 2,
                "整数" | "正整数" | "低精度実数" | "色" => 4,
                "実数" | "高精度実数" => 8,
                other => bail!("unsupported literal type {other}"),
            };
            return Ok(Argument::Literal {
                type_name,
                data: self.r.input.bytes(width)?.to_vec(),
            });
        }
        let arguments = match descriptor & 3 {
            0 => vec![self.reference()?],
            1 => vec![Argument::Text(self.r.input.string()?)],
            2 => Vec::new(),
            _ => bail!("unsupported value descriptor {descriptor:#x}"),
        };
        Ok(Argument::Value {
            descriptor,
            arguments,
        })
    }
    fn fields(&mut self, layout: &str) -> Result<Vec<Argument>> {
        let mut args = Vec::new();
        for field in layout.split_whitespace() {
            args.push(match field {
                "r" => self.reference()?,
                "v" => self.value()?,
                "s" => Argument::Text(self.r.input.string()?),
                "b" => self.number(1)?,
                "w" => self.number(2)?,
                "d" => self.number(4)?,
                other => Argument::Bytes(self.r.input.bytes(other.parse()?)?.to_vec()),
            });
        }
        Ok(args)
    }
    fn instruction(&mut self, class: &ClassId, flags: u32) -> Result<Vec<Argument>> {
        ensure!(
            class.schema == 0x11,
            "unsupported instruction schema {}",
            class.schema
        );
        Ok(match class.name.as_str() {
            "CVmRet" => Vec::new(),
            "CVmJump" | "CVmLabel" => self.fields("r")?,
            "CVmSync" => self.fields("r b d")?,
            "CVmImage" => self.fields("w w b b v v")?,
            "CVmFlagOp" => self.fields("v v r w b d")?,
            "CVmCall" => {
                let mut args = self.fields("r")?;
                let count = self.r.input.count()?;
                ensure!(
                    count <= self.r.input.remaining() / 4,
                    "invalid call argument count"
                );
                for _ in 0..count {
                    args.push(self.value()?);
                }
                args
            }
            "CVmMsg" => {
                let mut args = self.fields("r s r")?;
                if flags & 0x04000000 != 0 {
                    args.push(self.reference()?);
                }
                if flags & 0x00200000 != 0 {
                    let mode = self.r.input.u16()?;
                    args.push(Argument::Number(mode as u32));
                    if mode & 3 != 0 {
                        let count = self.r.input.count()?;
                        ensure!(
                            count <= self.r.input.remaining() / 2,
                            "invalid message argument count"
                        );
                        for _ in 0..count {
                            args.push(self.reference()?);
                            if mode & 3 == 3 {
                                args.push(self.reference()?);
                            }
                        }
                    }
                    if mode & 4 != 0 {
                        args.push(Argument::Text(self.r.input.string()?));
                    }
                }
                args
            }
            "CVmSound" => {
                let mut args = self.fields("r b")?;
                let f = self.r.input.u8()?;
                args.push(Argument::Number(f as u32));
                args.extend(self.fields("w w w")?);
                if f & 4 != 0 {
                    args.extend(if f & 8 != 0 {
                        self.fields("r")?
                    } else if f & 16 == 0 {
                        self.fields("14")?
                    } else {
                        self.fields("8")?
                    });
                }
                args
            }
            "CVmGenericMsg" => {
                let mut args = self.fields("v")?;
                let class = self
                    .r
                    .raw_class()?
                    .context("generic message has no class")?;
                self.r.duplicate_last()?;
                let message_args = self.message(&class)?;
                args.push(Argument::Message {
                    class,
                    arguments: message_args,
                });
                args.push(self.number(2)?);
                let count = self.r.input.count()?;
                ensure!(
                    count <= self.r.input.remaining() / 4,
                    "invalid message parameter count"
                );
                for _ in 0..count {
                    args.push(Argument::Parameter(self.parameter()?));
                }
                args
            }
            "CVmBlt" => {
                let mut args = Vec::new();
                if let Some(class) = self.r.class()? {
                    self.r.duplicate_last()?;
                    let arguments = self.inline(&class)?;
                    args.push(Argument::Message { class, arguments });
                }
                args.extend(self.fields("w b b v")?);
                args
            }
            other => bail!("unsupported instruction class {other}"),
        })
    }
    fn parameter(&mut self) -> Result<Vec<Argument>> {
        let mut args = Vec::new();
        match self.r.input.u16()? {
            0x1257 => {
                if let Some(class) = self.r.class()? {
                    args.push(Argument::Text(class.name));
                }
            }
            0x369e => {
                if let Some(class) = self.r.raw_class()? {
                    args.push(Argument::Text(class.name));
                }
            }
            0x2d6b | 0x2f1a => {
                args.push(self.number(2)?);
                let len = self.r.input.u16()? as usize;
                args.push(Argument::Bytes(self.r.input.bytes(len)?.to_vec()));
            }
            kind => bail!("unsupported parameter type {kind:#x}"),
        }
        let index = self.r.input.u16()?;
        args.push(Argument::Number(index as u32));
        if index == 0xffff {
            let len = self.r.input.u16()? as usize;
            let text = encoding_text(self.r.input.bytes(len)?);
            self.r.cache_class(ClassId {
                name: text.clone(),
                schema: 0,
            });
            args.push(Argument::Text(text));
        }
        args.push(self.value()?);
        Ok(args)
    }
    fn inline(&mut self, class: &ClassId) -> Result<Vec<Argument>> {
        let inline = self
            .r
            .read_inline(class, self.shift)
            .with_context(|| format!("inline {}#{}", class.name, class.schema))?;
        let mut args = vec![Argument::Bytes(inline.data)];
        args.extend(inline.references.into_iter().map(Argument::Reference));
        Ok(args)
    }
    fn message(&mut self, class: &ClassId) -> Result<Vec<Argument>> {
        let name = class
            .name
            .strip_prefix("&-")
            .context("invalid object message class")?;
        if let Some(fields) = self.native.and_then(|native| native.messages.get(name)) {
            let mut args = Vec::new();
            for field in fields {
                let argument = match field.kind {
                    FieldKind::U8 => self.number(1)?,
                    FieldKind::U16 => self.number(2)?,
                    FieldKind::U32 => self.number(4)?,
                    FieldKind::U64 => Argument::Bytes(self.r.input.bytes(8)?.to_vec()),
                    FieldKind::String => Argument::Text(self.r.input.string()?),
                    FieldKind::Variant => self.value()?,
                    FieldKind::Reference => self.reference()?,
                    FieldKind::Bytes(n) => Argument::Bytes(self.r.input.bytes(n)?.to_vec()),
                    _ => bail!(
                        "unsupported native field kind {:?} in {name}.{}",
                        field.kind,
                        field.name
                    ),
                };
                args.push(Argument::Field {
                    name: field.name.clone(),
                    offset: field.offset,
                    argument: Box::new(argument),
                });
            }
            return Ok(args);
        }
        ensure!(
            class.schema == 0,
            "unsupported object message schema {}",
            class.schema
        );
        let layout = match name {
            "OM_Null"
            | "OM_Visible"
            | "OM_UpdateAllView"
            | "OM_AllDisableSelecter"
            | "OM_Disable"
            | "OM_Enable"
            | "OM_AllEnableSelecter"
            | "OM_InputEnable"
            | "OM_Invisible"
            | "OM_OffAutoUpdate"
            | "OM_ParseScreenTrigger_AllHumanOut"
            | "OM_RemoveOffAutoUpdate"
            | "OM_RemoveSprite"
            | "OM_SetFitImage"
            | "OM_SetSprite" => "",
            "OM_VmmFinishByUser"
            | "OM_AgesRecalc"
            | "OM_InitUserSelectHistory"
            | "OM_SetTimeInterval"
            | "OM_SetAcsRoomRefRatio"
            | "OM_SetMsgOutputSpeed"
            | "OM_SetMsgTextLayout"
            | "OM_SetStyleOthers"
            | "OM_SetWorldPlayerHeight"
            | "OM_StartLayerEffect"
            | "OM_StopAutoAnim"
            | "OM_StopLayerEffect"
            | "OM_StopXsFade"
            | "OM_TryBrowseAndLoadUuc" => "d",
            "OM_SetUucSaveSwitch" => "b",
            "OM_VramAgesImageIn" => "r 16 r d",
            "OM_AccessGetValue" => "s s r",
            "OM_AccessWrite" => "s s",
            "OM_AddPostureMomentum" | "OM_StoreFrameImage" | "OM_TWFGauge_AddPostureMomentum" => {
                "r"
            }
            "OM_AddSelecter" => "s r",
            "OM_AskUserSelectEx" => "r r d",
            "OM_BodyScroll" | "OM_BoxScroll" => "20",
            "OM_ChangeAllBoxVisibility" => "b w",
            "OM_ChangeBoxStyleOthers"
            | "OM_PlayMedia"
            | "OM_SetBodySize"
            | "OM_SetBoxPos"
            | "OM_SetCharBreath"
            | "OM_SetColorData" => "8",
            "OM_ChangeSelecterStatus" => "s w w",
            "OM_CopyImage" | "OM_SetWorldCamera" => "36",
            "OM_EmbedAlpherChannel" => "r 12",
            "OM_EnableSelecter" | "OM_SetUucNavigateString" => "s",
            "OM_GetUserSelectHistory" => "r b r r",
            "OM_HumanIn" => "r 16 r r 8",
            "OM_HumanOut" | "OM_SetBaseAltitude" | "OM_SetRmtInfoScaling" | "OM_SetShading" => "12",
            "OM_ImageASyncFadeinout" => "r 20 r",
            "OM_StartFLE" => "r 96",
            "OM_LoadImageArea" => "w w b r",
            "OM_ParseImageBuild" => "r b",
            "OM_ParseScreenTrigger" => "12 r",
            "OM_PlayDeviceStreamVideo" => "r r 12",
            "OM_PlayTextStaffrollVideo" => "r r 32",
            "OM_rUGPSerifSetting" => "d s",
            "OM_SetAirDepth" => "22",
            "OM_SetBaseCompassPoint" => "14",
            "OM_SetBBInfoForQueue" | "OM_SetWorldCameraAsScreen" => "24",
            "OM_SetBreathEffect" => "r 40 r 10 r",
            "OM_SetCacheExtra" => "10",
            "OM_SetDataCPostureMoment" => "26",
            "OM_SetDrawRatioAsParseWnd" => "d r d",
            "OM_SetExclusiveCamera" => "d r",
            "OM_SetRmtInfoForQueue" => "16 s",
            "OM_SetShootingEnviroment" => "r 44 r d",
            "OM_SetShootingParam" => "d r r s s 27 r r 36",
            "OM_SetVarData" => "d v",
            "OM_SetWeather" => "30 r",
            "OM_SetWeatherRainSnow" => "r 24 r 12 r",
            "OM_SetWorldSXY" => "w w",
            "OM_TWF_ChangeArmCount" => "4 s 8",
            "OM_TWF_ExclusiveCommand" => "8 r",
            "OM_TWF_ExclusiveCommand2" => "78 r r",
            "OM_TWF_SelectArm" => "4 s d",
            "OM_TWF_SetArm" => "4 s d r",
            "OM_TWF_SetGaugeSpeed" => "w d d",
            _ => bail!("unsupported object message {name}"),
        };
        self.fields(layout)
    }
}

fn encoding_text(bytes: &[u8]) -> String {
    rugp_assets::binary::decode_text(bytes)
}
