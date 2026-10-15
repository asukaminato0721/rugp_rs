use rugp_assets::{
    motion::{MotionCurve, MotionResource, TimeCurve},
    object::{ClassId, ObjectReader},
};

fn class() -> ClassId {
    ClassId {
        name: "CMoveAcsOngen".into(),
        schema: 3,
    }
}
fn words(bytes: &mut Vec<u8>, values: &[i32]) {
    for value in values {
        bytes.extend(value.to_le_bytes());
    }
}
fn declared_class(bytes: &mut Vec<u8>, name: &str) {
    bytes.extend(0xffffu16.to_le_bytes());
    bytes.extend(0u16.to_le_bytes());
    bytes.extend((name.len() as u16).to_le_bytes());
    bytes.extend(name.bytes());
}
fn tail(bytes: &mut Vec<u8>) {
    bytes.push(0x70);
    words(bytes, &[-1, 4000]);
    bytes.extend(1000f32.to_le_bytes());
    bytes.push(1);
    bytes.extend(3u16.to_le_bytes());
}
fn body() -> Vec<u8> {
    let mut bytes = Vec::new();
    words(&mut bytes, &[10, -20, 30]);
    bytes.extend([2, 0]);
    words(&mut bytes, &[100, -40, 60]); // compact endpoint X/Z
    declared_class(&mut bytes, "CMN_Time_2G"); // class cache 2, instance cache 3
    words(&mut bytes, &[1]);
    bytes.extend([3, 3]); // 3D Bezier with timing object
    words(&mut bytes, &[200, 70, -80, 90]);
    bytes.extend(2u16.to_le_bytes());
    words(&mut bytes, &[11, 22, 33, -44, -55, -66]);
    bytes.extend(3u16.to_le_bytes()); // cached timing instance
    bytes.extend([3, 0]); // compact Bezier, no timing object
    words(&mut bytes, &[300, 100, -110]);
    bytes.extend(1u16.to_le_bytes());
    words(&mut bytes, &[-12, 13]);
    tail(&mut bytes);
    bytes
}

#[test]
fn typed_segments_compact_xz_controls_and_shared_time_instances() {
    let mut bytes = body();
    bytes.extend(0x12345678u32.to_le_bytes());
    let mut reader = ObjectReader::new(&bytes);
    reader.scrambled = true; // timing classes deliberately use plain MFC names
    let inline = reader.read_inline(&class(), 0).unwrap();
    assert!(reader.scrambled);
    let curve = MotionCurve::from_inline(&inline, &class()).unwrap();
    assert_eq!(curve.start, [10, -20, 30]);
    assert_eq!(curve.segments.len(), 3);
    assert_eq!(curve.segments[0].end, [-40, 0, 60]);
    assert_eq!(curve.segments[0].time_curve, Some(TimeCurve::Quadratic(1)));
    assert_eq!(curve.segments[1].end, [70, -80, 90]);
    assert_eq!(curve.segments[1].controls, [[11, 22, 33], [-44, -55, -66]]);
    assert_eq!(curve.segments[1].time_curve, curve.segments[0].time_curve);
    assert_eq!(curve.segments[2].end, [100, 0, -110]);
    assert_eq!(curve.segments[2].controls, [[-12, 0, 13]]);
    assert_eq!(curve.segments[2].time_curve, None);
    assert_eq!(
        (
            curve.timing_mode,
            curve.duration_ms,
            curve.rate,
            curve.dimensions,
            curve.flags
        ),
        (-1, 4000, 1000.0, 1, 3)
    );
    assert_eq!(reader.input.u32().unwrap(), 0x12345678);
    assert_eq!(reader.input.remaining(), 0);
    // The enclosing class cache still contains the timing class, in order.
    assert_eq!(reader.cached_class(2).unwrap().name, "CMN_Time_2G");
}

#[test]
fn standalone_extent_truncation_class_and_finite_values_are_strict() {
    let mut bytes = vec![0; 3];
    bytes.extend(body());
    MotionCurve::parse(&bytes, &class()).unwrap();
    for length in 0..bytes.len() {
        assert!(
            MotionCurve::parse(&bytes[..length], &class()).is_err(),
            "accepted {length}"
        );
    }
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(MotionCurve::parse(&extra, &class()).is_err());
    let resource = MotionResource::parse(&extra, &class()).unwrap();
    assert_eq!(resource.trailing_data, [0]);
    assert_eq!(
        resource.curve,
        MotionCurve::parse(&bytes, &class()).unwrap()
    );
    let mut wrong_class = class();
    wrong_class.schema = 2;
    assert!(MotionCurve::parse(&bytes, &wrong_class).is_err());
    wrong_class = class();
    wrong_class.name = "COther".into();
    assert!(MotionCurve::parse(&bytes, &wrong_class).is_err());
    let rate_offset = bytes.len() - 7;
    bytes[rate_offset..rate_offset + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(MotionCurve::parse(&bytes, &class()).is_err());
}

#[test]
fn invalid_segments_time_references_and_restoring_scramble_on_error() {
    let mut bytes = body();
    bytes[12] = 4;
    assert!(ObjectReader::new(&bytes).read_inline(&class(), 0).is_err());
    let mut bytes = vec![0; 12];
    bytes.extend([2, 0]);
    words(&mut bytes, &[100, 20, 30]);
    bytes.extend(100u16.to_le_bytes());
    tail(&mut bytes);
    assert!(ObjectReader::new(&bytes).read_inline(&class(), 0).is_err());
    // Class parsing may fail before returning a value; the outer archive's
    // scrambled-name state must still be restored.
    let mut bytes = vec![0; 12];
    bytes.extend([2, 0]);
    words(&mut bytes, &[100, 20, 30]);
    bytes.extend([0xff; 2]);
    let mut reader = ObjectReader::new(&bytes);
    reader.scrambled = true;
    assert!(reader.read_inline(&class(), 0).is_err());
    assert!(reader.scrambled);
}

#[test]
fn time_bezier_knots_and_cached_class_new_instance() {
    let mut bytes = vec![0; 12];
    for (index, tag) in [(0, 0xffffu16), (1, 0x8002)] {
        bytes.extend([2, 1]);
        words(&mut bytes, &[100, 1, 2, 3]);
        if index == 0 {
            declared_class(&mut bytes, "CMN_Time_Bezier");
        } else {
            bytes.extend(tag.to_le_bytes());
        }
        bytes.extend(1u16.to_le_bytes());
        for value in [0.0f32, 1.0, 0.25, -0.5, 0.75, 1.5] {
            bytes.extend(value.to_le_bytes());
        }
    }
    tail(&mut bytes);
    let inline = ObjectReader::new(&bytes).read_inline(&class(), 0).unwrap();
    let curve = MotionCurve::from_inline(&inline, &class()).unwrap();
    let expected = Some(TimeCurve::Bezier(vec![[0.0, 1.0, 0.25, -0.5, 0.75, 1.5]]));
    assert_eq!(curve.segments[0].time_curve, expected);
    assert_eq!(curve.segments[1].time_curve, expected);
    // Non-time-object cache indexes must not silently become linear timing.
    let mut invalid = body();
    let cached_tag = 12 + 2 + 12 + 6 + "CMN_Time_2G".len() + 4 + 2 + 16 + 2 + 24;
    invalid[cached_tag..cached_tag + 2].copy_from_slice(&2u16.to_le_bytes());
    assert!(
        ObjectReader::new(&invalid)
            .read_inline(&class(), 0)
            .is_err()
    );
}
