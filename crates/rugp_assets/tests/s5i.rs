use anyhow::Result;
use rugp_assets::s5i::decode_s5i;
fn fixture() -> Vec<u8> {
    let mut b = vec![0; 3];
    b.extend(0x47u32.to_le_bytes());
    for word in [1u16, 2, 4, 5, 2, 2] {
        b.extend(word.to_le_bytes());
    }
    b.extend(12u32.to_le_bytes());
    b.extend(24u32.to_le_bytes());
    b.extend([255, 0, 0, 128, 4, 3, 2, 1, 90, 91, 92, 93]); // bottom row + padding
    b.extend([0, 0, 255, 255, 0, 255, 0, 0, 94, 95, 96, 97]); // top row + padding
    b
}
#[test]
fn bottom_up_bgra_crop_padding_and_straight_alpha() -> Result<()> {
    let b = fixture();
    let image = decode_s5i(&b, 2)?;
    assert_eq!(
        (image.canvas_width, image.canvas_height, image.x, image.y),
        (4, 5, 1, 2)
    );
    assert_eq!((image.width, image.height), (2, 2));
    assert_eq!(
        image.rgba,
        [255, 0, 0, 255, 0, 255, 0, 0, 0, 0, 255, 128, 2, 3, 4, 1]
    );
    for n in 0..b.len() {
        assert!(decode_s5i(&b[..n], 2).is_err());
    }
    assert!(decode_s5i(&b, 1).is_err());
    for (offset, value) in [(3, 0x43u32), (19, 7), (23, 25)] {
        let mut bad = b.clone();
        bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(decode_s5i(&bad, 2).is_err());
    }
    let mut bad = b.clone();
    bad[7..9].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(decode_s5i(&bad, 2).is_err());
    let mut bad = b.clone();
    bad[11..15].fill(255);
    assert!(decode_s5i(&bad, 2).is_err());
    let mut bad = b;
    bad.push(0);
    assert!(decode_s5i(&bad, 2).is_err());
    Ok(())
}
