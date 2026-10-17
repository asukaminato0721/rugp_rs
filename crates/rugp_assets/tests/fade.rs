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
    assert!(Fade::from_inline("CFadeUnknown", &data).is_err());
    let mut data = body(false);
    data[38..40].copy_from_slice(&0u16.to_le_bytes());
    assert!(Fade::from_inline("CFadeNormal", &data).is_err());
    Ok(())
}

#[test]
fn specialized_effects_retain_common_regions_and_clock_with_bounded_envelopes() -> Result<()> {
    for (class, prefix, extra) in [
        ("CFadeSdtRatio", 0, 0),
        ("CFadeRSideCarten", 0, 0),
        ("CFadeMozaik", 0, 0),
        ("CFadeMulHorz", 0, 0),
        ("CFadeInvert", 0, 0),
        ("CFadeXsRatio", 0, 6),
        ("CFadeXsMergeBlack", 0, 6),
        ("CFadeStretchAnti", 0, 17),
        ("CFadeStretch", 0, 17),
        ("CFadeOverStretchAnti", 0, 25),
        ("CFadeXsOverStretchAnti", 0, 30),
        ("CFadeQubeStretchAnti", 0, 44),
        ("CFadeXsRasterNoize", 0, 23),
        ("CFadeCarten", 6, 0),
        ("CFadeSdtCircleRaster", 15, 0),
        ("CFadeXsSqrRaster_HRasterV_VRasterH", 0, 17),
        ("CFadeXsHRasterHOffset", 0, 17),
        ("CFadeXsSrcRotate", 0, 24),
        ("CFadeXsCircleRaster", 0, 21),
    ] {
        let mut data = vec![0; prefix];
        data.extend(body(false));
        data.resize(data.len() + extra, 0);
        let fade = Fade::from_inline(class, &data)?;
        assert_eq!(fade.rectangles[0], [0, 0, 1024, 600], "{class}");
        assert_eq!(fade.curve.alpha(125, 1000), 64, "{class}");
        assert_eq!(fade.curve.alpha(1000, 1000), 256, "{class}");
        for end in 0..data.len() {
            assert!(
                Fade::from_inline(class, &data[..end]).is_err(),
                "{class}, end {end}"
            );
        }
    }
    Ok(())
}

#[test]
fn over_stretch_preserves_signed_geometry_and_q13_envelopes() -> Result<()> {
    let mut data = body(false);
    for value in [0i16, 0, 800, 600, -400, 0, 0, 300] {
        data.extend(value.to_le_bytes());
    }
    data.push(1);
    for value in [22i16, 32, 2048, 0] {
        data.extend(value.to_le_bytes());
    }
    let fade = Fade::from_inline("CFadeOverStretchAnti", &data)?;
    let stretch = fade.stretch.unwrap();
    assert_eq!(stretch.destination_end, [0, 0, 800, 600]);
    assert_eq!(stretch.source_end, [-400, 0, 0, 300]);
    assert_eq!(stretch.opacity, [32, 2048]);
    assert_eq!(stretch.frames, 22);
    // This curve has a fractional Q8 opacity at x=65. Geometry must keep it.
    assert_eq!(fade.curve.value(65, 256), 4117);
    assert_eq!(fade.curve.alpha(65, 256), 128);
    Ok(())
}

#[test]
fn qube_stretch_reads_its_weights_pivot_and_tilt_after_the_stretch_base() -> Result<()> {
    // Retail scene 60 at 0x1034: the outgoing face of the 1024-wide turn.
    let mut data = body(false);
    data.extend([
        0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x58, 0x02, 0x7c, 0x00, 0x00, 0x00, 0x40, 0x01, 0x58,
        0x02, 0x01, 0x00, 0x00, 0x40, 0x00, 0x04, 0x00, 0xc0, 0x00, 0xfc, 0x00, 0x00, 0x02, 0x2c,
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0xfe, 0x00, 0x00, 0x00, 0x00, 0x01,
    ]);
    let fade = Fade::from_inline("CFadeQubeStretchAnti", &data)?;
    let stretch = fade.stretch.unwrap();
    assert_eq!(stretch.destination_end, [0, 0, 1024, 600]);
    assert_eq!(stretch.source_end, [124, 0, 320, 600]);
    assert_eq!(stretch.options, 1);
    let qube = fade.qube.unwrap();
    assert_eq!(qube.source_weight, [64, 4]);
    assert_eq!(qube.destination_weight, [192, 252]);
    assert_eq!(qube.center, [512, 300]);
    assert_eq!(qube.angles, [[0, 0, 0], [-400, 0, 0]]);
    assert_eq!(qube.pivot_from_rect, 1);
    Ok(())
}
