//! Reads rUGP's native serialization descriptors without loading DLLs.
//!
//! The original program initializes field tables with straight-line x86 MOV
//! instructions. We recover constant data writes, resolve PE data imports,
//! and expose the resulting schema to the Rust object reader. No native code
//! is executed, and this is not a CPU emulator.
use crate::binary::{Reader, decode_text};
use anyhow::{Context, Result, bail, ensure};
use iced_x86::{Decoder, DecoderOptions, Mnemonic, OpKind, Register};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::Path,
};

#[derive(Debug, Clone, Serialize)]
pub enum FieldKind {
    String,
    U8,
    U16,
    U32,
    U64,
    Variant,
    Reference,
    NativeObject,
    InlineObject,
    Bytes(usize),
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeField {
    pub name: String,
    pub type_name: String,
    pub offset: u32,
    pub kind: FieldKind,
}

#[derive(Debug, Default, Serialize)]
pub struct NativeSchema {
    pub messages: BTreeMap<String, Vec<NativeField>>,
    pub unresolved: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
enum Value {
    Literal(u32),
    Import { dll: String, ordinal: u32 },
}

struct Section {
    va: u32,
    offset: usize,
    size: usize,
    flags: u32,
}
struct Pe {
    data: Vec<u8>,
    sections: Vec<Section>,
    imports: HashMap<u32, Value>,
    exports: HashMap<u32, u32>,
    writes: HashMap<u32, Value>,
}

impl Pe {
    fn open(path: &Path) -> Result<Self> {
        let data = fs::read(path)?;
        let mut r = Reader::new(&data);
        ensure!(r.u16()? == 0x5a4d, "not an MZ executable");
        r.skip(0x3a)?;
        let offset = r.u32()? as usize;
        let mut r = Reader::new(data.get(offset..).context("PE header outside file")?);
        ensure!(r.u32()? == 0x4550, "not a PE executable");
        ensure!(r.u16()? == 0x14c, "expected 32-bit x86 PE");
        let count = r.u16()? as usize;
        ensure!(count <= 96, "too many PE sections");
        r.skip(12)?;
        let optional_size = r.u16()? as usize;
        r.u16()?;
        let optional = r.bytes(optional_size)?;
        let mut o = Reader::new(optional);
        ensure!(o.u16()? == 0x10b, "not PE32");
        o.skip(26)?;
        let base = o.u32()?;
        let mut sections = Vec::new();
        for _ in 0..count {
            r.skip(8)?;
            r.u32()?;
            let va = base
                .checked_add(r.u32()?)
                .context("section address overflow")?;
            let size = r.u32()? as usize;
            let offset = r.u32()? as usize;
            r.skip(12)?;
            let flags = r.u32()?;
            ensure!(
                offset.checked_add(size).is_some_and(|n| n <= data.len()),
                "PE section outside file"
            );
            sections.push(Section {
                va,
                offset,
                size,
                flags,
            });
        }
        let export_rva = u32::from_le_bytes(
            optional
                .get(96..100)
                .context("missing PE directories")?
                .try_into()?,
        );
        let import_rva = u32::from_le_bytes(
            optional
                .get(104..108)
                .context("missing import directory")?
                .try_into()?,
        );
        let mut pe = Self {
            data,
            sections,
            imports: HashMap::new(),
            exports: HashMap::new(),
            writes: HashMap::new(),
        };
        if export_rva != 0 {
            let dir = base
                .checked_add(export_rva)
                .context("export address overflow")?;
            let ordinal_base = pe.word(dir + 16)?;
            let count = pe.word(dir + 20)?.min(65536);
            let table = base
                .checked_add(pe.word(dir + 28)?)
                .context("export table overflow")?;
            for i in 0..count {
                pe.exports.insert(
                    ordinal_base + i,
                    base.checked_add(pe.word(table + i * 4)?)
                        .context("export overflow")?,
                );
            }
        }
        if import_rva != 0 {
            let mut dir = base
                .checked_add(import_rva)
                .context("import address overflow")?;
            for _ in 0..256 {
                let original = pe.word(dir)?;
                let name = pe.word(dir + 12)?;
                let first = pe.word(dir + 16)?;
                if name == 0 {
                    break;
                }
                let dll = pe
                    .text(base.checked_add(name).context("import name overflow")?)?
                    .to_ascii_lowercase();
                let table = base
                    .checked_add(if original != 0 { original } else { first })
                    .context("import table overflow")?;
                for i in 0..65536u32 {
                    let entry = pe.word(table + i * 4)?;
                    if entry == 0 {
                        break;
                    }
                    if entry & 0x80000000 != 0 {
                        pe.imports.insert(
                            base + first + i * 4,
                            Value::Import {
                                dll: dll.clone(),
                                ordinal: entry & 0xffff,
                            },
                        );
                    }
                }
                dir += 20;
            }
        }
        pe.recover_initializers();
        Ok(pe)
    }
    fn offset(&self, va: u32) -> Option<usize> {
        self.sections.iter().find_map(|s| {
            va.checked_sub(s.va)
                .filter(|&n| (n as usize) < s.size)
                .map(|n| s.offset + n as usize)
        })
    }
    fn word(&self, va: u32) -> Result<u32> {
        let offset = self.offset(va).context("PE address outside sections")?;
        Ok(u32::from_le_bytes(
            self.data
                .get(offset..offset + 4)
                .context("truncated PE word")?
                .try_into()?,
        ))
    }
    fn text(&self, va: u32) -> Result<String> {
        let offset = self.offset(va).context("PE string outside sections")?;
        let tail = &self.data[offset..self.data.len().min(offset + 2048)];
        let length = tail
            .iter()
            .position(|&n| n == 0)
            .context("unterminated PE string")?;
        Ok(decode_text(&tail[..length]))
    }
    fn value(&self, va: u32) -> Option<Value> {
        self.imports
            .get(&va)
            .or_else(|| self.writes.get(&va))
            .cloned()
            .or_else(|| self.word(va).ok().map(Value::Literal))
    }
    fn recover_initializers(&mut self) {
        let mut recovered = HashMap::new();
        for section in self.sections.iter().filter(|s| s.flags & 0x20 != 0) {
            let mut decoder = Decoder::with_ip(
                32,
                &self.data[section.offset..section.offset + section.size],
                section.va as u64,
                DecoderOptions::NONE,
            );
            let mut registers: HashMap<Register, Value> = HashMap::new();
            let mut candidate = HashMap::new();
            while decoder.can_decode() {
                let i = decoder.decode();
                match i.mnemonic() {
                    Mnemonic::Mov => {
                        let absolute =
                            i.memory_base() == Register::None && i.memory_index() == Register::None;
                        let source = match i.op1_kind() {
                            OpKind::Immediate32 => Some(Value::Literal(i.immediate32())),
                            OpKind::Register => registers.get(&i.op1_register()).cloned(),
                            OpKind::Memory if absolute => self.value(i.memory_displacement32()),
                            _ => None,
                        };
                        match i.op0_kind() {
                            OpKind::Register => {
                                if let Some(v) = source {
                                    registers.insert(i.op0_register(), v);
                                } else {
                                    registers.remove(&i.op0_register());
                                }
                            }
                            OpKind::Memory if absolute => {
                                let address = i.memory_displacement32();
                                if self.sections.iter().any(|s| {
                                    s.flags & 0x80000000 != 0
                                        && address >= s.va
                                        && address as u64 + 4 <= s.va as u64 + s.size as u64
                                }) && let Some(value) = source
                                {
                                    candidate.insert(address, value);
                                }
                            }
                            _ => {
                                registers.clear();
                                candidate.clear();
                            }
                        }
                    }
                    Mnemonic::Xor
                        if i.op0_kind() == OpKind::Register
                            && i.op1_kind() == OpKind::Register
                            && i.op0_register() == i.op1_register() =>
                    {
                        registers.insert(i.op0_register(), Value::Literal(0));
                    }
                    Mnemonic::Ret => {
                        recovered.extend(candidate.drain());
                        registers.clear();
                    }
                    Mnemonic::Nop => {}
                    _ => {
                        registers.clear();
                        candidate.clear();
                    }
                }
            }
        }
        self.writes = recovered;
    }
}

impl NativeSchema {
    pub fn from_directory(path: &Path) -> Result<Self> {
        let mut modules = BTreeMap::new();
        for name in ["UnivUI.dll", "rvmm.dll", "Vm60.dll"] {
            let pe = Pe::open(&path.join(name))
                .with_context(|| format!("read serialization metadata from {name}"))?;
            modules.insert(name.to_ascii_lowercase(), pe);
        }
        let mut schema = Self::default();
        for module in modules.values() {
            for section in module.sections.iter().filter(|s| s.flags & 0x80000000 != 0) {
                for n in (0..section.size.saturating_sub(16)).step_by(4) {
                    let descriptor = section.va + n as u32;
                    let Ok(flags) = module.word(descriptor + 8) else {
                        continue;
                    };
                    if flags & 0x70000000 != 0x30000000 {
                        continue;
                    }
                    let Ok(name) = module.word(descriptor).and_then(|p| module.text(p)) else {
                        continue;
                    };
                    if !name.starts_with("OM_") && !name.starts_with("ONM_") {
                        continue;
                    }
                    let name = name.split('/').next().unwrap().to_owned();
                    let table = module.word(descriptor + 4)?;
                    match Self::read_fields(&modules, module, table) {
                        Ok(fields) => {
                            schema.messages.insert(name, fields);
                        }
                        Err(error) => {
                            schema.unresolved.insert(name, format!("{error:#}"));
                        }
                    }
                }
            }
        }
        ensure!(
            !schema.messages.is_empty(),
            "no native object message descriptors found"
        );
        Ok(schema)
    }
    fn pointer<'a>(
        modules: &'a BTreeMap<String, Pe>,
        local: &'a Pe,
        value: Value,
    ) -> Result<(&'a Pe, u32)> {
        match value {
            Value::Literal(va) => Ok((local, va)),
            Value::Import { dll, ordinal } => {
                let module = modules
                    .get(&dll)
                    .with_context(|| format!("descriptor imported from {dll} ordinal {ordinal}"))?;
                Ok((
                    module,
                    *module
                        .exports
                        .get(&ordinal)
                        .context("missing data export")?,
                ))
            }
        }
    }
    fn read_fields(
        modules: &BTreeMap<String, Pe>,
        module: &Pe,
        table: u32,
    ) -> Result<Vec<NativeField>> {
        let mut fields = Vec::new();
        if table == 0 {
            return Ok(fields);
        }
        for i in 0..256 {
            let address = table + i * 12;
            let name = module
                .value(address)
                .context("field name pointer missing")?;
            let typ = module
                .value(address + 4)
                .context("field type pointer missing")?;
            let offset = match module.value(address + 8) {
                Some(Value::Literal(n)) => n,
                _ => bail!("field offset missing"),
            };
            if matches!(name, Value::Literal(0)) && matches!(typ, Value::Literal(0)) {
                return Ok(fields);
            }
            let (m, p) = Self::pointer(modules, module, name)?;
            let name = m.text(p)?;
            let (m, p) = Self::pointer(modules, module, typ)?;
            let type_name = m.text(m.word(p)?)?;
            let size = m.word(p + 4)?;
            let flags = m.word(p + 8)? & 0x7fffffff;
            let kind = if flags & 0x70000000 >= 0x60000000 {
                FieldKind::Reference
            } else {
                match flags {
                    0x10000000 => FieldKind::String,
                    0x11000000 | 0x12000000 | 0x13000000 => FieldKind::U8,
                    0x14000000 | 0x15000000 => FieldKind::U16,
                    0x16000000 | 0x17000000 | 0x18000000 => FieldKind::U32,
                    0x19000000 => FieldKind::U64,
                    0x1b000000 => FieldKind::Variant,
                    0x1c000000 => FieldKind::NativeObject,
                    0x10500000 => FieldKind::InlineObject,
                    _ if size <= 4096 => FieldKind::Bytes(size as usize),
                    _ => bail!("unsupported field type {type_name}: {flags:#x}, size {size}"),
                }
            };
            fields.push(NativeField {
                name,
                type_name,
                offset,
                kind,
            });
        }
        bail!("native field table has no terminator")
    }
}
