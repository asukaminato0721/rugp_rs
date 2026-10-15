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
#[test]
fn synchronization_queries_acknowledge_and_preserve_the_status_value() {
    let mut program = Program::default();
    program
        .insert(
            100,
            scene(
                1,
                vec![
                    instruction(
                        "CVmSync",
                        0,
                        24,
                        vec![
                            reference("CPmEffect", 700),
                            Argument::Number(32),
                            Argument::Number(0),
                        ],
                    ),
                    instruction(
                        "CVmSync",
                        24,
                        48,
                        vec![
                            reference("CPmEffect", 700),
                            Argument::Number(32),
                            Argument::Number(0),
                        ],
                    ),
                    instruction("CVmMsg", 48, 72, vec![Argument::Text("example".into())]),
                    instruction("CVmRet", 72, u32::MAX, vec![]),
                ],
            ),
        )
        .unwrap();
    let mut vm = Vm::new(Arc::new(program), 1).unwrap();
    assert!(vm.acknowledge_sync_query(true).is_err());
    assert!(matches!(vm.step().unwrap(), Step::Host { .. }));
    vm.acknowledge_sync_query(false).unwrap();
    assert_eq!(vm.synchronization_status, Some(false));
    assert_eq!(vm.cursor().unwrap().address, 24);
    assert!(matches!(vm.step().unwrap(), Step::Host { .. }));
    vm.acknowledge_sync_query(true).unwrap();
    assert_eq!(vm.synchronization_status, Some(true));
    assert!(matches!(vm.step().unwrap(), Step::Host { .. }));
    assert!(vm.acknowledge_sync_query(false).is_err());
    assert_eq!(vm.cursor().unwrap().address, 48);
    assert_eq!(vm.synchronization_status, Some(true));
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
fn object_assignment_copies_values_without_integer_coercion_and_preserves_scope() {
    let Argument::Reference(owner) = reference("CRsa", 200) else {
        unreachable!()
    };
    let Argument::Reference(mut r) = reference("CPostureMoment", 700) else {
        unreachable!()
    };
    r.flags |= 1;
    r.owner = Some(Box::new(owner));
    let parameter = Argument::Reference(r);
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
                        32,
                        vec![reference("CRsa", 200), reference("CPostureMoment", 900)],
                    ),
                    instruction("CVmRet", 32, u32::MAX, vec![]),
                ],
            ),
        )
        .unwrap();
    let null = Argument::Reference(
        rugp_assets::object::ObjectReader::new(&[0; 3])
            .reference(0)
            .unwrap(),
    );
    program
        .insert(
            200,
            scene(
                2,
                vec![
                    instruction(
                        "CVmFlagOp",
                        0,
                        32,
                        flag(0x50, parameter.clone(), reference("CVmVarObj", 600), 32),
                    ),
                    instruction(
                        "CVmFlagOp",
                        32,
                        64,
                        flag(
                            0x50,
                            reference("CVmVarObj", 600),
                            reference("CVmVarObj", 601),
                            64,
                        ),
                    ),
                    instruction("CVmFlagOp", 64, 96, flag(0x50, null, parameter.clone(), 96)),
                    instruction(
                        "CVmFlagOp",
                        96,
                        128,
                        flag(0x50, parameter, reference("CVmVarObj", 602), 128),
                    ),
                    instruction(
                        "CVmFlagOp",
                        128,
                        160,
                        flag(0x50, immediate(7), reference("CVmVarObj", 600), 160),
                    ),
                    instruction("CVmRet", 160, u32::MAX, vec![]),
                ],
            ),
        )
        .unwrap();
    let mut vm = Vm::new(Arc::new(program), 1).unwrap();
    for _ in 0..5 {
        assert!(matches!(vm.step().unwrap(), Step::Advanced));
    }
    for offset in [600, 601] {
        let Value::Reference(r) = &vm.variables[&offset] else {
            panic!("lost object value");
        };
        assert_eq!(r.offset, Some(900));
        assert_eq!(r.class.as_ref().unwrap().name, "CPostureMoment");
    }
    assert!(matches!(vm.variables[&602], Value::Null));
    assert!(matches!(vm.arguments()[0], Value::Null));
    assert!(!vm.variables.contains_key(&700));
    assert!(vm.step().is_err());
    assert_eq!(vm.cursor().unwrap().address, 128);
    assert!(matches!(vm.variables[&600], Value::Reference(_)));
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

#[test]
fn scoped_arguments_and_message_overrides() {
    let Argument::Reference(owner) = reference("CRsa", 200) else {
        unreachable!()
    };
    let Argument::Reference(mut r) = reference("整数", 700) else {
        unreachable!()
    };
    r.flags |= 1;
    r.selector = 0;
    r.owner = Some(Box::new(owner));
    let param = Argument::Reference(r);
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
                        vec![reference("CRsa", 200), immediate(41)],
                    ),
                    instruction(
                        "CVmCall",
                        16,
                        32,
                        vec![reference("CRsa", 200), immediate(10)],
                    ),
                    instruction("CVmRet", 32, u32::MAX, vec![]),
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
                    instruction("CVmFlagOp", 0, 32, flag(1, immediate(1), param.clone(), 32)),
                    instruction(
                        "CVmGenericMsg",
                        32,
                        64,
                        vec![
                            reference("CImgBox", 300),
                            Argument::Message {
                                class: ClassId {
                                    name: "&-OM_Test".into(),
                                    schema: 0,
                                },
                                arguments: vec![Argument::Field {
                                    name: "x".into(),
                                    offset: 8,
                                    argument: Box::new(Argument::Number(0)),
                                }],
                            },
                            Argument::Number(0),
                            Argument::Parameter(vec![
                                Argument::Text("&-OM_Test".into()),
                                Argument::Number(65535),
                                Argument::Text("$x".into()),
                                param.clone(),
                            ]),
                        ],
                    ),
                    instruction("CVmRet", 64, u32::MAX, vec![]),
                ],
            ),
        )
        .unwrap();
    let mut vm = Vm::new(Arc::new(program), 1).unwrap();
    assert!(vm.evaluate(&param).is_err()); // no leaked/implicit global argument
    for expected in [42, 11] {
        vm.step().unwrap();
        vm.step().unwrap();
        let Step::Host { instruction, .. } = vm.step().unwrap() else {
            panic!("expected message");
        };
        let message = vm.message(&instruction).unwrap();
        assert_eq!(message.class, "OM_Test");
        assert_eq!(message.fields["x"].integer().unwrap(), expected);
        vm.acknowledge_host().unwrap();
        vm.step().unwrap();
        assert!(vm.arguments().is_empty());
        assert!(!vm.variables.contains_key(&700));
    }
    assert!(matches!(vm.step().unwrap(), Step::Halted));
}

#[test]
fn message_output_references_are_not_scalar_values() {
    let mut program = Program::default();
    let message = instruction(
        "CVmGenericMsg",
        0,
        28,
        vec![
            reference("CStdb", 900),
            Argument::Message {
                class: ClassId {
                    name: "&-OM_AccessGetValue".into(),
                    schema: 0,
                },
                arguments: vec![
                    Argument::Field {
                        name: "destination".into(),
                        offset: 16,
                        argument: Box::new(reference("CInt", 600)),
                    },
                    Argument::Field {
                        name: "value".into(),
                        offset: 20,
                        argument: Box::new(Argument::Value {
                            descriptor: 0,
                            arguments: vec![reference("CInt", 600)],
                        }),
                    },
                ],
            },
            Argument::Number(0),
            Argument::Parameter(vec![
                Argument::Text("&-OM_AccessGetValue".into()),
                Argument::Number(0),
                Argument::Value {
                    descriptor: 0,
                    arguments: vec![reference("CInt", 700)],
                },
            ]),
        ],
    );
    program
        .insert(
            100,
            scene(
                1,
                vec![message.clone(), instruction("CVmRet", 28, u32::MAX, vec![])],
            ),
        )
        .unwrap();
    let mut vm = Vm::new(Arc::new(program), 1).unwrap();
    vm.variables.insert(600, Value::Integer(12));
    vm.variables.insert(700, Value::Integer(5));
    let resolved = vm.message(&message).unwrap();
    assert_eq!(resolved.fields["value"].integer().unwrap(), 12);
    let Value::Reference(destination) = &resolved.fields["destination"] else {
        panic!("output pointer was dereferenced");
    };
    assert_eq!(destination.offset, Some(700));
    vm.step().unwrap();
    vm.write_object(destination, Value::Integer(42)).unwrap();
    assert_eq!(vm.variables[&700].integer().unwrap(), 42);
    assert_eq!(vm.cursor().unwrap().address, 0); // write does not acknowledge
    vm.acknowledge_host().unwrap();
    assert_eq!(vm.variables[&600].integer().unwrap(), 12);
}
