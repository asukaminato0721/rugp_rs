use rugp_assets::background::{Background, SceneBackground};

fn encoded_class(name: &str) -> Vec<u8> {
    let mut bits = vec![true];
    for c in name.bytes() {
        bits.extend([true, false, false, false, false, false]);
        bits.extend((0..8).map(|bit| c >> bit & 1 != 0));
    }
    bits.extend([true; 7]);
    let mut bytes = vec![0; bits.len().div_ceil(8)];
    for (i, bit) in bits.into_iter().enumerate() {
        bytes[i / 8] |= u8::from(bit) << (i % 8);
    }
    bytes
}
fn header() -> Vec<u8> {
    let mut data = vec![0; 3];
    for word in [3u16, 0, 1, 1] {
        data.extend(word.to_le_bytes());
    }
    data.extend([0; 20 + 4 + 2 + 1]); // placement, flags, legacy count, properties
    data.push(1);
    data.extend(0x123456u32.to_le_bytes());
    data.extend([0; 12]);
    data.extend(0.6f32.to_le_bytes());
    data
}
fn image(data: &mut Vec<u8>) {
    data.push(0); // first RIO reference's ancestor count
    data.extend(0x8005u16.to_le_bytes()); // enclosing CBg2d's registered Cr6Ti
    data.extend(0xc040u16.to_le_bytes());
    data.extend(69u32.to_le_bytes());
    for word in [1u16, 1, 0, 0, 1, 1] {
        data.extend(word.to_le_bytes());
    }
    data.extend(2u32.to_le_bytes());
    data.extend(0x0707070fu32.to_le_bytes());
    data.extend([0; 5]); // metadata and null image properties
    data.extend(1u32.to_le_bytes());
    data.extend([0; 8]);
    data.extend([8, 0, 0, 0]); // blue pixel, alignment and lookahead guard
}
fn derived(data: &mut Vec<u8>, first: bool) {
    if first {
        data.extend(0xffffu16.to_le_bytes());
        data.extend(1u16.to_le_bytes());
        let name = encoded_class("CBg2dLayerRip007Image");
        data.push(name.len() as u8);
        data.extend(name);
    } else {
        data.extend(0x8006u16.to_le_bytes()); // cached raw layer class
    }
    data.extend([0; 4 + 24 + 4]);
    if first {
        image(data);
    } else {
        data.extend(8u16.to_le_bytes()); // image reserved after raw layer slot 7
    }
}
fn base(data: &mut Vec<u8>, value: u8) {
    data.extend([1, b'a' + value]); // synthetic CString
    data.extend(u16::from(value).to_le_bytes());
    data.extend([2, 3]);
    for n in [-1i32, 2, 3] {
        data.extend(n.to_le_bytes());
    }
    data.extend(u32::from(value).to_le_bytes());
}
fn fixture() -> Vec<u8> {
    let mut data = header();
    derived(&mut data, true);
    derived(&mut data, false);
    data.extend([0; 2]); // null next pointer
    base(&mut data, 1); // tail's base fields precede head's base fields
    base(&mut data, 0);
    data.extend(2u32.to_le_bytes());
    data
}

#[test]
fn linked_layers_share_class_and_image_cache_and_keep_head_order() {
    let data = fixture();
    let bg = SceneBackground::parse(&data, 6, 0).unwrap();
    assert_eq!(bg.color, 0x123456);
    assert_eq!(bg.scale, 0.6);
    assert_eq!(bg.layers.len(), 2);
    for (i, layer) in bg.layers.iter().enumerate() {
        assert_eq!(layer.id, i as u16);
        assert_eq!(layer.flags, i as u32);
        assert_eq!(layer.attributes, [2, 3]);
        assert_eq!(layer.parameters, [-1, 2, 3]);
        assert_eq!(
            layer
                .image
                .inline
                .as_ref()
                .unwrap()
                .decode_bitmap()
                .unwrap()
                .rgba,
            [0, 0, 2, 255]
        );
    }
    assert_eq!(bg.layers[1].image.cache_index, Some(8));
    for end in 0..data.len() {
        assert!(
            SceneBackground::parse(&data[..end], 6, 0).is_err(),
            "accepted truncation {end}"
        );
    }
}

#[test]
fn extension_validates_count_eof_null_and_finite_scale() {
    let mut empty = header();
    empty.extend([0; 6]);
    assert!(
        SceneBackground::parse(&empty, 6, 0)
            .unwrap()
            .layers
            .is_empty()
    );
    let mut invalid_pointer = empty.clone();
    let len = invalid_pointer.len();
    invalid_pointer[len - 6] = 1;
    assert!(SceneBackground::parse(&invalid_pointer, 6, 0).is_err());
    let mut count = fixture();
    let len = count.len();
    count[len - 4] = 3;
    assert!(SceneBackground::parse(&count, 6, 0).is_err());
    empty.push(0);
    assert!(SceneBackground::parse(&empty, 6, 0).is_err());
    let mut scale = header();
    let len = scale.len();
    scale[len - 4..].copy_from_slice(&f32::NAN.to_le_bytes());
    scale.extend([0; 6]);
    assert!(SceneBackground::parse(&scale, 6, 0).is_err());
    // The legacy inspection API remains deliberately permissive about tail.
    assert!(Background::parse(&scale, 6, 0).is_ok());
}
