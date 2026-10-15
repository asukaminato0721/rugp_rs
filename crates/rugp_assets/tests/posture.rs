use rugp_assets::{
    object::{ClassId, ObjectReader},
    posture::Posture,
};

fn class() -> ClassId {
    ClassId {
        name: "CPostureMoment".into(),
        schema: 4,
    }
}

fn body(version: u8, ancestry_header: bool) -> Vec<u8> {
    let mut bytes = vec![5, version, 1];
    if ancestry_header {
        bytes.push(0);
    }
    bytes.extend([0; 10]);
    for value in [101i32, -202, 303, -1, -404, 505, -606, 707, -808, 909] {
        bytes.extend(value.to_le_bytes());
    }
    if version >= 2 {
        for value in [-1001i32, 1102, -1203] {
            bytes.extend(value.to_le_bytes());
        }
    }
    bytes.extend(0x1234u16.to_le_bytes());
    bytes.extend(0xabcd_u16.to_le_bytes());
    if version >= 2 {
        bytes.extend(0x5678u16.to_le_bytes());
    }
    for value in [0.5f32, -2.0, 0.0] {
        bytes.extend(value.to_le_bytes());
    }
    bytes
}

#[test]
fn all_versions_preserve_typed_fields_and_following_data() {
    for version in 0..=2 {
        let mut bytes = body(version, true);
        let length = bytes.len();
        bytes.extend(0x12345678u32.to_le_bytes());
        let mut reader = ObjectReader::new(&bytes);
        let inline = reader.read_inline(&class(), 0).unwrap();
        assert_eq!(inline.data.len(), length);
        let posture = Posture::from_inline(&inline, &class()).unwrap();
        let fields = &posture.fields;
        assert_eq!(
            (fields.state, fields.version, fields.options),
            (5, version, 1)
        );
        assert_eq!(fields.base_point, [101, -202, 303]);
        assert_eq!(fields.elapsed_ms, -1);
        assert_eq!(fields.angles, [-404, 505, -606]);
        assert_eq!(fields.position, [707, -808, 909]);
        assert_eq!(
            fields.other,
            if version >= 2 {
                [-1001, 1102, -1203]
            } else {
                [0; 3]
            }
        );
        assert_eq!(fields.position_mode, 0x1234);
        assert_eq!(fields.angle_mode, 0xabcd);
        assert_eq!(fields.other_mode, if version >= 2 { 0x5678 } else { 0 });
        assert_eq!(fields.scales, [0.5, -2.0, 0.0]);
        assert!(
            posture
                .references
                .iter()
                .all(|reference| reference.class.is_none())
        );
        assert_eq!(reader.input.u32().unwrap(), 0x12345678);
        assert_eq!(reader.input.remaining(), 0);
        for length in 0..length {
            assert!(
                ObjectReader::new(&bytes[..length])
                    .read_inline(&class(), 0)
                    .is_err(),
                "accepted version {version} truncated at {length}"
            );
        }
    }
}

#[test]
fn body_shares_enclosing_reference_framing() {
    let mut bytes = vec![0, 0, 0]; // first reference: ancestry count, null tag
    bytes.extend(body(2, false));
    bytes.extend([0, 0]); // following reference: no second ancestry count
    let mut reader = ObjectReader::new(&bytes);
    assert!(reader.reference(0).unwrap().class.is_none());
    let inline = reader.read_inline(&class(), 0).unwrap();
    assert_eq!(
        inline.posture.as_ref().unwrap().base_point,
        [101, -202, 303]
    );
    assert!(reader.reference(0).unwrap().class.is_none());
    assert_eq!(reader.input.remaining(), 0);
}

#[test]
fn malformed_metadata_is_rejected() {
    let bytes = body(3, true);
    assert!(ObjectReader::new(&bytes).read_inline(&class(), 0).is_err());
    for bits in [
        f32::NAN.to_bits(),
        f32::INFINITY.to_bits(),
        f32::NEG_INFINITY.to_bits(),
    ] {
        let mut bytes = body(2, true);
        let offset = bytes.len() - 12;
        bytes[offset..offset + 4].copy_from_slice(&bits.to_le_bytes());
        assert!(ObjectReader::new(&bytes).read_inline(&class(), 0).is_err());
    }
    let bytes = body(2, true);
    let mut inline = ObjectReader::new(&bytes).read_inline(&class(), 0).unwrap();
    let mut wrong_class = class();
    wrong_class.schema = 3;
    assert!(Posture::from_inline(&inline, &wrong_class).is_err());
    wrong_class = class();
    wrong_class.name = "COther".into();
    assert!(Posture::from_inline(&inline, &wrong_class).is_err());
    inline.references.pop();
    assert!(Posture::from_inline(&inline, &class()).is_err());
    inline.posture = None;
    assert!(Posture::from_inline(&inline, &class()).is_err());
}
