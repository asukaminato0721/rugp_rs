use anyhow::Result;
use rugp_assets::crsi::{
    Crsi, FrameIndex,
    codec::{decode_frame, decode_keyframe},
};

fn fixture() -> Vec<u8> {
    let mut data = vec![0; 11];
    data.extend(4u16.to_le_bytes());
    data.extend(2u16.to_le_bytes());
    data.extend(0u32.to_le_bytes());
    data.extend(3000u16.to_le_bytes());
    data.extend(2u16.to_le_bytes());
    for (kind, offset, codec, size) in [(1u8, 0u32, 0x575u16, 8u32), (0, 8, 0x175, 6)] {
        data.push(kind);
        data.extend(offset.to_le_bytes());
        data.extend(0u16.to_le_bytes());
        data.extend(codec.to_le_bytes());
        if codec & 0x1c00 == 0x400 {
            data.extend(8u16.to_le_bytes());
        }
        data.extend(size.to_le_bytes());
    }
    data
}

#[derive(Default)]
struct Encoded(Vec<bool>);
impl Encoded {
    fn bits(&mut self, bits: &[bool]) {
        self.0.extend_from_slice(bits);
    }
    fn signed(&mut self, value: i32) {
        self.bits(&[value != 0]);
        if value == 0 {
            return;
        }
        self.bits(&[value < 0]);
        let value = value.unsigned_abs();
        let high = 31 - value.leading_zeros();
        for i in (0..high).rev() {
            self.bits(&[true, value >> i & 1 != 0]);
        }
        if high < 6 {
            self.bits(&[false]);
        }
    }
    fn bytes(self) -> Vec<u8> {
        let mut bytes = vec![0; self.0.len().div_ceil(8)];
        for (i, bit) in self.0.into_iter().enumerate() {
            bytes[i / 8] |= u8::from(bit) << (i % 8);
        }
        bytes
    }
}
fn frame(kind: u8, data: &[u8]) -> FrameIndex {
    FrameIndex {
        kind,
        offset: 0,
        unknown: 0,
        codec: 0x575,
        auxiliary: Some(8),
        size: data.len() as u32,
    }
}

#[test]
fn keyframe_spatial_interpolation_and_repeated_top_right_pixel() -> Result<()> {
    let mut stream = Encoded::default();
    // One literal, average predictor without deltas, three adaptive luma codes,
    // then one repeated block. The repeat carries the top-right RGB pixel,
    // while the spatial YUV state carries the bottom-right predictor.
    stream.bits(&[
        false, false, false, false, false, false, true, true, true, false,
    ]);
    let data = stream.bytes();
    let image = decode_keyframe(4, 2, &frame(1, &data), &data)?;
    let luma: Vec<_> = image.rgba.as_chunks::<4>().0.iter().map(|p| p[0]).collect();
    assert_eq!(luma, [16, 32, 32, 32, 96, 64, 32, 32]);
    assert!(
        image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[0] == p[1] && p[1] == p[2] && p[3] == 255)
    );
    assert!(decode_keyframe(3, 2, &frame(1, &data), &data).is_err());
    let mut truncated = frame(1, &data);
    truncated.size = 1;
    assert!(decode_keyframe(4, 2, &truncated, &data[..1]).is_err());
    Ok(())
}

#[test]
fn temporal_modes_preserve_prior_pixels_or_replicate_top_right() -> Result<()> {
    let prior = rugp_assets::image::Image {
        canvas_width: 2,
        canvas_height: 2,
        width: 2,
        height: 2,
        x: 0,
        y: 0,
        display_scale: 1.0,
        rgba: vec![
            10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255,
        ],
    };
    for replicate in [false, true] {
        let mut stream = Encoded::default();
        stream.bits(&[false, false]); // one literal, averaged predictor
        stream.signed(32); // Y: 64 + 2 * 32 = neutral residual 128
        stream.signed(0);
        stream.signed(0);
        stream.bits(&[replicate]);
        if !replicate {
            stream.bits(&[true]);
        }
        let data = stream.bytes();
        let index = frame(0, &data);
        let image = decode_frame(2, 2, &index, &data, Some(&prior))?;
        if replicate {
            assert!(
                image
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| p == &prior.rgba[4..8])
            );
        } else {
            assert_eq!(image.rgba, prior.rgba);
        }
        assert!(decode_frame(2, 2, &index, &data, None).is_err());
        assert!(decode_keyframe(2, 2, &index, &data).is_err());
    }
    Ok(())
}

#[test]
fn temporal_signed_codes_saturate_both_directions() -> Result<()> {
    let prior = rugp_assets::image::Image {
        canvas_width: 2,
        canvas_height: 2,
        width: 2,
        height: 2,
        x: 0,
        y: 0,
        display_scale: 1.0,
        rgba: [128, 128, 128, 255].repeat(4),
    };
    for (delta, expected) in [(0, 0), (32, 128), (127, 255)] {
        let mut stream = Encoded::default();
        stream.bits(&[false, false]);
        stream.signed(delta);
        stream.signed(0);
        stream.signed(0);
        stream.bits(&[true]);
        let data = stream.bytes();
        let image = decode_frame(2, 2, &frame(0, &data), &data, Some(&prior))?;
        assert!(
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [expected, expected, expected, 255])
        );
    }
    Ok(())
}
#[test]
fn crsi_index_is_bounded_and_does_not_require_payload() -> Result<()> {
    let data = fixture();
    let extent = data.len() as u32 + 14;
    let stream = Crsi::parse_index(&data, extent)?;
    assert_eq!((stream.width, stream.height, stream.rate), (4, 2, 3000));
    assert_eq!(stream.frames.len(), 2);
    assert_eq!(stream.payload_start as usize, data.len());
    assert_eq!(stream.payload_size, 14);
    assert_eq!(stream.frames[0].channel_depths(), [6, 7, 6]);
    assert_eq!(stream.frames[0].codec_id(), 1);
    assert_eq!(stream.frames[0].auxiliary, Some(8));
    assert!(stream.frames[0].is_keyframe());
    assert!(!stream.frames[1].is_keyframe());
    for len in 0..data.len() {
        assert!(Crsi::parse_index(&data[..len], extent).is_err());
    }
    assert!(Crsi::parse_index(&data, extent - 1).is_err());
    let mut data = data;
    data[39..43].copy_from_slice(&0u32.to_le_bytes()); // second frame overlaps first
    assert!(Crsi::parse_index(&data, extent).is_err());
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_movie_indexes_and_temporal_prefixes() -> Result<()> {
    use anyhow::Context;
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let archive = rugp_assets::RioArchive::open(root)?;
    let sequence = rugp_assets::crsi::VideoSequence::parse(
        &archive.read_at(0x3c423, 45)?,
        archive.address_shift,
    )?;
    assert_eq!((sequence.x, sequence.y, sequence.rate), (0, 0, 0));
    for (offset, size, count, keyframes) in [
        (0xd0ba5f, 234130539, 3511, 1487),
        (0x7c67b686, 202725217, 3875, 1665),
    ] {
        let stream = Crsi::read(&archive, offset, size)?;
        assert_eq!(
            (stream.width, stream.height, stream.rate),
            (1024, 600, 3000)
        );
        assert_eq!(stream.frames.len(), count);
        assert_eq!(
            stream.frames.iter().filter(|f| f.is_keyframe()).count(),
            keyframes
        );
        assert_eq!(stream.trailing_data.len(), 17);
        let mut previous = None;
        for i in 0..16 {
            let data = stream.frame_data(&archive, offset, i)?;
            let image = decode_frame(
                stream.width,
                stream.height,
                &stream.frames[i],
                &data,
                previous.as_ref(),
            )?;
            assert!(image.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
            previous = Some(image);
        }
    }
    Ok(())
}

#[test]
fn video_sequence_defaults_and_truncations() -> Result<()> {
    let mut data = vec![0; 45];
    data[3] = 2; // object-reference cache padding count
    data[32..36].copy_from_slice(&(-20i32).to_le_bytes());
    data[36..40].copy_from_slice(&25i32.to_le_bytes());
    data[40..44].copy_from_slice(&3000u32.to_le_bytes());
    let sequence = rugp_assets::crsi::VideoSequence::parse(&data, 0)?;
    assert_eq!((sequence.x, sequence.y, sequence.rate), (-20, 25, 3000));
    assert!(sequence.source.class.is_none());
    assert!(sequence.target.class.is_none());
    for len in 0..data.len() {
        assert!(rugp_assets::crsi::VideoSequence::parse(&data[..len], 0).is_err());
    }
    data.push(0);
    assert!(rugp_assets::crsi::VideoSequence::parse(&data, 0).is_err());
    Ok(())
}
