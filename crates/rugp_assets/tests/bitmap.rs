use rugp_assets::{
    image::decode_r6ti,
    rip::{decode_rip007, decode_rip008},
};

fn bitmap_header(kind: u32) -> Vec<u8> {
    let mut data = vec![0; 3];
    data.extend(69u32.to_le_bytes());
    for value in [1u16, 1, 0, 0, 1, 1] {
        data.extend(value.to_le_bytes());
    }
    data.extend(kind.to_le_bytes());
    data
}

fn r6_fixture(kind: u32, bits: &[u8]) -> Vec<u8> {
    let mut data = bitmap_header(kind);
    data.extend(0x0707070fu32.to_le_bytes());
    data.extend([0; 6]); // compression metadata and null resource reference
    data.extend((bits.len() as u32).to_le_bytes());
    data.extend([0; 8]);
    data.extend(bits);
    data
}

#[test]
fn r6_channel_order_opaque_transparent_and_truncation() {
    // One blue pixel, independently encoded with the LSB predictor grammar.
    for (kind, bits) in [(2, &[8u8][..]), (3, &[0x55, 0x05, 0x02][..])] {
        let data = r6_fixture(kind, bits);
        let image = decode_r6ti(&data, 0).unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.rgba, [0, 0, 2, 255]);
        for length in 0..data.len() {
            assert!(decode_r6ti(&data[..length], 0).is_err());
        }
    }
    assert_eq!(
        decode_r6ti(&r6_fixture(3, &[0]), 0).unwrap().rgba,
        [0, 0, 0, 0]
    );
}

#[test]
fn rip_007_and_008_pixel_layouts() {
    let mut old = bitmap_header(2);
    old.extend([0, 0, 0, 1, 8, 8, 8]);
    old.extend([0; 3]);
    old.extend(1u32.to_le_bytes());
    old.extend([0; 4]);
    old.push(0x10); // a unit increment in the first chroma component
    assert_eq!(decode_rip007(&old, 2, 0).unwrap().rgba, [0, 0, 1, 255]);
    let mut modern = bitmap_header(2);
    modern.extend([3, 8, 8, 8]);
    modern.extend([0; 3]);
    modern.extend(1u32.to_le_bytes());
    modern.extend([0; 4]);
    modern.push(0x10);
    assert_eq!(decode_rip008(&modern, 3, 0).unwrap().rgba, [0, 1, 0, 255]);
    for length in 0..old.len() {
        assert!(decode_rip007(&old[..length], 2, 0).is_err());
    }
    for length in 0..modern.len() {
        assert!(decode_rip008(&modern[..length], 3, 0).is_err());
    }
}
