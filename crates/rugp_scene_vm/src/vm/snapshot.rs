//! Portable VM state, separate from rendering, input, profile files, and audio.
//! Hosts must snapshot their own state into the SAME reference writer and resume
//! pending operations before acknowledging them. A VM snapshot alone is not a
//! playable game save or a reason to skip a suspended host instruction.
use super::{Cursor, Frame, Program, Value, Vm};
use anyhow::{Context, Result, ensure};
use rugp_assets::object::{
    ObjectRef,
    snapshot::{ReferenceSnapshot, ReferenceSnapshotWriter},
};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    collections::HashSet,
    io::{BufWriter, Write},
    sync::Arc,
};

const MAX_VALUES: usize = 1_000_000;
const MAX_VALUE_BYTES: usize = 64 * 1024 * 1024;
const MAX_ARGUMENTS: usize = 65_536;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmSnapshot {
    version: u32,
    program_digest: [u8; 20],
    constants: Vec<u32>,
    frame: Option<SavedFrame>,
    stack: Vec<SavedFrame>,
    variables: Vec<(u64, SavedValue)>,
    shared_parameters: Vec<(usize, usize, SavedValue)>,
    pending_host: bool,
    synchronization_status: Option<bool>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedFrame {
    cursor: Cursor,
    arguments: Vec<SavedValue>,
}
#[derive(Debug, Serialize, Deserialize)]
enum SavedValue {
    Null,
    Integer(i32),
    Real(f64),
    Text(String),
    Bytes(Vec<u8>),
    Reference(u32),
}

#[derive(Default)]
struct ValueBudget {
    count: usize,
    bytes: usize,
}
impl ValueBudget {
    fn live_value(&mut self, value: &Value) -> Result<()> {
        let bytes = match value {
            Value::Real(value) => {
                ensure!(value.is_finite(), "nonfinite VM snapshot scalar");
                0
            }
            Value::Text(text) => text.len(),
            Value::Bytes(bytes) => bytes.len(),
            _ => 0,
        };
        self.reserve(bytes)
    }
    fn value(&mut self, value: &SavedValue) -> Result<()> {
        let bytes = match value {
            SavedValue::Real(value) => {
                ensure!(value.is_finite(), "nonfinite VM snapshot scalar");
                0
            }
            SavedValue::Text(text) => text.len(),
            SavedValue::Bytes(bytes) => bytes.len(),
            _ => 0,
        };
        self.reserve(bytes)
    }
    fn reserve(&mut self, bytes: usize) -> Result<()> {
        ensure!(
            self.count < MAX_VALUES,
            "VM snapshot value count exceeds limit"
        );
        ensure!(
            bytes <= MAX_VALUE_BYTES.saturating_sub(self.bytes),
            "VM snapshot scalar payload exceeds limit"
        );
        self.count += 1;
        self.bytes += bytes;
        Ok(())
    }
}

impl Vm {
    fn snapshot_program_digest(&self) -> Result<[u8; 20]> {
        if let Some(digest) = self.snapshot_program_digest.get() {
            return Ok(*digest);
        }
        let digest = program_digest(&self.program)?;
        let _ = self.snapshot_program_digest.set(digest);
        Ok(digest)
    }
    pub fn snapshot(&self, references: &mut ReferenceSnapshotWriter) -> Result<VmSnapshot> {
        let constants = program_constants(&self.program)
            .iter()
            .map(|r| references.insert(r))
            .collect::<Result<_>>()?;
        let mut budget = ValueBudget::default();
        let mut save = |value: &Value| -> Result<SavedValue> {
            budget.live_value(value)?;
            let saved = match value {
                Value::Null => SavedValue::Null,
                Value::Integer(v) => SavedValue::Integer(*v),
                Value::Real(v) => SavedValue::Real(*v),
                Value::Text(v) => SavedValue::Text(v.clone()),
                Value::Bytes(v) => SavedValue::Bytes(v.clone()),
                Value::Reference(r) => SavedValue::Reference(references.insert(r)?),
            };
            Ok(saved)
        };
        let mut frame = |frame: &Frame| -> Result<SavedFrame> {
            ensure!(
                frame.arguments.len() <= MAX_ARGUMENTS,
                "VM snapshot argument count exceeds limit"
            );
            Ok(SavedFrame {
                cursor: frame.cursor.clone(),
                arguments: frame
                    .arguments
                    .iter()
                    .map(&mut save)
                    .collect::<Result<_>>()?,
            })
        };
        ensure!(self.stack.len() <= 1024, "VM snapshot stack exceeds limit");
        let snapshot = VmSnapshot {
            version: 1,
            program_digest: self.snapshot_program_digest()?,
            constants,
            frame: self.frame.as_ref().map(&mut frame).transpose()?,
            stack: self.stack.iter().map(&mut frame).collect::<Result<_>>()?,
            variables: self
                .variables
                .iter()
                .map(|(key, value)| Ok((*key, save(value)?)))
                .collect::<Result<_>>()?,
            shared_parameters: self
                .shared_parameters
                .iter()
                .map(|((scene, index), value)| Ok((*scene, *index, save(value)?)))
                .collect::<Result<_>>()?,
            pending_host: self.pending_host,
            synchronization_status: self.synchronization_status,
        };
        snapshot.validate(&self.program)?;
        Ok(snapshot)
    }
}

impl VmSnapshot {
    /// Return both the VM and the shared reference roots for restoring the host.
    /// This constructs a new VM; failure never partly overwrites a running VM.
    pub fn restore(
        &self,
        program: Arc<Program>,
        references: &ReferenceSnapshot,
    ) -> Result<(Vm, Vec<ObjectRef>)> {
        self.validate(&program)?;
        ensure!(
            self.program_digest == program_digest(&program)?,
            "VM snapshot belongs to a different program"
        );
        let constants = program_constants(&program);
        ensure!(
            self.constants.len() == constants.len(),
            "VM snapshot script constant count differs"
        );
        let bindings = self
            .constants
            .iter()
            .copied()
            .zip(&constants)
            .collect::<Vec<_>>();
        let references = references.restore_with_bindings(&bindings)?;
        let restore = |value: &SavedValue| -> Result<Value> {
            Ok(match value {
                SavedValue::Null => Value::Null,
                SavedValue::Integer(v) => Value::Integer(*v),
                SavedValue::Real(v) => Value::Real(*v),
                SavedValue::Text(v) => Value::Text(v.clone()),
                SavedValue::Bytes(v) => Value::Bytes(v.clone()),
                SavedValue::Reference(index) => Value::Reference(
                    references
                        .get(*index as usize)
                        .context("VM snapshot reference root out of range")?
                        .clone(),
                ),
            })
        };
        let frame = |frame: &SavedFrame| -> Result<Frame> {
            Ok(Frame {
                cursor: frame.cursor.clone(),
                arguments: frame.arguments.iter().map(restore).collect::<Result<_>>()?,
            })
        };
        let vm = Vm {
            snapshot_program_digest: std::sync::OnceLock::from(self.program_digest),
            program,
            frame: self.frame.as_ref().map(frame).transpose()?,
            stack: self.stack.iter().map(frame).collect::<Result<_>>()?,
            variables: self
                .variables
                .iter()
                .map(|(key, value)| Ok((*key, restore(value)?)))
                .collect::<Result<_>>()?,
            shared_parameters: self
                .shared_parameters
                .iter()
                .map(|(scene, index, value)| Ok(((*scene, *index), restore(value)?)))
                .collect::<Result<_>>()?,
            pending_host: self.pending_host,
            synchronization_status: self.synchronization_status,
        };
        Ok((vm, references))
    }
    fn validate(&self, program: &Program) -> Result<()> {
        ensure!(self.version == 1, "unsupported VM snapshot version");
        ensure!(self.stack.len() <= 1024, "VM snapshot stack exceeds limit");
        ensure!(
            self.frame.is_some() || (self.stack.is_empty() && !self.pending_host),
            "halted VM snapshot has active stack/host state"
        );
        let mut budget = ValueBudget::default();
        for frame in self.frame.iter().chain(&self.stack) {
            program.instruction(&frame.cursor)?;
            ensure!(
                frame.arguments.len() <= MAX_ARGUMENTS,
                "VM snapshot argument count exceeds limit"
            );
            for value in &frame.arguments {
                budget.value(value)?;
            }
        }
        if self.pending_host {
            let frame = self
                .frame
                .as_ref()
                .context("pending VM snapshot has no frame")?;
            ensure!(
                !matches!(
                    program.instruction(&frame.cursor)?.class.name.as_str(),
                    "CVmRet" | "CVmLabel" | "CVmCall" | "CVmJump" | "CVmFlagOp"
                ),
                "VM snapshot suspends a core instruction as a host operation"
            );
        }
        let mut variables = HashSet::new();
        let mut reference_roots = HashSet::new();
        let mut unique_reference = |value: &SavedValue| -> Result<()> {
            if let SavedValue::Reference(index) = value {
                // Each captured value gets its own root (inline identities may
                // still alias). Reusing one giant root in millions of values
                // must not bypass the reference graph's expansion budget.
                ensure!(
                    reference_roots.insert(*index),
                    "reused VM snapshot reference root"
                );
            }
            Ok(())
        };
        for frame in self.frame.iter().chain(&self.stack) {
            for value in &frame.arguments {
                unique_reference(value)?;
            }
        }
        for (offset, value) in &self.variables {
            ensure!(variables.insert(*offset), "duplicate VM snapshot variable");
            budget.value(value)?;
            unique_reference(value)?;
        }
        ensure!(
            program.initial.keys().all(|key| variables.contains(key)),
            "VM snapshot omits initial variables"
        );
        let mut shared = HashSet::new();
        let shared_owners = program
            .scene_offsets
            .iter()
            .filter(|(offset, scene)| {
                program.declaration_scenes.contains(scene)
                    && !program.called_scene_offsets.contains(offset)
            })
            .map(|(_, scene)| *scene)
            .collect::<HashSet<_>>();
        for (scene, index, value) in &self.shared_parameters {
            ensure!(
                *index < MAX_ARGUMENTS && shared_owners.contains(scene),
                "invalid VM snapshot shared parameter"
            );
            ensure!(
                shared.insert((*scene, *index)),
                "duplicate VM snapshot shared parameter"
            );
            budget.value(value)?;
            unique_reference(value)?;
        }
        Ok(())
    }
}

fn program_constants(program: &Program) -> Vec<ObjectRef> {
    let mut identities = HashSet::new();
    let mut constants = Vec::new();
    let mut visit = |r: &ObjectRef| {
        if let Some(inline) = &r.inline
            && identities.insert(inline.identity())
        {
            constants.push(r.clone());
        }
    };
    for scene in program.scenes.values() {
        scene.visit_references(&mut visit);
    }
    for value in program.initial.values() {
        if let Value::Reference(r) = value {
            r.visit(&mut visit);
        }
    }
    constants
}

// A streaming compatibility fingerprint, not authentication of untrusted saves.
// Includes instruction bodies, scene offsets and initial values, without copying
// the complete serialized script into a temporary byte buffer or save file.
fn program_digest(program: &Program) -> Result<[u8; 20]> {
    struct HashWriter(Sha1);
    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut hash = BufWriter::with_capacity(64 * 1024, HashWriter(Sha1::new()));
    serde_json::to_writer(
        &mut hash,
        &(&program.scenes, &program.scene_offsets, &program.initial),
    )?;
    let hash = hash.into_inner().map_err(|error| error.into_error())?;
    Ok(hash.0.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Argument, Instruction, Scenario, vm::Step};
    use rugp_assets::object::{ClassId, InlineIdentity};

    fn reference(name: &str, offset: u64) -> ObjectRef {
        ObjectRef {
            class: Some(ClassId {
                name: name.into(),
                schema: 1,
            }),
            cache_index: None,
            flags: 0xc108,
            offset: Some(offset),
            size: Some(7),
            owner: None,
            selector: 0,
            inline: None,
        }
    }
    fn instruction(name: &str, address: u32, next: u32, arguments: Vec<Argument>) -> Instruction {
        Instruction {
            index: address / 32,
            stream_offset: 0,
            class: ClassId {
                name: name.into(),
                schema: 17,
            },
            address,
            next_address: next,
            flags: 0,
            arguments,
        }
    }
    fn program() -> Arc<Program> {
        let mut program = Program::default();
        let mut add = |offset, id, instructions| {
            program
                .insert(
                    offset,
                    Scenario {
                        object_id: id,
                        memory_size: 1024,
                        instructions,
                        null_records: Vec::new(),
                        trailing_bytes: 0,
                    },
                )
                .unwrap();
        };
        add(
            100,
            1,
            vec![
                instruction(
                    "CVmCall",
                    0,
                    32,
                    vec![
                        Argument::Reference(reference("CRsa", 200)),
                        Argument::Reference(reference("CVmVarObj", 901)),
                    ],
                ),
                instruction("CVmMsg", 32, 64, vec![]),
                instruction("CVmRet", 64, u32::MAX, vec![]),
            ],
        );
        add(
            200,
            2,
            vec![
                instruction("CVmSync", 0, 32, vec![]),
                instruction("CVmRet", 32, u32::MAX, vec![]),
            ],
        );
        add(300, 3, vec![instruction("CVmRet", 0, u32::MAX, vec![])]);
        program.initial.insert(900, Value::Integer(1));
        program.initial.insert(
            901,
            Value::Reference(ObjectRef::transient(ClassId {
                name: "CPostureMoment".into(),
                schema: 4,
            })),
        );
        Arc::new(program)
    }
    fn identity(value: &Value) -> InlineIdentity {
        let Value::Reference(r) = value else {
            panic!("expected reference");
        };
        r.inline.as_ref().unwrap().identity()
    }
    fn serialized(vm: &Vm) -> (VmSnapshot, ReferenceSnapshot) {
        let mut references = ReferenceSnapshotWriter::default();
        let state = vm.snapshot(&mut references).unwrap();
        (
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap(),
            serde_json::from_slice(&serde_json::to_vec(&references.finish().unwrap()).unwrap())
                .unwrap(),
        )
    }
    fn copy(state: &VmSnapshot) -> VmSnapshot {
        serde_json::from_slice(&serde_json::to_vec(state).unwrap()).unwrap()
    }

    #[test]
    fn suspended_call_restores_arguments_globals_shared_slots_status_and_host_identity() {
        let program = program();
        let mut vm = Vm::new(Arc::clone(&program), 1).unwrap();
        assert!(matches!(vm.step().unwrap(), Step::Advanced));
        assert!(matches!(vm.step().unwrap(), Step::Host { .. }));
        vm.synchronization_status = Some(false);
        vm.variables.insert(900, Value::Integer(42));
        let host = ObjectRef::transient(ClassId {
            name: "CGenericUso".into(),
            schema: 1,
        });
        vm.variables.insert(902, Value::Reference(host.clone()));
        vm.variables
            .insert(903, Value::Text("synthetic text".into()));
        vm.variables.insert(904, Value::Real(-1.25));
        vm.variables.insert(905, Value::Bytes(vec![1, 2, 3]));
        vm.variables.insert(906, Value::Null);
        vm.shared_parameters.insert((3, 7), Value::Integer(23));
        let mut writer = ReferenceSnapshotWriter::default();
        let host_root = writer.insert(&host).unwrap();
        let saved = vm.snapshot(&mut writer).unwrap();
        let graph = writer.finish().unwrap();
        let saved: VmSnapshot =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        let graph: ReferenceSnapshot =
            serde_json::from_slice(&serde_json::to_vec(&graph).unwrap()).unwrap();
        vm.variables.insert(900, Value::Integer(99));
        let new_program = self::program();
        let (mut restored, roots) = saved.restore(Arc::clone(&new_program), &graph).unwrap();
        assert_eq!(
            restored.cursor(),
            Some(&Cursor {
                scene: 2,
                address: 0
            })
        );
        assert_eq!(restored.call_depth(), 1);
        assert_eq!(restored.variables[&900].integer().unwrap(), 42);
        assert_eq!(restored.shared_parameters[&(3, 7)].integer().unwrap(), 23);
        assert_eq!(restored.synchronization_status, Some(false));
        assert_eq!(
            identity(&restored.arguments()[0]),
            identity(&new_program.initial[&901])
        );
        assert_eq!(
            identity(&restored.variables[&901]),
            identity(&new_program.initial[&901])
        );
        assert_ne!(
            identity(&restored.variables[&901]),
            identity(&program.initial[&901])
        );
        assert_eq!(
            identity(&restored.variables[&902]),
            roots[host_root as usize]
                .inline
                .as_ref()
                .unwrap()
                .identity()
        );
        assert_ne!(
            identity(&restored.variables[&902]),
            host.inline.as_ref().unwrap().identity()
        );
        assert!(matches!(&restored.variables[&903],Value::Text(s) if s == "synthetic text"));
        assert!(matches!(&restored.variables[&904],Value::Real(v) if *v == -1.25));
        assert!(matches!(&restored.variables[&905],Value::Bytes(v) if *v == [1,2,3]));
        assert!(matches!(&restored.variables[&906], Value::Null));
        for _ in 0..2 {
            assert!(
                matches!(restored.step().unwrap(),Step::Host {instruction,..} if instruction.class.name == "CVmSync")
            );
            assert_eq!(
                restored.cursor(),
                Some(&Cursor {
                    scene: 2,
                    address: 0
                })
            );
        }
        restored.acknowledge_sync_query(true).unwrap();
        assert_eq!(restored.synchronization_status, Some(true));
        assert!(matches!(restored.step().unwrap(), Step::Advanced));
        assert_eq!(restored.call_depth(), 0);
        assert!(
            matches!(restored.step().unwrap(),Step::Host {instruction,..} if instruction.class.name == "CVmMsg")
        );
        restored.acknowledge_host().unwrap();
        assert!(matches!(restored.step().unwrap(), Step::Halted));
        assert_eq!(
            restored.snapshot_program_digest.get(),
            Some(&saved.program_digest)
        );
    }

    #[test]
    fn unsuspended_and_halted_snapshots_do_not_invent_host_acknowledgements() {
        let program = program();
        let vm = Vm::new(Arc::clone(&program), 1).unwrap();
        let (state, refs) = serialized(&vm);
        let (mut restored, _) = state.restore(Arc::clone(&program), &refs).unwrap();
        assert!(restored.pending_instruction().is_none());
        assert!(restored.acknowledge_host().is_err());
        assert!(matches!(restored.step().unwrap(), Step::Advanced));
        assert_eq!(restored.call_depth(), 1);
        let mut vm = Vm::new(Arc::clone(&program), 3).unwrap();
        assert!(matches!(vm.step().unwrap(), Step::Halted));
        let (state, refs) = serialized(&vm);
        let (mut restored, _) = state.restore(program, &refs).unwrap();
        assert!(restored.cursor().is_none());
        assert!(matches!(restored.step().unwrap(), Step::Halted));
        assert!(restored.acknowledge_host().is_err());
    }

    #[test]
    fn changed_program_or_initial_values_cannot_load_compatible_looking_pcs() {
        let vm = Vm::new(program(), 1).unwrap();
        let (state, refs) = serialized(&vm);
        let mut changed = program();
        Arc::get_mut(&mut changed)
            .unwrap()
            .initial
            .insert(900, Value::Integer(2));
        assert!(
            state
                .restore(changed, &refs)
                .err()
                .unwrap()
                .to_string()
                .contains("different program")
        );
        let mut changed = program();
        Arc::get_mut(&mut changed)
            .unwrap()
            .scenes
            .get_mut(&1)
            .unwrap()
            .instructions[1]
            .flags = 1;
        assert!(
            state
                .restore(changed, &refs)
                .err()
                .unwrap()
                .to_string()
                .contains("different program")
        );
    }

    #[test]
    fn corrupt_frames_duplicate_state_and_reference_amplification_are_rejected() {
        let program = program();
        let vm = Vm::new(Arc::clone(&program), 1).unwrap();
        let (state, refs) = serialized(&vm);
        let mut bad = copy(&state);
        bad.frame.as_mut().unwrap().cursor.address = 999;
        assert!(bad.restore(Arc::clone(&program), &refs).is_err());
        let mut bad = copy(&state);
        bad.pending_host = true;
        assert!(bad.restore(Arc::clone(&program), &refs).is_err());
        let mut bad = copy(&state);
        bad.variables.push((900, SavedValue::Integer(0)));
        assert!(bad.restore(Arc::clone(&program), &refs).is_err());
        let mut bad = copy(&state);
        bad.variables.retain(|(offset, _)| *offset != 900);
        assert!(bad.restore(Arc::clone(&program), &refs).is_err());
        let mut bad = copy(&state);
        bad.variables.push((999, SavedValue::Reference(u32::MAX)));
        assert!(bad.restore(Arc::clone(&program), &refs).is_err());
        let mut bad = copy(&state);
        let reference = bad
            .variables
            .iter()
            .find_map(|(_, v)| {
                if let SavedValue::Reference(id) = v {
                    Some(*id)
                } else {
                    None
                }
            })
            .unwrap();
        bad.variables.push((999, SavedValue::Reference(reference)));
        assert!(
            bad.restore(Arc::clone(&program), &refs)
                .err()
                .unwrap()
                .to_string()
                .contains("reused")
        );
        let mut bad = copy(&state);
        bad.shared_parameters.push((1, 0, SavedValue::Null));
        assert!(bad.restore(Arc::clone(&program), &refs).is_err());
        let mut bad = copy(&state);
        bad.frame = None;
        bad.pending_host = true;
        assert!(bad.restore(Arc::clone(&program), &refs).is_err());
        let mut bad = copy(&state);
        bad.version = 9;
        assert!(bad.restore(program, &refs).is_err());
    }

    #[test]
    fn invalid_scalars_and_stack_limits_fail_before_a_snapshot_is_resumed() {
        let mut vm = Vm::new(program(), 1).unwrap();
        vm.variables.insert(999, Value::Real(f64::INFINITY));
        assert!(
            vm.snapshot(&mut ReferenceSnapshotWriter::default())
                .is_err()
        );
        vm.variables.remove(&999);
        let (state, refs) = serialized(&vm);
        let mut bad = copy(&state);
        bad.variables.push((999, SavedValue::Real(f64::NAN)));
        assert!(bad.restore(program(), &refs).is_err());
        let mut bad = copy(&state);
        bad.stack = (0..1025)
            .map(|_| SavedFrame {
                cursor: Cursor {
                    scene: 1,
                    address: 32,
                },
                arguments: Vec::new(),
            })
            .collect();
        assert!(bad.restore(program(), &refs).is_err());
        let mut budget = ValueBudget {
            count: MAX_VALUES,
            bytes: 0,
        };
        assert!(budget.value(&SavedValue::Null).is_err());
        let mut budget = ValueBudget {
            count: 0,
            bytes: MAX_VALUE_BYTES,
        };
        assert!(budget.value(&SavedValue::Text("x".into())).is_err());
        // Ensure deterministic ordering of fingerprint inputs rather than an
        // accidental dependency on hash-map iteration or allocation addresses.
        assert_eq!(
            program_digest(&program()).unwrap(),
            program_digest(&program()).unwrap()
        );
    }
}
