use crate::binary::{Bits, Reader};
use crate::crypto::{decode_offset, decode_size};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::sync::Arc;

pub const ENCRYPTED_SIGNATURE: u32 = 0x1edb927c;
pub const OBJECT_SIGNATURE: u32 = 0x29f6cba4;
pub const RIO_SIGNATURE: u32 = 0x596e32cd;
pub const ICI_SIGNATURE: u32 = 0x673ce92a;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ClassId {
    pub name: String,
    pub schema: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct ObjectNode {
    pub id: usize,
    pub parent: Option<usize>,
    pub class: ClassId,
    pub flags: u16,
    /// Position in the concatenated RIO volumes, including the object prefix.
    pub offset: Option<u64>,
    /// Stored bytes, including the object prefix.
    pub size: Option<u32>,
    pub encoded_offset: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ObjectRef {
    pub class: Option<ClassId>,
    pub cache_index: Option<u32>,
    pub flags: u16,
    pub offset: Option<u64>,
    pub size: Option<u32>,
    pub owner: Option<Box<ObjectRef>>,
    pub selector: u32,
    pub inline: Option<InlineObject>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InlineObject {
    pub data: Vec<u8>,
    pub references: Vec<ObjectRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitmap: Option<InlineBitmap>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub posture: Option<Box<crate::posture::PostureFields>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion: Option<Box<crate::motion::MotionCurve>>,
    #[serde(skip)]
    identity: InlineIdentity,
}

/// Runtime identity of an inline instance, independent of byte equality.
/// Cloned/cached references retain it; a separately read identical body does not.
/// Never serialized: allocation addresses are not archive identifiers.
#[derive(Debug, Clone)]
pub struct InlineIdentity(Arc<()>);
impl PartialEq for InlineIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for InlineIdentity {}

#[derive(Debug, Clone, Serialize)]
pub struct InlineBitmap {
    pub header: BitmapHeader,
    pub data_offset: usize,
}
#[derive(Debug, Clone, Serialize)]
pub enum BitmapHeader {
    R6(crate::image::R6Header),
    Rip(crate::rip::RipHeader),
}
impl InlineObject {
    pub fn identity(&self) -> InlineIdentity {
        self.identity.clone()
    }
    pub fn decode_bitmap(&self) -> Result<crate::image::Image> {
        let bitmap = self
            .bitmap
            .as_ref()
            .context("inline object is not a supported bitmap")?;
        let compressed = self
            .data
            .get(bitmap.data_offset..)
            .context("inline bitmap offset outside body")?;
        match &bitmap.header {
            BitmapHeader::R6(header) => crate::image::decode_r6ti_pixels(
                header,
                compressed
                    .get(..header.data_size as usize)
                    .context("truncated inline bitmap")?,
            ),
            BitmapHeader::Rip(header) => crate::rip::decode_rip_pixels(header, compressed),
        }
    }
}

impl ObjectRef {
    pub fn visit(&self, visitor: &mut impl FnMut(&Self)) {
        visitor(self);
        if let Some(owner) = &self.owner {
            owner.visit(visitor);
        }
        if let Some(inline) = &self.inline {
            for reference in &inline.references {
                reference.visit(visitor);
            }
        }
    }
}

#[derive(Clone)]
enum CacheEntry {
    Empty,
    Class(ClassId),
    Object(ObjectRef),
    TimeCurve(Box<crate::motion::TimeCurve>),
}

pub struct ObjectReader<'a> {
    pub input: Reader<'a>,
    cache: Vec<CacheEntry>,
    pub encrypted: bool,
    pub scrambled: bool,
    pub schema: u16,
    reference_started: bool,
    last_tag: u32,
}

impl<'a> ObjectReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            input: Reader::new(data),
            cache: vec![CacheEntry::Empty; 2],
            encrypted: false,
            scrambled: false,
            schema: 0,
            reference_started: false,
            last_tag: 0,
        }
    }
    pub fn with_data<'b>(&self, data: &'b [u8]) -> ObjectReader<'b> {
        ObjectReader {
            input: Reader::new(data),
            cache: self.cache.clone(),
            encrypted: self.encrypted,
            scrambled: self.scrambled,
            schema: self.schema,
            reference_started: false,
            last_tag: 0,
        }
    }
    pub fn raw_class(&mut self) -> Result<Option<ClassId>> {
        let tag = self.input.u16()?;
        if tag == 0xffff {
            let schema = self.input.u8()? as u16;
            let name = self.input.string()?;
            let class = ClassId { name, schema };
            self.cache.push(CacheEntry::Class(class.clone()));
            Ok(Some(class))
        } else if tag & 0x8000 != 0 {
            match self.cache.get((tag & 0x7fff) as usize) {
                Some(CacheEntry::Class(class)) => Ok(Some(class.clone())),
                _ => bail!("invalid raw class reference {tag:#x}"),
            }
        } else {
            Ok(None)
        }
    }
    pub fn duplicate_last(&mut self) -> Result<()> {
        let last = self.cache.last().context("empty object cache")?.clone();
        self.cache.push(last);
        Ok(())
    }
    pub fn cache_class(&mut self, class: ClassId) {
        self.cache.push(CacheEntry::Class(class));
    }
    pub fn reference(&mut self, shift: u32) -> Result<ObjectRef> {
        self.reference_at_depth(shift, 0)
    }
    fn reference_at_depth(&mut self, shift: u32, depth: usize) -> Result<ObjectRef> {
        ensure!(depth < 128, "object reference exceeds recursion limit");
        if !self.reference_started {
            self.reference_started = true;
            let count = self.input.short_count()?;
            self.pad(count)?;
        }
        let class = self.class()?;
        let mut object = ObjectRef {
            class: class.clone(),
            cache_index: None,
            flags: 0,
            offset: None,
            size: None,
            owner: None,
            selector: 0,
            inline: None,
        };
        if class.is_none() {
            ensure!(
                (self.last_tag as usize) < self.cache.len(),
                "object cache reference {} out of range at {:#x}",
                self.last_tag,
                self.input.position
            );
            if let Some(CacheEntry::Object(cached)) = self.cache.get(self.last_tag as usize) {
                object = cached.clone();
            }
            object.cache_index = Some(self.last_tag);
            return Ok(object);
        }
        object.flags = self.input.u16()?;
        if object.flags & 0x40 != 0 {
            let index = self.cache.len();
            self.cache.push(CacheEntry::Object(object.clone()));
            object.inline = Some(self.read_inline(class.as_ref().unwrap(), shift)?);
            self.cache[index] = CacheEntry::Object(object.clone());
            return Ok(object);
        }
        object.offset = Some(decode_offset(self.input.u32()?) << shift);
        object.size = Some(decode_size(self.input.u32()?));
        object.owner = Some(Box::new(self.reference_at_depth(shift, depth + 1)?));
        object.selector = if object.flags & 7 != 0 {
            self.input.u32()?
        } else if object.flags & 0x8000 != 0 {
            self.input.u8()? as u32
        } else {
            self.input.u16()? as u32
        };
        self.cache.push(CacheEntry::Object(object.clone()));
        Ok(object)
    }
    pub fn read_inline(&mut self, class: &ClassId, shift: u32) -> Result<InlineObject> {
        let start = self.input.position;
        let mut references = Vec::new();
        let mut bitmap = None;
        let mut posture = None;
        let mut motion = None;
        match (class.name.as_str(), class.schema) {
            ("CMimeFlat16LimitOne", 1) => {
                crate::mime::MimeCurve::read_body(&mut self.input, class)?;
            }
            ("Cr6Ti", 3) => {
                let header = crate::image::R6Header::read(self, shift)?;
                let data_offset = self.input.position - start;
                references.push(*header.image_properties.clone());
                self.input.skip(header.stored_data_size())?;
                bitmap = Some(InlineBitmap {
                    header: BitmapHeader::R6(header),
                    data_offset,
                });
            }
            ("CRip007" | "CRip008", 2 | 3) => {
                let header = crate::rip::RipHeader::read(
                    self,
                    class.schema,
                    shift,
                    class.name == "CRip008",
                )?;
                let data_offset = self.input.position - start;
                if let Some(properties) = &header.image_properties {
                    references.push(*properties.clone());
                }
                self.input.skip(header.data_size as usize)?;
                bitmap = Some(InlineBitmap {
                    header: BitmapHeader::Rip(header),
                    data_offset,
                });
            }
            ("CDcAgesModelQsT", 1) => {
                let version = self.input.u8()?;
                ensure!(version == 1, "unsupported model version {version}");
                self.input.skip(16)?;
                for _ in 0..2 {
                    let size = self.input.u32()? as usize;
                    self.input.skip(size)?;
                }
                let parts = self.input.count()?;
                ensure!(
                    parts <= self.input.remaining() / 2,
                    "invalid model part count"
                );
                for _ in 0..parts {
                    references.push(self.reference(shift)?);
                    self.input.skip(20)?;
                }
                references.push(self.reference(shift)?);
            }
            ("CSceneTimer", 1) => {
                let version = self.input.u8()?;
                ensure!(
                    version == 1 || version == 2,
                    "unsupported scene timer version {version}"
                );
                self.input.string()?;
                self.input.skip(if version == 2 { 24 } else { 22 })?;
            }
            ("CChar2CompleteDataCore", 1) => {
                let version = self.input.u8()?;
                ensure!(
                    (1..=6).contains(&version),
                    "unsupported character core version {version}"
                );
                if version >= 5 {
                    references.push(self.reference(shift)?);
                    self.input.skip(12)?;
                } else {
                    self.input.u32()?;
                }
                references.push(self.reference(shift)?);
                if version >= 6 {
                    self.input.u32()?;
                }
                self.input.skip(28)?;
                if version >= 2 {
                    self.input.skip(16)?;
                }
                for _ in 0..2 {
                    references.push(self.reference(shift)?);
                }
                let face_version = self.input.u8()?;
                ensure!(
                    face_version == 1 || face_version == 2,
                    "unsupported character face version {face_version}"
                );
                self.input.skip(if face_version >= 2 { 8 } else { 4 })?;
                for _ in 0..12 {
                    references.push(self.reference(shift)?);
                }
                self.input.skip(16)?;
                if version >= 2 {
                    self.input.skip(8)?;
                    if version >= 3 {
                        self.input.u32()?;
                    }
                    if version >= 4 {
                        self.input.u32()?;
                    }
                }
            }
            ("CAcsPos", 2) => {
                self.input.skip(32)?;
            }
            ("CMoveAcsOngen", 3) => {
                motion = Some(Box::new(crate::motion::MotionCurve::read_body(
                    self, class,
                )?));
            }
            ("CPostureMoment", 4) => {
                let parsed = crate::posture::Posture::read_body(self, class, shift)?;
                references.extend(parsed.references);
                posture = Some(Box::new(parsed.fields));
            }
            (name, 12) if name.starts_with("CFade") => {
                ensure!(
                    [
                        "CFadeNormal",
                        "CFadeSdtRatio",
                        "CFadeRSideCarten",
                        "CFadeMozaik",
                        "CFadeMulHorz",
                        "CFadeInvert",
                        "CFadeShock",
                        "CFadeXsRatio",
                        "CFadeXsMergeBlack",
                        "CFadeStretchAnti",
                        "CFadeOverStretchAnti",
                        "CFadeXsOverStretchAnti",
                        "CFadeQubeStretchAnti",
                        "CFadeXsRasterNoize",
                        "CFadeCarten",
                        "CFadeXsSqrRaster_HRasterV_VRasterH",
                        "CFadeXsSrcRotate",
                        "CFadeXsCircleRaster",
                        "CFadeXsHRasterHOffset",
                        "CFadeMergeBlack",
                        "CFadeMergeWhite",
                        "CFadeMergeColor",
                        "CFadeStretch",
                        "CFadeSdtCircleRaster"
                    ]
                    .contains(&name),
                    "unsupported inline effect {name}"
                );
                if name == "CFadeCarten" {
                    self.input.skip(6)?;
                }
                if name == "CFadeSdtCircleRaster" {
                    self.input.skip(15)?;
                }
                let count = self.input.u16()? as usize;
                self.input.u16()?;
                for _ in 0..3 {
                    references.push(self.reference(shift)?);
                }
                self.input.skip(24 + count * 4)?;
                let tail = match name {
                    "CFadeXsRatio" | "CFadeXsMergeBlack" => 6,
                    "CFadeStretchAnti" | "CFadeStretch" => 17,
                    "CFadeOverStretchAnti" => 25,
                    "CFadeXsOverStretchAnti" => 30,
                    "CFadeQubeStretchAnti" => 44,
                    "CFadeXsRasterNoize" => 23,
                    "CFadeXsSqrRaster_HRasterV_VRasterH" | "CFadeXsHRasterHOffset" => 17,
                    "CFadeXsSrcRotate" => 24,
                    "CFadeXsCircleRaster" => 21,
                    "CFadeMergeWhite" | "CFadeMergeColor" => 4,
                    _ => 0,
                };
                self.input.skip(tail)?;
            }
            (name, schema) => bail!("unsupported inline class {name}#{schema} at {start:#x}"),
        }
        Ok(InlineObject {
            data: self.input.consumed_since(start)?.to_vec(),
            references,
            bitmap,
            posture,
            motion,
            identity: InlineIdentity(Arc::new(())),
        })
    }
    pub fn header(&mut self) -> Result<ClassId> {
        let signature = self.input.u32()?;
        ensure!(
            [
                ENCRYPTED_SIGNATURE,
                OBJECT_SIGNATURE,
                RIO_SIGNATURE,
                ICI_SIGNATURE
            ]
            .contains(&signature),
            "invalid RIO signature {signature:#x}"
        );
        let version = self.input.u16()?;
        if (0x10..=0x3fff).contains(&version) {
            self.schema = version;
            if version >= 0x11 {
                self.input.u16()?;
            }
        } else {
            self.input.position -= 2;
        }
        self.encrypted = signature == ENCRYPTED_SIGNATURE;
        self.scrambled = self.encrypted;
        let class = self.class()?.context("root object has no class")?;
        self.cache.push(CacheEntry::Empty);
        Ok(class)
    }
    pub fn pad(&mut self, count: usize) -> Result<()> {
        ensure!(
            self.cache.len().saturating_add(count) <= 1_000_000,
            "class cache exceeds limit"
        );
        self.cache
            .resize(self.cache.len() + count, CacheEntry::Empty);
        Ok(())
    }
    pub(crate) fn motion_time_curve(&mut self) -> Result<Option<crate::motion::TimeCurve>> {
        let scrambled = self.scrambled;
        self.scrambled = false;
        let class = self.class();
        self.scrambled = scrambled;
        if let Some(class) = class? {
            let index = self.cache.len();
            self.pad(1)?;
            let curve = crate::motion::TimeCurve::read_body(&mut self.input, &class)?;
            self.cache[index] = CacheEntry::TimeCurve(Box::new(curve.clone()));
            Ok(Some(curve))
        } else if self.last_tag == 0 {
            Ok(None)
        } else {
            match self.cache.get(self.last_tag as usize) {
                Some(CacheEntry::TimeCurve(curve)) => Ok(Some((**curve).clone())),
                _ => bail!("invalid motion time curve reference {}", self.last_tag),
            }
        }
    }
    pub fn cached_class(&self, index: u32) -> Result<&ClassId> {
        match self.cache.get(index as usize) {
            Some(CacheEntry::Class(class)) => Ok(class),
            _ => bail!("cache entry {index} is not a declared class/member name"),
        }
    }
    pub fn class(&mut self) -> Result<Option<ClassId>> {
        let tag = self.input.u16()?;
        let tag32 = if tag == 0x7fff {
            self.input.u32()?
        } else {
            ((tag as u32 & 0x8000) << 16) | (tag as u32 & 0x7fff)
        };
        self.last_tag = tag32;
        if tag32 & 0x80000000 == 0 {
            return Ok(None);
        }
        if tag == 0xffff {
            let schema = self.input.u16()?;
            let name = if self.scrambled {
                let len = self.input.short_count()?;
                ensure!(len <= 255, "class name exceeds limit");
                decode_class_name(self.input.bytes(len)?)?
            } else {
                let len = self.input.u16()? as usize;
                ensure!(len <= 255, "class name exceeds limit");
                std::str::from_utf8(self.input.bytes(len)?)?.to_owned()
            };
            let class = ClassId { name, schema };
            self.cache.push(CacheEntry::Class(class.clone()));
            Ok(Some(class))
        } else {
            let index = (tag32 & 0x7fffffff) as usize;
            match self.cache.get(index) {
                Some(CacheEntry::Class(class)) => Ok(Some(class.clone())),
                _ => bail!(
                    "invalid class reference {index} at {:#x}",
                    self.input.position - 2
                ),
            }
        }
    }
    pub fn nodes(&mut self, shift: u32) -> Result<Vec<ObjectNode>> {
        let mut nodes = Vec::new();
        self.node_list(None, 0, shift, &mut nodes)?;
        Ok(nodes)
    }
    fn node_list(
        &mut self,
        parent: Option<usize>,
        depth: usize,
        shift: u32,
        nodes: &mut Vec<ObjectNode>,
    ) -> Result<()> {
        ensure!(depth < 128, "object tree exceeds recursion limit");
        let count = self.input.count()?;
        ensure!(
            count <= 1_000_000usize.saturating_sub(nodes.len()),
            "object count exceeds limit"
        );
        for _ in 0..count {
            let at = self.input.position;
            let flags = self.input.u16()?;
            let class = match flags & 7 {
                0 => {
                    self.input.skip(if flags & 0x8000 != 0 { 1 } else { 2 })?;
                    self.class()?.context("node has no class")?
                }
                1 => {
                    self.input.u32()?;
                    let kind = self.input.u16()?;
                    if kind == 0x1e57 {
                        self.class()?.context("node has no class")?
                    } else if kind == 0x2d6b || kind == 0x2f1a {
                        let schema = self.input.u16()?;
                        let len = self.input.u16()? as usize;
                        let (name, _, errors) =
                            encoding_rs::SHIFT_JIS.decode(self.input.bytes(len)?);
                        ensure!(!errors, "invalid basic type name");
                        ClassId {
                            name: name.into_owned(),
                            schema,
                        }
                    } else {
                        bail!("unsupported node type {kind:#x} at {at:#x}");
                    }
                }
                _ => bail!("unsupported node flags {flags:#x} at {at:#x}"),
            };
            let (offset, size, encoded_offset) = if flags & 8 != 0 {
                let offset = self.input.u32()?;
                let size = self.input.u32()?;
                (
                    Some(decode_offset(offset) << shift),
                    Some(decode_size(size)),
                    Some(offset),
                )
            } else {
                (None, None, None)
            };
            let id = nodes.len();
            nodes.push(ObjectNode {
                id,
                parent,
                class,
                flags,
                offset,
                size,
                encoded_offset,
            });
            if flags & 0x200 == 0 {
                self.node_list(Some(id), depth + 1, shift, nodes)?;
            }
        }
        Ok(())
    }
}

pub fn decode_class_name(data: &[u8]) -> Result<String> {
    const MAP1: &[u8] = b"eaitrosducmnSglR";
    const MAP2: &[u8] = b"\x01COFLfBMxphyAVbI";
    const MAP3: &[u8] = b"EHTDPWXkqvNjwGz02U_K15JQZ467839\0";
    let mut bits = Bits::new(data);
    let mut output = Vec::new();
    if bits.read(1) == Some(0) {
        output.push(b'C');
    }
    while let Some(bit) = bits.read(1) {
        let value = if bit == 0 {
            bits.read(4).map(|n| MAP1[n as usize])
        } else if bits.read(1) == Some(1) {
            bits.read(5).map(|n| MAP3[n as usize])
        } else {
            match bits.read(4) {
                Some(0) => bits.read(8).map(|n| n as u8),
                Some(n) => Some(MAP2[n as usize]),
                None => None,
            }
        };
        match value {
            Some(0) | None => break,
            Some(n) => output.push(n),
        }
    }
    Ok(crate::binary::decode_text(&output))
}
