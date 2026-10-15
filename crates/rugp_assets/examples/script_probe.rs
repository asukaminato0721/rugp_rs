use rugp_assets::{
    RioArchive,
    binary::Reader,
    crypto::{RIO_KEY, read_encrypted},
};
fn main() -> anyhow::Result<()> {
    let root = std::env::args_os().nth(1).expect("game directory");
    let archive = RioArchive::open(root)?;
    let selected = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<usize>().ok());
    let offset = std::env::args()
        .nth(3)
        .map(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16))
        .transpose()?
        .unwrap_or(0);
    for node in archive
        .nodes
        .iter()
        .filter(|n| n.class.name == "CRsa" && selected.is_none_or(|id| n.id == id))
        .take(6)
    {
        let data = archive.read_node(node.id)?;
        let mut r = Reader::new(&data);
        r.skip(12)?;
        let decrypted = read_encrypted(&mut r, RIO_KEY)?;
        println!("Object {}: {} bytes", node.id, decrypted.len());
        for (i, row) in decrypted[offset..].chunks(16).take(20).enumerate() {
            print!("{:06x} ", offset + i * 16);
            for b in row {
                print!("{b:02x} ");
            }
            println!(" {}", encoding_rs::SHIFT_JIS.decode(row).0.escape_debug());
        }
    }
    Ok(())
}
