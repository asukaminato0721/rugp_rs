use anyhow::{Context, Result};
use rugp_assets::{RioArchive, layout_text::LayoutText};

fn fixture(count: u16) -> Vec<u8> {
    let mut b = vec![0; 3];
    b.extend(2u16.to_le_bytes());
    for n in [0u32, 0x123456, 0x80ffffff] {
        b.extend(n.to_le_bytes());
    }
    for face in [b"serif".as_slice(), b"sans".as_slice()] {
        b.push(face.len() as u8);
        b.extend(face);
    }
    for n in [17u32, 14, 0, 0x80000000, 12, 34, 1024, 600, 0] {
        b.extend(n.to_le_bytes());
    }
    let text = b"credits\nexample";
    b.push(text.len() as u8);
    b.extend(text);
    b.push(1);
    b.extend(0x1e57u16.to_le_bytes());
    b.extend([0xff, 0xff, 1, 0, 3, 0x70, 0x4e, 0xff]); // CSbm#1
    b.extend(count.to_le_bytes());
    if count != 0 {
        b.push(4); // native first-reference context count, not an array element
        for _ in 0..count {
            b.extend([0; 2]);
        }
    }
    b
}

#[test]
fn layout_text_keeps_typed_styles_fonts_extent_and_reference_array() -> Result<()> {
    for count in [0, 2, 10] {
        let b = fixture(count);
        let layout = LayoutText::parse(&b, 1, 0)?;
        assert_eq!(layout.text, "credits\nexample");
        assert_eq!(layout.font_faces, ["serif", "sans"]);
        assert_eq!(layout.font_sizes, [17, 14]);
        assert_eq!(layout.background_color, 0x123456);
        assert_eq!(layout.text_style, 0x80ffffff);
        assert_eq!(layout.secondary_style, 0x80000000);
        assert_eq!(layout.rectangle, [12, 34, 1024, 600]);
        assert_eq!(layout.images.len(), usize::from(count));
        assert!(layout.images.iter().all(|r| r.class.is_none()));
        for n in 0..b.len() {
            assert!(LayoutText::parse(&b[..n], 1, 0).is_err(), "cut {n}");
        }
        let mut bad = b.clone();
        bad.push(0);
        assert!(LayoutText::parse(&bad, 1, 0).is_err());
        assert!(LayoutText::parse(&b, 2, 0).is_err());
        let mut bad = b.clone();
        bad[3] = 3;
        assert!(LayoutText::parse(&bad, 1, 0).is_err());
        let mut bad = b.clone();
        let footer = bad.len()
            - if count == 0 {
                13
            } else {
                14 + 2 * usize::from(count)
            };
        bad[footer] = 2;
        assert!(LayoutText::parse(&bad, 1, 0).is_err());
        let mut bad = b.clone();
        bad[footer + 1] = 0;
        assert!(LayoutText::parse(&bad, 1, 0).is_err());
        let mut bad = b.clone();
        bad[footer + 5] = 2;
        assert!(LayoutText::parse(&bad, 1, 0).is_err());
        let mut bad = b.clone();
        bad[footer + 11..footer + 13].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(LayoutText::parse(&bad, 1, 0).is_err());
    }
    Ok(())
}

#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_ending_layout_text_and_null_bitmap_slots() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
    let archive = RioArchive::open(root)?;
    for (offset, size, width, slots, style) in [
        (0x6ad8fe2a, 5848, 472, 10, 0x80ffffff),
        (0x74a408d7, 5267, 1024, 10, 0x80ffffff),
        (0x8b9735b0, 4279, 472, 1, 0x80ffffff),
        (0x943cafe2, 5848, 400, 10, 0x80000000),
    ] {
        let data = archive.read_at(offset, size)?;
        let layout = LayoutText::parse(&data, 1, archive.address_shift)?;
        assert_eq!(layout.rectangle, [0, 0, width, 600]);
        assert_eq!(layout.font_sizes, [17, 14]);
        assert_eq!(layout.text_style, style);
        assert!(!layout.text.is_empty());
        assert_eq!(layout.images.len(), slots);
        assert!(layout.images.iter().all(|r| r.class.is_none()));
    }
    Ok(())
}
