use rugp_assets::{
    mime::MimeCurve,
    object::{ClassId, ObjectReader},
};

fn class() -> ClassId {
    ClassId {
        name: "CMimeFlat16LimitOne".into(),
        schema: 1,
    }
}
fn fixture(samples: [i16; 17]) -> Vec<u8> {
    let mut data = vec![0; 3];
    for sample in samples {
        data.extend(sample.to_le_bytes());
    }
    data
}

#[test]
fn sampled_curve_signed_interpolation_input_clamping_and_exact_boundaries() {
    let mut curve = MimeCurve::alpha_ramp(false);
    for (x, y) in [
        (i32::MIN, 0),
        (0, 0),
        (4096, 512),
        (32768, 4096),
        (65536, 8192),
        (i32::MAX, 8192),
    ] {
        assert_eq!(curve.evaluate(x), y);
    }
    assert_eq!(MimeCurve::alpha_ramp(true).evaluate(32768), 4096);
    curve.samples[0] = 1;
    curve.samples[1] = -2;
    assert_eq!(curve.evaluate(1), 0); // 1 + (-3 >> 12), not 1 + (-3 / 4096)
    assert_eq!(curve.evaluate(2048), -1);
    assert_eq!(curve.evaluate(4095), -2);
    curve.samples[16] = i16::MIN;
    assert_eq!(curve.evaluate(65536), -32768); // output is NOT clamped to unity
    curve.samples[15] = i16::MAX;
    assert_eq!(curve.evaluate(65535), -32753);
}

#[test]
fn sampled_curve_schema_extent_and_inline_framing_are_strict() {
    let expected = MimeCurve::alpha_ramp(true);
    let data = fixture(expected.samples);
    assert_eq!(MimeCurve::parse(&data, &class()).unwrap(), expected);
    for end in 0..data.len() {
        assert!(MimeCurve::parse(&data[..end], &class()).is_err());
    }
    let mut extra = data.clone();
    extra.push(0);
    assert!(MimeCurve::parse(&extra, &class()).is_err());
    let mut bad_class = class();
    bad_class.schema = 2;
    assert!(MimeCurve::parse(&data, &bad_class).is_err());
    let mut body = data[3..].to_vec();
    body.extend(0x12345678u32.to_le_bytes());
    let mut r = ObjectReader::new(&body);
    let inline = r.read_inline(&class(), 0).unwrap();
    assert_eq!(
        MimeCurve::from_inline(&inline.data, &class()).unwrap(),
        expected
    );
    assert_eq!(r.input.u32().unwrap(), 0x12345678);
    assert_eq!(r.input.remaining(), 0);
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_sampled_draw_ratio_curve_is_not_a_fade_object() -> anyhow::Result<()> {
    let archive =
        rugp_assets::RioArchive::open(std::env::var_os("RUGP_TEST_GAME").expect("game directory"))?;
    let data = archive.read_at(248008045, 37)?;
    let curve = MimeCurve::parse(&data, &class())?;
    assert_eq!(curve.evaluate(0), 1056);
    assert_eq!(curve.evaluate(32768), 1632);
    assert_eq!(curve.evaluate(65536), 8192);
    let body = MimeCurve::from_inline(&data[3..], &class())?;
    for x in -1..=65537 {
        assert_eq!(curve.evaluate(x), body.evaluate(x));
        let time = x.clamp(0, 65536) as usize;
        let index = time / 4096;
        let raw_sample = |i| {
            i32::from(i16::from_le_bytes(
                data[3 + i * 2..5 + i * 2].try_into().unwrap(),
            ))
        };
        let expected = if index == 16 {
            raw_sample(16)
        } else {
            raw_sample(index)
                + ((raw_sample(index + 1) - raw_sample(index)) * (time % 4096) as i32)
                    .div_euclid(4096)
        };
        assert_eq!(curve.evaluate(x), expected);
    }
    Ok(())
}
