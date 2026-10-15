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
fn r6_preserves_world_display_scale_without_resampling() {
    let mut data = r6_fixture(2, &[8]);
    assert_eq!(decode_r6ti(&data, 0).unwrap().display_scale, 1.0);
    data[37..41].copy_from_slice(&0.5f32.to_le_bytes());
    data[41..45].copy_from_slice(&0x12345678u32.to_le_bytes());
    let image = decode_r6ti(&data, 0).unwrap();
    assert_eq!(image.display_scale, 0.5);
    assert_eq!((image.width, image.height), (1, 1));
    assert_eq!(image.rgba, [0, 0, 2, 255]);
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

#[derive(Default)]
struct Codes(Vec<bool>);
impl Codes {
    fn bit(&mut self, value: bool) {
        self.0.push(value);
    }
    fn signed(&mut self, value: i32) {
        self.bit(value != 0);
        if value == 0 {
            return;
        }
        self.bit(value < 0);
        let value = value.unsigned_abs();
        for bit in (0..31 - value.leading_zeros()).rev() {
            self.bit(true);
            self.bit(value >> bit & 1 != 0);
        }
        self.bit(false);
    }
    fn bytes(&self, lsb: bool) -> Vec<u8> {
        let mut data = vec![0; self.0.len().div_ceil(8)];
        for (i, bit) in self.0.iter().enumerate() {
            data[i / 8] |= (*bit as u8) << if lsb { i % 8 } else { 7 - i % 8 };
        }
        data
    }
}

#[test]
fn crmti_profiles_channel_order_copy_and_bounds() {
    use rugp_assets::crmt::MipLevel;
    let mut codes = Codes::default();
    codes.signed(1);
    codes.signed(0); // first row, one pixel
    codes.signed(0);
    codes.signed(0); // next row, unchanged span/start
    for copy in [false, true] {
        codes.signed(32); // opaque
        codes.bit(false); // alpha count: unsigned zero = one pixel
        codes.bit(false); // literal count: one pixel
        codes.bit(copy);
        if !copy {
            codes.signed(0); // common green delta
            codes.signed(1); // native B
            codes.signed(0); // native R
        }
    }
    for depth in [5u32, 6, 7] {
        for flags in [7, 15] {
            let mut level = MipLevel {
                width: 1,
                height: 2,
                precision: flags | (depth * 0x01010100),
                active_rows: 2,
                data: codes.bytes(flags == 15),
            };
            let blue = 1u8 << (8 - depth);
            assert_eq!(
                level.decode().unwrap().rgba,
                [0, 0, blue, 255, 0, 0, blue, 255]
            );
            let original = level.data.clone();
            for length in 0..original.len() {
                level.data = original[..length].to_vec();
                assert!(level.decode().is_err());
            }
            level.data = original;
            level.data.push(1);
            assert!(level.decode().is_err()); // nonzero padding
            level.active_rows = 3;
            assert!(level.decode().is_err());
        }
    }
}

#[test]
fn background_inline_bitmap_preserves_cache_and_tail() {
    use rugp_assets::background::Background;
    let mut data = vec![0; 3];
    data.extend(3u16.to_le_bytes()); // pre-registered Cr6Ti schema
    data.extend(0u16.to_le_bytes()); // embedded version
    data.extend(1u16.to_le_bytes());
    data.extend(1u16.to_le_bytes());
    data.extend([0; 20]);
    data.extend(0u32.to_le_bytes());
    data.extend(1u16.to_le_bytes());
    data.extend([0; 12]);
    data.push(0); // ancestor count on first reference
    data.extend(0x8005u16.to_le_bytes());
    data.extend(0xc040u16.to_le_bytes());
    data.extend(&bitmap_header(2)[3..]);
    data.extend(0x0707070fu32.to_le_bytes());
    data.extend([0; 3]);
    data.extend(0u16.to_le_bytes()); // metadata + cached null; no second ancestor count
    data.extend(1u32.to_le_bytes());
    data.extend([0; 8]);
    data.push(8);
    data.push(0); // no background properties
    for length in 0..data.len() {
        assert!(Background::parse(&data[..length], 6, 0).is_err());
    }
    data.extend([0xaa, 0xbb]);
    let bg = Background::parse(&data, 6, 0).unwrap();
    assert_eq!((bg.width, bg.height), (1, 1));
    assert_eq!(bg.planes.len(), 1);
    let inline = bg.planes[0].image.inline.as_ref().unwrap();
    assert_eq!(inline.decode_bitmap().unwrap().rgba, [0, 0, 2, 255]);
    assert_eq!(bg.trailing_data, [0xaa, 0xbb]);
}
