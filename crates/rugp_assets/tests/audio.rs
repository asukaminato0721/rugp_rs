use rugp_assets::oor::{Oor, SetupHeaders, VorbisDecoder};

#[derive(Default)]
struct Codes(Vec<bool>);
impl Codes {
    fn push(&mut self, value: u64, count: usize) {
        for bit in (0..count).rev() {
            self.0.push((value >> bit) & 1 != 0);
        }
    }
    fn bytes(mut self) -> Vec<u8> {
        self.0.resize(self.0.len().div_ceil(8) * 8, false);
        self.0
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| c.iter().fold(0, |n, b| (n << 1) | u8::from(*b)))
            .collect()
    }
}
fn page(flags: u64, packets: &[&[u8]]) -> Vec<u8> {
    let mut bits = Codes::default();
    bits.push(0, 2);
    bits.push(flags, 4);
    bits.push(8, 4);
    bits.push(0, 1);
    bits.push(packets.len() as u64, 8);
    bits.push(0, 2);
    for packet in packets {
        bits.push(packet.len() as u64, 8);
    }
    let mut bytes = bits.bytes();
    for packet in packets {
        bytes.extend(*packet);
    }
    bytes
}
fn fixture() -> Vec<u8> {
    // The complete three-byte identification page; these are NOT RIO prefix bytes.
    let mut data = vec![0x08, 0x0d, 0x17]; // mono, 44100 Hz, blocks 256/2048
    data.extend(page(0, &[&[1], &[]])); // external setup #1, empty comment placeholder
    data.extend(page(4, &[&[0x02, 0x04]])); // partial audio packet
    data.extend(page(8 | 1, &[&[0x06]])); // continuation + EOS
    data
}

#[test]
fn oor_identification_fragmentation_and_truncation() {
    let data = fixture();
    let stream = Oor::parse(&data).unwrap();
    assert_eq!(stream.info.channels, 1);
    assert_eq!(stream.info.sample_rate, 44100);
    assert_eq!(stream.info.block_sizes, (8, 11));
    assert_eq!(stream.setup_id, 1);
    assert_eq!(stream.packets.len(), 1);
    assert_eq!(stream.packets[0].data, [2, 4, 6]);
    assert!(VorbisDecoder::new(stream, &SetupHeaders::default()).is_err());
    for length in 0..data.len() {
        assert!(
            Oor::parse(&data[..length]).is_err(),
            "accepted truncated stream at {length}"
        );
    }
    let mut extra = data.clone();
    extra.push(0);
    assert_eq!(Oor::parse(&extra).unwrap().trailing_data, [0]);
    let mut changed = data;
    *changed.last_mut().unwrap() = 7; // framing does not decode Vorbis itself
    assert!(Oor::parse(&changed).is_ok());
}

#[test]
fn oor_rejects_corrupt_headers_and_continuations() {
    for input in [
        vec![],
        vec![0xff],
        vec![0x08, 0x00, 0],
        vec![0x08, 0x0d, 0x16],
    ] {
        assert!(Oor::parse(&input).is_err());
    }
    let mut data = vec![0x08, 0x0d, 0x17];
    data.extend(page(0, &[&[1]]));
    data.extend(page(8 | 1, &[&[0]]));
    assert!(Oor::parse(&data).is_err());
    let headers = SetupHeaders::from_plugin(&[0; 256]);
    assert!(headers.is_empty());
}
