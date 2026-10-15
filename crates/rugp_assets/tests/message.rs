use anyhow::{Context, Result};
use rugp_assets::message::{FrameImage, MessageBox};

fn message_fixture(children: bool) -> Vec<u8> {
    let mut b = vec![0; 3];
    b.extend([0; 4]);
    b.extend(3u32.to_le_bytes());
    b.push(1);
    for rect in [
        [-32i16, -8, 64, 16],
        [0; 4],
        [80, 402, 864, 190],
        [0, 0, 864, 190],
        [0; 4],
    ] {
        for word in rect {
            b.extend(word.to_le_bytes());
        }
    }
    b.push(4);
    b.extend(b"test");
    b.extend(u16::from(children).to_le_bytes());
    if children {
        b.extend([0; 3]);
    }
    b.extend([0; 8]); // reserved words and surface flags
    for word in [2i32, -1, -1] {
        b.extend(word.to_le_bytes());
    }
    for word in [0i16, 0, 0, 24, 4, 0] {
        b.extend(word.to_le_bytes());
    }
    b.extend(if children { vec![0; 2] } else { vec![0; 3] });
    b.extend(0x122u32.to_le_bytes());
    b.extend([0; 3]);
    b.extend([0; 2]);
    b.push(1);
    b.extend(0x311u32.to_le_bytes());
    b
}

#[test]
fn message_window_resource_is_not_a_ui_snapshot() -> Result<()> {
    for children in [false, true] {
        let b = message_fixture(children);
        let message = MessageBox::parse(&b, 15, 0)?;
        assert_eq!(message.rectangles[2], [80, 402, 864, 190]);
        assert_eq!(message.children.len(), usize::from(children));
        assert_eq!(message.surface_parameters, [2, -1, -1]);
        assert_eq!(message.initial_text, "test");
        assert_eq!(message.content_rect, [0, 0, 0, 24]);
        assert_eq!(message.spacing, [4, 0]);
        assert_eq!(message.layout, 0x311);
        assert!(message.frame.class.is_none());
        for n in 0..b.len() {
            assert!(MessageBox::parse(&b[..n], 15, 0).is_err(), "truncation {n}");
        }
        assert!(MessageBox::parse(&b, 14, 0).is_err());
        let mut bad = b.clone();
        bad.push(0);
        assert!(MessageBox::parse(&bad, 15, 0).is_err());
        let mut bad = b.clone();
        let version = bad.len() - 5;
        bad[version] = 2;
        assert!(MessageBox::parse(&bad, 15, 0).is_err());
        let mut bad = b.clone();
        let version = bad.len() - 14;
        bad[version] = 0;
        assert!(MessageBox::parse(&bad, 15, 0).is_err());
    }
    Ok(())
}

#[test]
fn eight_frame_pieces_and_signed_geometry_are_bounded() -> Result<()> {
    let mut b = vec![0; 3 + 1 + 16]; // prefix, first-reference padding, eight nulls
    for i in -8i16..8 {
        b.extend(i.to_le_bytes());
    }
    let frame = FrameImage::parse(&b, 1, 0)?;
    assert_eq!(frame.geometry, std::array::from_fn(|i| i as i16 - 8));
    assert!(frame.pieces.iter().all(|p| p.class.is_none()));
    for n in 0..b.len() {
        assert!(FrameImage::parse(&b[..n], 1, 0).is_err());
    }
    b.push(0);
    assert!(FrameImage::parse(&b, 1, 0).is_err());
    assert!(FrameImage::parse(&b, 2, 0).is_err());
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_message_window_and_eight_border_bitmaps() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let archive = rugp_assets::RioArchive::open(root)?;
    let node = archive
        .nodes
        .iter()
        .find(|n| n.class.name == "CMsgSelBox")
        .context("message box")?;
    let message = MessageBox::parse(
        &archive.read_node(node.id)?,
        node.class.schema,
        archive.address_shift,
    )?;
    assert_eq!(message.rectangles[2], [80, 402, 864, 190]);
    assert_eq!(message.layout, 0x311);
    let frame = FrameImage::parse(
        &archive.read_at(
            message.frame.offset.context("frame offset")?,
            message.frame.size.context("frame size")? as usize,
        )?,
        1,
        archive.address_shift,
    )?;
    assert_eq!(&frame.geometry[12..], &[6, 6, 5, 5]);
    for (i, piece) in frame.pieces.iter().enumerate() {
        let class = piece.class.as_ref().context("frame piece class")?;
        assert_eq!((class.name.as_str(), class.schema), ("CS5i", 2));
        let bytes = archive.read_at(
            piece.offset.context("piece offset")?,
            piece.size.context("piece size")? as usize,
        )?;
        let image = rugp_assets::s5i::decode_s5i(&bytes, class.schema)?;
        let expected = [
            (6, 6),
            (7, 7),
            (6, 6),
            (6, 6),
            (16, 6),
            (6, 16),
            (6, 16),
            (16, 6),
        ];
        assert_eq!((image.width, image.height), expected[i]);
        if i == 0 {
            assert_eq!(image.rgba[3], 0);
            assert_eq!(
                image.rgba[((image.height - 1) * image.width) as usize * 4 + 3],
                83
            );
        }
    }
    Ok(())
}
