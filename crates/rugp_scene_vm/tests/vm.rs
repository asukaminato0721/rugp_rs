use rugp_assets::object::{ClassId, ObjectRef};
use rugp_scene_vm::{
    Argument, Instruction, Scenario,
    vm::{Cursor, Program, Step, Value, Vm},
};
use std::sync::Arc;
fn reference(name: &str, offset: u64) -> Argument {
    Argument::Reference(ObjectRef {
        class: Some(ClassId {
            name: name.into(),
            schema: 0,
        }),
        cache_index: None,
        flags: 0xc108,
        offset: Some(offset),
        size: Some(7),
        owner: None,
        selector: 0,
        inline: None,
    })
}
fn instruction(name: &str, address: u32, next: u32, args: Vec<Argument>) -> Instruction {
    Instruction {
        index: 0,
        stream_offset: 0,
        class: ClassId {
            name: name.into(),
            schema: 17,
        },
        address,
        next_address: next,
        flags: 0,
        arguments: args,
    }
}
fn scene(id: usize, instructions: Vec<Instruction>) -> Scenario {
    Scenario {
        object_id: id,
        memory_size: 1024,
        instructions,
        null_records: Vec::new(),
        trailing_bytes: 0,
    }
}
fn immediate(value: i32) -> Argument {
    Argument::Value {
        descriptor: ((value as u32) << 2) | 2,
        arguments: Vec::new(),
    }
}
fn flag(operation: u32, src: Argument, dst: Argument, branch: u32) -> Vec<Argument> {
    vec![
        src,
        dst,
        reference("CLabel", 0),
        Argument::Number(operation),
        Argument::Number(1),
        Argument::Number(branch),
    ]
}
#[test]
fn calls_returns_values_and_host_acknowledgement() {
    let mut program = Program::default();
    program
        .insert(
            100,
            scene(
                1,
                vec![
                    instruction(
                        "CVmCall",
                        0,
                        16,
                        vec![reference("CRsa", 200), immediate(42)],
                    ),
                    instruction("CVmRet", 16, u32::MAX, vec![]),
                ],
            ),
        )
        .unwrap();
    program
        .insert(
            200,
            scene(
                2,
                vec![
                    instruction("CVmLabel", 0, 16, vec![reference("CLabel", 900)]),
                    instruction(
                        "CVmFlagOp",
                        16,
                        48,
                        flag(0, immediate(7), reference("CInt", 600), 48),
                    ),
                    instruction("CVmMsg", 48, 76, vec![Argument::Text("test".into())]),
                    instruction("CVmRet", 76, u32::MAX, vec![]),
                ],
            ),
        )
        .unwrap();
    let mut vm = Vm::new(Arc::new(program), 1).unwrap();
    vm.variables.insert(600, Value::Integer(0));
    assert!(vm.acknowledge_host().is_err());
    assert!(matches!(vm.step().unwrap(), Step::Advanced));
    assert_eq!(vm.call_depth(), 1);
    assert_eq!(vm.arguments()[0].integer().unwrap(), 42);
    vm.step().unwrap();
    vm.step().unwrap();
    assert_eq!(vm.variables[&600].integer().unwrap(), 7);
    for _ in 0..3 {
        assert!(matches!(vm.step().unwrap(), Step::Host { .. }));
        assert_eq!(vm.cursor().unwrap().address, 48);
    }
    vm.acknowledge_host().unwrap();
    vm.step().unwrap();
    assert_eq!(
        vm.cursor(),
        Some(&Cursor {
            scene: 1,
            address: 16
        })
    );
    assert!(matches!(vm.step().unwrap(), Step::Halted));
    assert!(matches!(vm.step().unwrap(), Step::Halted));
}
#[test]
fn conditions_use_signed_values_and_false_branch() {
    for (op, expected) in [
        (0x40, true),
        (0x41, false),
        (0x42, false),
        (0x43, true),
        (0x44, false),
        (0x45, false),
        (0x46, true),
        (0x47, true),
    ] {
        let mut program = Program::default();
        program
            .insert(
                100,
                scene(
                    1,
                    vec![
                        instruction(
                            "CVmFlagOp",
                            0,
                            32,
                            flag(op, immediate(2), reference("CInt", 600), 48),
                        ),
                        instruction("CVmRet", 32, u32::MAX, vec![]),
                        instruction("CVmRet", 48, u32::MAX, vec![]),
                    ],
                ),
            )
            .unwrap();
        let mut vm = Vm::new(Arc::new(program), 1).unwrap();
        vm.variables.insert(600, Value::Integer(-1));
        vm.step().unwrap();
        assert_eq!(vm.cursor().unwrap().address, if expected { 32 } else { 48 });
    }
}
#[test]
fn labels_invalid_pc_and_unknown_operations_are_not_skipped() {
    let mut program = Program::default();
    program
        .insert(
            100,
            scene(
                1,
                vec![
                    instruction("CVmJump", 0, 16, vec![reference("CLabel", 900)]),
                    instruction("CVmMsg", 16, 32, vec![]),
                    instruction("CVmLabel", 32, 48, vec![reference("CLabel", 900)]),
                    instruction("CVmRet", 48, u32::MAX, vec![]),
                ],
            ),
        )
        .unwrap();
    let mut vm = Vm::new(Arc::new(program), 1).unwrap();
    vm.step().unwrap();
    assert_eq!(vm.cursor().unwrap().address, 32);
    assert_eq!(
        vm.evaluate(&immediate(-42)).unwrap().integer().unwrap(),
        -42
    );
    let mut program = Program::default();
    program
        .insert(
            100,
            scene(
                1,
                vec![instruction("CVmJump", 0, 16, vec![reference("CRsa", 999)])],
            ),
        )
        .unwrap();
    let mut vm = Vm::new(Arc::new(program), 1).unwrap();
    assert!(vm.step().is_err());
    assert_eq!(vm.cursor().unwrap().address, 0);
}
