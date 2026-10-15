use rugp_assets::{RioArchive, object::ObjectReader};
fn main() -> anyhow::Result<()> {
    let archive = RioArchive::open(std::env::args_os().nth(1).expect("game directory"))?;
    println!("{:#?}", archive.project);
    let bytes = archive.read_node(1)?;
    let mut r = ObjectReader::new(&bytes[3..]);
    r.scrambled = true;
    r.pad(3)?;
    println!("Process version {:#x}", r.input.u32()?);
    for _ in 0..5 {
        println!("{:#?}", r.reference(archive.address_shift)?);
    }
    Ok(())
}
