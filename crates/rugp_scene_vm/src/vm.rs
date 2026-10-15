//! Host-independent instruction scheduling, calls, labels and scalar state.
//! Rendering/audio/UI instructions yield to a host and are never silently skipped.
use crate::{Argument, Instruction, Scenario};
use anyhow::{Context, Result, bail, ensure};
use rugp_assets::{RioArchive, native_schema::NativeSchema, object::ObjectRef, script::Script};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub mod snapshot;

#[derive(Debug, Clone, serde::Serialize)]
pub enum Value {
    Null,
    Integer(i32),
    Real(f64),
    Text(String),
    Reference(ObjectRef),
    Bytes(Vec<u8>),
}
impl Value {
    pub fn integer(&self) -> Result<i32> {
        match self {
            Self::Integer(v) => Ok(*v),
            Self::Null => bail!("expected integer, got null"),
            Self::Real(_) => bail!("expected integer, got real"),
            Self::Text(_) => bail!("expected integer, got text"),
            Self::Bytes(_) => bail!("expected integer, got bytes"),
            Self::Reference(r) => bail!(
                "expected integer, got {} reference",
                r.class.as_ref().map_or("null", |c| c.name.as_str())
            ),
        }
    }
}
fn literal_value(type_name: &str, data: &[u8]) -> Result<Value> {
    Ok(match (type_name, data) {
        ("フラグ" | "バイト", [v]) => Value::Integer(*v as i32),
        ("短整数", [a, b]) => Value::Integer(i16::from_le_bytes([*a, *b]) as i32),
        ("短正整数", [a, b]) => Value::Integer(u16::from_le_bytes([*a, *b]) as i32),
        ("整数" | "正整数" | "色", [a, b, c, d]) => {
            Value::Integer(i32::from_le_bytes([*a, *b, *c, *d]))
        }
        ("低精度実数", [a, b, c, d]) => {
            Value::Real(f32::from_le_bytes([*a, *b, *c, *d]) as f64)
        }
        ("実数" | "高精度実数", bytes) if bytes.len() == 8 => {
            Value::Real(f64::from_le_bytes(bytes.try_into()?))
        }
        _ => bail!("unsupported runtime literal {type_name}"),
    })
}
fn initial_value(argument: &Argument) -> Result<Value> {
    Ok(match argument {
        Argument::Value {
            descriptor,
            arguments,
        } => match descriptor & 3 {
            2 => Value::Integer((*descriptor as i32) >> 2),
            0 | 1 => {
                return initial_value(arguments.first().context("empty initial variable value")?);
            }
            _ => bail!("unsupported initial variable descriptor {descriptor:#x}"),
        },
        Argument::Reference(r) if r.offset.is_none() && r.cache_index == Some(0) => Value::Null,
        Argument::Reference(r) => Value::Reference(r.clone()),
        Argument::Text(s) => Value::Text(s.clone()),
        Argument::Literal { type_name, data } => return literal_value(type_name, data),
        _ => bail!("invalid initial variable value"),
    })
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub scene: usize,
    pub address: u32,
}
#[derive(Default)]
pub struct Program {
    pub scenes: BTreeMap<usize, Scenario>,
    addresses: BTreeMap<(usize, u32), usize>,
    scene_offsets: BTreeMap<u64, usize>,
    labels: BTreeMap<u64, Cursor>,
    initial: BTreeMap<u64, Value>,
    declaration_scenes: BTreeSet<usize>,
    called_scene_offsets: BTreeSet<u64>,
}
impl Program {
    pub fn insert(&mut self, offset: u64, scene: Scenario) -> Result<()> {
        let id = scene.object_id;
        ensure!(
            !self.scenes.contains_key(&id) && !self.scene_offsets.contains_key(&offset),
            "duplicate scene identity"
        );
        if scene.instructions.iter().all(|i| i.class.name == "CVmRet") {
            self.declaration_scenes.insert(id);
        }
        for (index, ins) in scene.instructions.iter().enumerate() {
            if matches!(ins.class.name.as_str(), "CVmCall" | "CVmJump")
                && let Some(Argument::Reference(r)) = ins.arguments.first()
                && r.class.as_ref().is_some_and(|c| c.name == "CRsa")
                && let Some(offset) = r.offset
            {
                self.called_scene_offsets.insert(offset);
            }
            ensure!(
                self.addresses.insert((id, ins.address), index).is_none(),
                "duplicate command address"
            );
            if ins.class.name == "CVmLabel"
                && let Some(Argument::Reference(label)) = ins.arguments.first()
                && let Some(offset) = label.offset
            {
                let cursor = Cursor {
                    scene: id,
                    address: ins.address,
                };
                ensure!(
                    self.labels
                        .insert(offset, cursor.clone())
                        .is_none_or(|v| v == cursor),
                    "label defined at multiple addresses"
                );
            }
        }
        self.scene_offsets.insert(offset, id);
        self.scenes.insert(id, scene);
        Ok(())
    }
    pub fn load(archive: &RioArchive, native: &NativeSchema) -> Result<Self> {
        let mut program = Self::default();
        let mut variables = BTreeMap::new();
        for node in archive.nodes.iter().filter(|n| n.class.name == "CRsa") {
            let scene = Scenario::decode_with_schema(
                &Script::load(archive, node.id)?,
                archive.address_shift,
                Some(native),
            )?;
            scene.visit_references(&mut |r| {
                if let (Some(offset), Some(size), Some(class)) = (r.offset, r.size, &r.class)
                    && [
                        "CInt",
                        "CBool",
                        "CFlag",
                        "CByte",
                        "CShort",
                        "CUShort",
                        "CUInt",
                        "CVmVarObj",
                    ]
                    .contains(&class.name.as_str())
                {
                    variables.entry(offset).or_insert((size, class.clone()));
                }
            });
            program.insert(node.offset.context("scene extent missing")?, scene)?;
        }
        for (offset, (size, class)) in variables {
            let bytes = archive.read_at(offset, size as usize)?;
            if class.name == "CVmVarObj" {
                let argument = crate::scenario::decode_variable_value(
                    &bytes,
                    class.schema,
                    archive.address_shift,
                )
                .with_context(|| format!("decode variable object at {offset:#x}"))?;
                program.initial.insert(offset, initial_value(&argument)?);
                continue;
            }
            let payload = bytes.get(3..).context("scalar object prefix missing")?;
            let value = match (class.name.as_str(), payload) {
                ("CInt" | "CUInt", [a, b, c, d]) => i32::from_le_bytes([*a, *b, *c, *d]),
                ("CShort", [a, b]) => i16::from_le_bytes([*a, *b]) as i32,
                ("CUShort", [a, b]) => u16::from_le_bytes([*a, *b]) as i32,
                ("CBool" | "CFlag" | "CByte", [a]) => *a as i32,
                _ => bail!("unsupported {} scalar extent at {offset:#x}", class.name),
            };
            program.initial.insert(offset, Value::Integer(value));
        }
        Ok(program)
    }
    fn instruction(&self, cursor: &Cursor) -> Result<&Instruction> {
        let index = *self
            .addresses
            .get(&(cursor.scene, cursor.address))
            .with_context(|| format!("invalid PC {}:{:#x}", cursor.scene, cursor.address))?;
        Ok(&self.scenes[&cursor.scene].instructions[index])
    }
    pub fn startup(&self, archive: &RioArchive) -> Result<usize> {
        let process = archive
            .project
            .as_ref()
            .context("game project missing")?
            .references
            .get("processes")
            .context("process ocean missing")?;
        let data = archive.read_at(
            process.offset.context("process ocean offset missing")?,
            process.size.context("process ocean extent missing")? as usize,
        )?;
        let process = rugp_assets::process::ProcessOcean::parse(&data, archive.address_shift)?;
        Ok(self.target(&process.references[1])?.scene)
    }
    fn target(&self, reference: &ObjectRef) -> Result<Cursor> {
        ensure!(
            reference.selector == 0,
            "unsupported control-flow selector {}",
            reference.selector
        );
        let offset = reference
            .offset
            .context("control-flow target has no stored identity")?;
        match reference.class.as_ref().map(|c| c.name.as_str()) {
            Some("CRsa") => Ok(Cursor {
                scene: *self
                    .scene_offsets
                    .get(&offset)
                    .context("target scene not loaded")?,
                address: 0,
            }),
            Some("CLabel") => self
                .labels
                .get(&offset)
                .cloned()
                .context("jump label has no definition"),
            other => bail!("unsupported control-flow target {other:?}"),
        }
    }
}

#[derive(Debug, Clone)]
struct Frame {
    cursor: Cursor,
    arguments: Vec<Value>,
}
enum Place {
    Global(u64),
    Parameter {
        depth: usize,
        index: usize,
    },
    SharedParameter {
        scene: usize,
        index: usize,
        default: Box<Value>,
    },
}
pub struct Message {
    pub class: String,
    pub target: Value,
    pub fields: BTreeMap<String, Value>,
}
#[derive(Debug)]
pub enum Step {
    Advanced,
    Host {
        cursor: Cursor,
        instruction: Instruction,
    },
    Halted,
}
pub struct Vm {
    program: Arc<Program>,
    /// Only this VM owns the cache: Program's public scene collection may be
    /// edited once all VMs using it are dropped, so Program cannot cache it.
    snapshot_program_digest: std::sync::OnceLock<[u8; 20]>,
    frame: Option<Frame>,
    stack: Vec<Frame>,
    pub variables: BTreeMap<u64, Value>,
    pending_host: bool,
    shared_parameters: BTreeMap<(usize, usize), Value>,
    /// Native process status written by CVmSync's nonblocking query mode.
    pub synchronization_status: Option<bool>,
}
impl Vm {
    pub fn new(program: Arc<Program>, entry: usize) -> Result<Self> {
        let cursor = Cursor {
            scene: entry,
            address: 0,
        };
        program.instruction(&cursor)?;
        Ok(Self {
            variables: program.initial.clone(),
            program,
            snapshot_program_digest: std::sync::OnceLock::new(),
            frame: Some(Frame {
                cursor,
                arguments: Vec::new(),
            }),
            stack: Vec::new(),
            pending_host: false,
            shared_parameters: BTreeMap::new(),
            synchronization_status: None,
        })
    }
    pub fn cursor(&self) -> Option<&Cursor> {
        self.frame.as_ref().map(|f| &f.cursor)
    }
    pub fn arguments(&self) -> &[Value] {
        self.frame.as_ref().map_or(&[], |f| &f.arguments)
    }
    pub fn call_depth(&self) -> usize {
        self.stack.len()
    }
    /// Inspect a failed/suspended host operation without executing another step.
    pub fn pending_instruction(&self) -> Option<&Instruction> {
        if !self.pending_host {
            return None;
        }
        self.frame
            .as_ref()
            .and_then(|frame| self.program.instruction(&frame.cursor).ok())
    }
    fn parameter_place(&self, r: &ObjectRef) -> Result<Option<Place>> {
        if r.flags & 7 != 1 {
            return Ok(None);
        }
        let Some(owner) = &r.owner else {
            return Ok(None);
        };
        if owner.class.as_ref().is_none_or(|c| c.name != "CRsa") {
            return Ok(None);
        }
        let scene = *self
            .program
            .scene_offsets
            .get(&owner.offset.context("parameter owner has no offset")?)
            .context("parameter owner scene is not loaded")?;
        let depth = std::iter::once(self.frame.as_ref())
            .chain(self.stack.iter().rev().map(Some))
            .position(|f| f.is_some_and(|f| f.cursor.scene == scene));
        // Empty, never-called CRsa objects also declare shared typed slots.
        // Retail helper 211 accesses slots owned by inert declaration 208;
        // these are not arguments belonging to its active caller (482).
        if depth.is_none()
            && self.program.declaration_scenes.contains(&scene)
            && !self
                .program
                .called_scene_offsets
                .contains(&owner.offset.unwrap())
        {
            let default = match r.class.as_ref().map(|c| c.name.as_str()) {
                Some(
                    "フラグ" | "バイト" | "短整数" | "短正整数" | "整数" | "正整数" | "色" | "CInt"
                    | "CUInt" | "CShort" | "CUShort" | "CFlag" | "CBool" | "CByte",
                ) => Value::Integer(0),
                Some("低精度実数" | "実数" | "高精度実数") => Value::Real(0.0),
                Some("文字列") => Value::Text(String::new()),
                _ => Value::Null,
            };
            ensure!(r.selector < 65536, "shared parameter slot exceeds limit");
            return Ok(Some(Place::SharedParameter {
                scene,
                index: r.selector as usize,
                default: Box::new(default),
            }));
        }
        let depth = depth.with_context(|| format!("parameter owner scene {scene}, slot {} has no active call frame; active scenes {:?}", r.selector,
                self.frame.iter().chain(self.stack.iter().rev()).map(|f| f.cursor.scene).collect::<Vec<_>>()))?;
        Ok(Some(Place::Parameter {
            depth,
            index: r.selector as usize,
        }))
    }
    fn read_parameter_place(&self, place: Place) -> Result<Value> {
        match place {
            Place::Parameter { depth, index } => self.parameter(depth, index).cloned(),
            Place::SharedParameter {
                scene,
                index,
                default,
            } => Ok(self
                .shared_parameters
                .get(&(scene, index))
                .cloned()
                .unwrap_or(*default)),
            Place::Global(_) => bail!("global place is not a parameter"),
        }
    }
    fn parameter(&self, depth: usize, index: usize) -> Result<&Value> {
        let frame = if depth == 0 {
            self.frame.as_ref().context("no active frame")?
        } else {
            self.stack
                .get(
                    self.stack
                        .len()
                        .checked_sub(depth)
                        .context("invalid call depth")?,
                )
                .context("invalid parameter frame")?
        };
        frame
            .arguments
            .get(index)
            .with_context(|| format!("argument {index} missing in scene {}", frame.cursor.scene))
    }
    fn assign(&mut self, place: Place, value: Value) -> Result<()> {
        match place {
            Place::SharedParameter { scene, index, .. } => {
                self.shared_parameters.insert((scene, index), value);
            }
            Place::Global(offset) => {
                self.variables.insert(offset, value);
            }
            Place::Parameter { depth, index } => {
                ensure!(index < 65536, "parameter slot exceeds frame limit");
                let frame = if depth == 0 {
                    self.frame.as_mut().context("no active frame")?
                } else {
                    let position = self
                        .stack
                        .len()
                        .checked_sub(depth)
                        .context("invalid call depth")?;
                    self.stack
                        .get_mut(position)
                        .context("invalid parameter frame")?
                };
                // Script helpers also use parameter slots as locals. A plain
                // assignment can initialize a slot not supplied by the caller.
                if index >= frame.arguments.len() {
                    frame.arguments.resize(index + 1, Value::Null);
                }
                frame.arguments[index] = value;
            }
        }
        Ok(())
    }
    pub fn evaluate(&self, arg: &Argument) -> Result<Value> {
        Ok(match arg {
            Argument::Number(n) => Value::Integer(*n as i32),
            Argument::Text(s) => Value::Text(s.clone()),
            Argument::Bytes(bytes) => Value::Bytes(bytes.clone()),
            Argument::Field { argument, .. } => return self.evaluate(argument),
            Argument::Reference(r) => {
                if let Some(place) = self.parameter_place(r)? {
                    return self.read_parameter_place(place);
                }
                if let Some(offset) = r.offset {
                    self.variables
                        .get(&offset)
                        .cloned()
                        .unwrap_or_else(|| Value::Reference(r.clone()))
                } else if r.cache_index == Some(0) {
                    Value::Null
                } else {
                    Value::Reference(r.clone())
                }
            }
            Argument::Value {
                descriptor,
                arguments,
            } => match descriptor & 3 {
                2 => Value::Integer((*descriptor as i32) >> 2),
                0 | 1 => {
                    return self
                        .evaluate(arguments.first().context("empty reference/string value")?);
                }
                _ => bail!("unsupported runtime value descriptor {descriptor:#x}"),
            },
            Argument::Literal { type_name, data } => return literal_value(type_name, data),
            _ => bail!("argument is not an evaluable value"),
        })
    }
    fn destination(&self, arg: &Argument) -> Result<Place> {
        match arg {
            Argument::Reference(r) => {
                if let Some(place) = self.parameter_place(r)? {
                    return Ok(place);
                }
                ensure!(
                    r.flags & 7 == 0,
                    "object member assignment requires host property binding"
                );
                Ok(Place::Global(
                    r.offset
                        .context("assignment target has no stored identity")?,
                ))
            }
            Argument::Value {
                descriptor,
                arguments,
            } if descriptor & 3 == 0 => {
                self.destination(arguments.first().context("empty assignment target")?)
            }
            _ => bail!("assignment target is not a variable reference"),
        }
    }
    /// Resolve an object pointer without reading a scalar object's contents.
    /// Native reference fields (including output destinations) use this path;
    /// valiant/value fields continue to use `evaluate`.
    pub fn resolve_object(&self, arg: &Argument) -> Result<Value> {
        let value = match arg {
            Argument::Reference(reference) => {
                if let Some(place) = self.parameter_place(reference)? {
                    self.read_parameter_place(place)?
                } else if reference.offset.is_none() && reference.cache_index == Some(0) {
                    Value::Null
                } else {
                    Value::Reference(reference.clone())
                }
            }
            Argument::Value {
                descriptor,
                arguments,
            } if descriptor & 3 == 0 => {
                return self.resolve_object(arguments.first().context("empty object value")?);
            }
            _ => self.evaluate(arg)?,
        };
        ensure!(
            matches!(value, Value::Reference(_) | Value::Null),
            "object field is not a reference"
        );
        Ok(value)
    }
    /// Host-side scalar assignment uses the same scoped/global destinations as
    /// VM operations. It does not acknowledge or advance a pending instruction.
    pub fn write_object(&mut self, reference: &ObjectRef, value: Value) -> Result<()> {
        let destination = self.destination(&Argument::Reference(reference.clone()))?;
        self.assign(destination, value)
    }
    /// Resolve native defaults and the VM's named/indexed parameter overrides.
    pub fn message(&self, ins: &Instruction) -> Result<Message> {
        ensure!(
            ins.class.name == "CVmGenericMsg",
            "instruction is not an object message"
        );
        let target =
            self.resolve_object(ins.arguments.first().context("missing message target")?)?;
        let Some(Argument::Message { class, arguments }) = ins.arguments.get(1) else {
            bail!("missing message body");
        };
        let mut fields = BTreeMap::new();
        let mut order = Vec::new();
        let mut reference_fields = Vec::new();
        for field in arguments {
            let Argument::Field { name, argument, .. } = field else {
                bail!("message body lacks native field metadata");
            };
            let value = if matches!(argument.as_ref(), Argument::Reference(_)) {
                reference_fields.push(name.clone());
                self.resolve_object(argument)?
            } else {
                self.evaluate(argument)?
            };
            fields.insert(name.clone(), value);
            order.push(name.clone());
        }
        for parameter in ins.arguments.iter().skip(3) {
            let Argument::Parameter(args) = parameter else {
                bail!("invalid message override record");
            };
            let name = if let Some(Argument::Text(name)) = args.get(args.len().saturating_sub(2)) {
                name.strip_prefix('$').unwrap_or(name).to_owned()
            } else {
                let index = args
                    .iter()
                    .find_map(|a| {
                        if let Argument::Number(index) = a {
                            Some(*index as usize)
                        } else {
                            None
                        }
                    })
                    .context("message override has no field identifier")?;
                order
                    .get(index)
                    .context("message override index outside field list")?
                    .clone()
            };
            ensure!(
                fields.contains_key(&name),
                "message {} has no field {name}",
                class.name
            );
            let value = if reference_fields.contains(&name) {
                self.resolve_object(args.last().context("empty message override")?)?
            } else {
                self.evaluate(args.last().context("empty message override")?)?
            };
            fields.insert(name, value);
        }
        Ok(Message {
            class: class
                .name
                .strip_prefix("&-")
                .unwrap_or(&class.name)
                .to_owned(),
            target,
            fields,
        })
    }
    fn advance(&mut self) -> Result<()> {
        let frame = self.frame.as_mut().context("VM is halted")?;
        let next = self.program.instruction(&frame.cursor)?.next_address;
        ensure!(next != u32::MAX, "instruction chain ended without return");
        frame.cursor.address = next;
        self.program.instruction(&frame.cursor)?;
        Ok(())
    }
    /// A host must acknowledge only operations it actually implemented.
    pub fn acknowledge_host(&mut self) -> Result<()> {
        ensure!(self.pending_host, "no pending host instruction");
        self.advance()?;
        self.pending_host = false;
        Ok(())
    }
    pub fn acknowledge_sync_query(&mut self, completed: bool) -> Result<()> {
        ensure!(self.pending_host, "no pending synchronization query");
        ensure!(
            self.program
                .instruction(&self.frame.as_ref().context("VM is halted")?.cursor)?
                .class
                .name
                == "CVmSync",
            "status completion is not a synchronization"
        );
        self.acknowledge_host()?;
        self.synchronization_status = Some(completed);
        Ok(())
    }
    pub fn step(&mut self) -> Result<Step> {
        let Some(frame) = &self.frame else {
            return Ok(Step::Halted);
        };
        let cursor = frame.cursor.clone();
        let ins = self.program.instruction(&cursor)?.clone();
        if self.pending_host {
            return Ok(Step::Host {
                cursor,
                instruction: ins,
            });
        }
        match ins.class.name.as_str() {
            "CVmRet" => {
                self.frame = self.stack.pop();
                return Ok(if self.frame.is_some() {
                    Step::Advanced
                } else {
                    Step::Halted
                });
            }
            "CVmLabel" => self.advance()?,
            "CVmJump" | "CVmCall" => {
                let Some(Argument::Reference(reference)) = ins.arguments.first() else {
                    bail!("missing jump/call target");
                };
                let target = self.program.target(reference)?;
                self.program.instruction(&target)?;
                if ins.class.name == "CVmCall" {
                    ensure!(self.stack.len() < 1024, "VM call stack limit exceeded");
                    let arguments = ins.arguments[1..]
                        .iter()
                        .map(|v| self.evaluate(v))
                        .collect::<Result<Vec<_>>>()?;
                    self.advance()?;
                    self.stack.push(
                        self.frame
                            .replace(Frame {
                                cursor: target,
                                arguments,
                            })
                            .unwrap(),
                    );
                } else {
                    self.frame.as_mut().unwrap().cursor = target;
                }
            }
            "CVmFlagOp" => {
                ensure!(
                    ins.arguments.len() == 6,
                    "invalid flag operation argument count"
                );
                let operation = self.evaluate(&ins.arguments[3])?.integer()?;
                let mode = self.evaluate(&ins.arguments[4])?.integer()?;
                ensure!(mode == 1, "unsupported flag branch mode {mode}");
                // Vm60 100720c2 separates object operations at 0x50 before
                // scalar evaluation. 10072280..1007230e assigns the resolved
                // object to CVmVarObj (or the selected parameter slot), without
                // first converting either object pointer to an integer.
                if operation == 0x50 {
                    let source = self.evaluate(&ins.arguments[0])?;
                    ensure!(
                        matches!(source, Value::Reference(_) | Value::Null),
                        "object assignment source is not a reference"
                    );
                    let destination = self.destination(&ins.arguments[1])?;
                    self.assign(destination, source)?;
                    self.advance()?;
                    return Ok(Step::Advanced);
                }
                let source = if operation == 0x40 || operation == 0x41 {
                    0
                } else {
                    self.evaluate(&ins.arguments[0])?.integer()?
                };
                let current = if operation == 0 {
                    0 // assignment does not read the destination's old value
                } else {
                    self.evaluate(&ins.arguments[1])?.integer()?
                };
                let next = match operation {
                    0 => Some(source),
                    1 => Some(current.wrapping_add(source)),
                    2 => Some(current.wrapping_sub(source)),
                    3 => Some(current.wrapping_mul(source)),
                    4 => Some(
                        current
                            .checked_div(source)
                            .context("VM division overflow or zero divisor")?,
                    ),
                    5 => Some(
                        current
                            .checked_rem(source)
                            .context("VM remainder overflow or zero divisor")?,
                    ),
                    0x40..=0x47 => None,
                    _ => bail!("unsupported flag operation {operation:#x}"),
                };
                if let Some(value) = next {
                    let destination = self.destination(&ins.arguments[1])?;
                    self.assign(destination, Value::Integer(value))?;
                    self.advance()?;
                } else {
                    let predicate = match operation {
                        0x40 => current != 0,
                        0x41 => current == 0,
                        0x42 => current == source,
                        0x43 => current != source,
                        0x44 => current > source,
                        0x45 => current >= source,
                        0x46 => current < source,
                        0x47 => current <= source,
                        _ => unreachable!(),
                    };
                    if predicate {
                        self.advance()?;
                    } else {
                        let address = self.evaluate(&ins.arguments[5])?.integer()? as u32;
                        let target = Cursor {
                            scene: cursor.scene,
                            address,
                        };
                        self.program.instruction(&target)?;
                        self.frame.as_mut().unwrap().cursor = target;
                    }
                }
            }
            _ => {
                self.pending_host = true;
                return Ok(Step::Host {
                    cursor,
                    instruction: ins,
                });
            }
        }
        Ok(Step::Advanced)
    }
}
