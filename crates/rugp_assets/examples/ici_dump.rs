fn main() -> anyhow::Result<()> {
    let path = std::env::args_os().nth(1).expect("ICI path");
    let data = rugp_assets::crypto::decode_ici(&std::fs::read(path)?)?;
    for (i, row) in data.chunks(16).enumerate() {
        print!("{:06x} ", i * 16);
        for b in row {
            print!("{b:02x} ");
        }
        println!(" {}", encoding_rs::SHIFT_JIS.decode(row).0.escape_debug());
    }
    Ok(())
}
