use anyhow::Result;
use rugp_assets::fade::Fade;

fn body(reverse: bool) -> Vec<u8> {
    let mut data = Vec::new();
    for word in [3u16, 0x11, 1, 2, 2, 0, 0, 1024, 600, 0, 0, 0, 0, 0, 0, 0, 0] {
        data.extend(word.to_le_bytes());
    }
    for (x, y) in [(0u16, 0u16), (64, 4096), (256, 8192)] {
        data.extend(x.to_le_bytes());
        data.extend((if reverse { 8192 - y } else { y }).to_le_bytes());
    }
    data
}
#[test]
fn native_curve_interpolation_and_color() -> Result<()> {
    let fade = Fade::from_inline("CFadeNormal", &body(false))?;
    assert_eq!(fade.rectangles[0], [0, 0, 1024, 600]);
    assert_eq!(fade.mode, 0x11);
    assert_eq!(fade.curve.alpha(0, 1000), 0);
    assert_eq!(fade.curve.alpha(125, 1000), 64);
    assert_eq!(fade.curve.alpha(250, 1000), 128);
    assert_eq!(fade.curve.alpha(625, 1000), 192);
    assert_eq!(fade.curve.alpha(u32::MAX, 1000), 256);
    assert_eq!(fade.curve.alpha(0, 0), 256);
    let shock = Fade::from_inline("CFadeShock", &body(false))?;
    assert_eq!(shock.color, None);
    assert_eq!(shock.curve.alpha(1000, 1000), 256);
    let mut data = body(true);
    data.extend(0x00123456u32.to_le_bytes());
    let fade = Fade::from_inline("CFadeMergeColor", &data)?;
    assert_eq!(fade.color, Some([0x56, 0x34, 0x12]));
    assert_eq!(fade.curve.alpha(0, 1000), 256);
    assert_eq!(fade.curve.alpha(1000, 1000), 0);
    for len in 0..data.len() {
        assert!(Fade::from_inline("CFadeMergeColor", &data[..len]).is_err());
    }
    assert!(Fade::from_inline("CFadeMozaik", &data).is_err());
    let mut data = body(false);
    data[38..40].copy_from_slice(&0u16.to_le_bytes());
    assert!(Fade::from_inline("CFadeNormal", &data).is_err());
    Ok(())
}
