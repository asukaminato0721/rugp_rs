use anyhow::{Context, Result};
use rugp_assets::menu::{ImageMenu, SelectorMask};

fn class_name(name: &str) -> Vec<u8> {
    let mut bits = vec![true]; // no implicit C prefix
    for c in name.bytes() {
        bits.extend([true, false, false, false, false, false]); // escaped byte
        for bit in 0..8 {
            bits.push(c >> bit & 1 != 0);
        }
    }
    bits.extend([true; 7]); // MAP3[31] terminator
    let mut bytes = vec![0; bits.len().div_ceil(8)];
    for (i, b) in bits.into_iter().enumerate() {
        bytes[i / 8] |= u8::from(b) << (i % 8);
    }
    bytes
}
fn menu_fixture() -> Vec<u8> {
    let mut data = vec![0; 3];
    data.extend(2u32.to_le_bytes());
    data.extend([0; 5]); // first-reference cache padding, two null references
    data.extend(2u16.to_le_bytes());
    for id in 1..=2u32 {
        if id == 1 {
            data.extend(0xffffu16.to_le_bytes());
            data.extend(2u16.to_le_bytes());
            let name = class_name("CImgSel");
            data.push(name.len() as u8);
            data.extend(name);
        } else {
            data.extend(0x8005u16.to_le_bytes());
        }
        for word in [id, id + 10, 0x09400040] {
            data.extend(word.to_le_bytes());
        }
        data.extend([0; 8]); // four image references
        data.push(8);
        data.extend(b"NAME={x}");
        data.extend(0x101u16.to_le_bytes());
        for n in [2u16, 3, 4, 5] {
            data.extend(n.to_le_bytes());
        }
    }
    data.extend(0x10046u32.to_le_bytes());
    data.extend(16u16.to_le_bytes());
    data.extend(8u16.to_le_bytes());
    data.extend([0; 8]); // stride and preserved backing-stream trailer
    data
}
#[test]
fn menu_raw_object_cache_is_distinct_from_rio_references() -> Result<()> {
    let data = menu_fixture();
    let menu = ImageMenu::parse(&data, 1, 0)?;
    assert_eq!((menu.width, menu.height), (16, 8));
    assert_eq!(menu.selectors.len(), 2);
    for (i, selector) in menu.selectors.iter().enumerate() {
        assert_eq!(selector.header, i as u32 + 1);
        assert_eq!(selector.id, i as u32 + 11);
        assert_eq!(selector.rect, [2, 3, 4, 5]);
        assert_eq!(selector.properties.read_string("NAME", ""), "x");
    }
    assert_eq!(menu.trailing_data, [0; 4]);
    for len in 0..data.len() - 4 {
        assert!(ImageMenu::parse(&data[..len], 1, 0).is_err());
    }
    assert!(ImageMenu::parse(&data, 2, 0).is_err());
    Ok(())
}
fn mask_fixture(payload: &[u8]) -> Vec<u8> {
    let mut data = vec![0; 3];
    data.extend(69u32.to_le_bytes());
    for word in [1u16, 2, 4, 2, 4, 2] {
        data.extend(word.to_le_bytes());
    }
    data.extend(0x10001u32.to_le_bytes());
    data.extend((payload.len() as u32).to_le_bytes());
    data.extend([0; 4]);
    data.extend(payload);
    data
}
#[test]
fn indexed_mask_resets_color_each_row_and_rejects_overruns() -> Result<()> {
    let data = mask_fixture(&[0, 7, 2, 8, 2, 1, 9, 3]);
    let mask = SelectorMask::parse(&data)?;
    assert_eq!(mask.pixels, [7, 7, 8, 8, 0, 9, 9, 9]);
    assert_eq!(mask.at(1, 2), Some(7));
    assert_eq!(mask.at(4, 3), Some(9));
    assert_eq!(mask.at(0, 2), None);
    assert_eq!(mask.at(5, 2), None);
    for len in 0..data.len() {
        assert!(SelectorMask::parse(&data[..len]).is_err());
    }
    assert!(SelectorMask::parse(&mask_fixture(&[5])).is_err());
    assert!(SelectorMask::parse(&mask_fixture(&[4, 4, 1])).is_err());
    Ok(())
}
#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_title_menu_and_exact_indexed_hit_regions() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let archive = rugp_assets::RioArchive::open(root)?;
    let menu = ImageMenu::parse(&archive.read_at(0xec84908, 1119)?, 1, archive.address_shift)?;
    assert_eq!(menu.selectors.len(), 5);
    assert_eq!(
        menu.selectors.iter().map(|s| s.id).collect::<Vec<_>>(),
        [0, 1, 2, 3, 4]
    );
    let mask = SelectorMask::parse(&archive.read_at(
        menu.base.offset.context("mask offset")?,
        menu.base.size.context("mask size")? as usize,
    )?)?;
    for (x, y, id) in [
        (0, 0, 1),
        (200, 100, 2),
        (200, 160, 3),
        (200, 217, 4),
        (200, 275, 5),
    ] {
        assert_eq!(mask.at(x, y), Some(id));
    }
    Ok(())
}
