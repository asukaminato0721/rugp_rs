use anyhow::{Context, Result};
use rugp_assets::object::{ClassId, ObjectRef};
use rugp_runtime::{Game, RunState};
use rugp_scene_vm::{
    Argument, Instruction, Scenario,
    vm::{Program, Vm},
};
use std::sync::Arc;

fn reference(name: &str, schema: u16, offset: u64, size: u32) -> ObjectRef {
    ObjectRef {
        class: Some(ClassId {
            name: name.into(),
            schema,
        }),
        cache_index: None,
        flags: 0xc108,
        offset: Some(offset),
        size: Some(size),
        owner: None,
        selector: 0,
        inline: None,
    }
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn native_background_geometry_and_instant_camera_pan_reach_real_pixels() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let mut game = Game::open(root.as_ref())?;
    let bg = reference("CBg2d", 6, 13570189, 3577);
    let bytes = game
        .archive
        .read_at(bg.offset.unwrap(), bg.size.unwrap() as usize)?;
    let background =
        rugp_assets::background::Background::parse(&bytes, 6, game.archive.address_shift)?;
    assert_eq!(background.placement().scale_q16, 8 << 16);
    assert_eq!(background.placement().origin, [600, 600]);
    assert_eq!(background.placement().depth, 5000);
    let mut images = rugp_runtime::image_manager::ImageManager::new(Arc::clone(&game.archive));
    let image = images.load_world(&bg)?;
    let texture = &image.planes[0].texture;
    let plane = image.planes[0].plane;
    assert_eq!((texture.width, texture.height), (1200, 1200));
    let quad = plane.quad(&rugp_runtime::world::Camera::default(), (0, 0, 1024, 600))?;
    assert_eq!(
        quad[0],
        [-11744051.0 / 65536.0, -25637683.0 / 65536.0, 5000.0]
    );

    let logo = reference("Cr6Ti", 3, 0xcdefba, 23171);
    let target = reference("CParseBgBox", 15, 212259, 118);
    let source = rugp_assets::image::decode_r6ti(
        &game.archive.read_at(0xcdefba, 23171)?,
        game.archive.address_shift,
    )?;
    let field = |name: &str, argument: Argument| Argument::Field {
        name: name.into(),
        offset: 0,
        argument: Box::new(argument),
    };
    let n = |name: &str, value: i32| field(name, Argument::Number(value as u32));
    let message = |name: &str, fields: Vec<Argument>| {
        vec![
            Argument::Reference(target.clone()),
            Argument::Message {
                class: ClassId {
                    name: name.into(),
                    schema: 0,
                },
                arguments: fields,
            },
            Argument::Number(0),
        ]
    };
    let mut instructions = Vec::new();
    let mut add = |name: &str, arguments: Vec<Argument>| {
        let address = instructions.len() as u32 * 64;
        instructions.push(Instruction {
            index: instructions.len() as u32,
            stream_offset: 0,
            class: ClassId {
                name: name.into(),
                schema: 17,
            },
            address,
            next_address: address + 64,
            flags: 0,
            arguments,
        });
    };
    add(
        "CVmGenericMsg",
        message("OM_SetWorldCameraAsScreen", vec![n("ズーム率", 1440)]),
    );
    add(
        "CVmGenericMsg",
        message(
            "OM_HumanIn2",
            vec![
                n("スタイル", 512),
                field("ベース画像", Argument::Reference(logo)),
            ],
        ),
    );
    add(
        "CVmGenericMsg",
        message(
            "OM_SetWorldCamera",
            vec![n("ピッチ角", -2), n("総処理時間基", 400)],
        ),
    );
    add("CVmGenericMsg", message("OM_ParseScreenTrigger", vec![]));
    let null = || {
        Argument::Reference(
            rugp_assets::object::ObjectReader::new(&[0; 3])
                .reference(0)
                .unwrap(),
        )
    };
    add(
        "CVmSync",
        vec![null(), Argument::Number(0), Argument::Number(500)],
    );
    add(
        "CVmGenericMsg",
        message(
            "OM_SetWorldCamera",
            vec![n("ピッチ角", 2), n("総処理時間基", 400)],
        ),
    );
    add("CVmGenericMsg", message("OM_ParseScreenTrigger", vec![]));
    add(
        "CVmSync",
        vec![null(), Argument::Number(0), Argument::Number(1)],
    );
    add(
        "CVmGenericMsg",
        message("OM_SetWorldCamera", vec![n("フェード方式", 16)]),
    ); // not silently accepted
    let mut program = Program::default();
    program.insert(
        10,
        Scenario {
            object_id: 1,
            memory_size: 640,
            instructions,
            null_records: vec![],
            trailing_bytes: 0,
        },
    )?;
    game.vm = Vm::new(Arc::new(program), 1)?;
    let verify_pixels = |game: &Game, top: f64| {
        // Independent inverse mapping: 1024x600 plain image at depth 720,
        // focal length 1440. Do not use Plane or the compositor as an oracle.
        for y in 0..600 {
            for x in 0..1024 {
                let sx = ((f64::from(x) + 0.5 + 512.0) / 2.0).floor() as usize;
                let sy = ((f64::from(y) + 0.5 - top) / 2.0).floor() as usize;
                let si = (sy * 1024 + sx) * 4;
                let di = (y as usize * 1024 + x as usize) * 4;
                let alpha = u32::from(source.rgba[si + 3]);
                for c in 0..3 {
                    let expected = ((u32::from(source.rgba[si + c]) * alpha + 127) / 255) as u8;
                    assert_eq!(game.frame()[di + c], expected, "pixel {x},{y}, channel {c}");
                }
                assert_eq!(game.frame()[di + 3], 255);
            }
        }
    };
    assert_eq!(game.step(0)?, RunState::Waiting);
    assert_eq!(game.objects[&212259].camera.angles_q4, [0, -32, 0]);
    verify_pixels(&game, -300.0 + 5.625);
    let first = game.frame().to_vec();
    assert_eq!(game.step(200)?, RunState::Waiting);
    assert_eq!(game.frame(), first); // mode 0 does not ramp over 400ms
    assert_eq!(game.step(300)?, RunState::Waiting);
    assert_ne!(game.frame(), first);
    verify_pixels(&game, -300.0 - 5.625);
    let before = game.frame().to_vec();
    let error = game
        .step(1)
        .context("expected unsupported transition")
        .unwrap_err();
    assert!(format!("{error:#}").contains("camera style/transition"));
    assert_eq!(game.objects[&212259].camera.angles_q4, [0, 32, 0]);
    assert_eq!(game.frame(), before);
    Ok(())
}
