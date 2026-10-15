use rugp_assets::{
    binary::Reader,
    crypto::{RIO_KEY, read_encrypted},
    object::{ClassId, ObjectReader},
};

const ENCRYPTED: &[u8] = &[
    86, 169, 209, 54, 159, 87, 46, 201, 176, 4, 39, 249, 60, 140, 111, 134, 227, 240, 240, 101,
    124, 133, 49, 213, 152, 59, 30, 98, 209, 185, 225, 99, 119, 222, 95, 170, 215, 172, 243, 88,
    104, 193, 135, 80,
];

#[test]
fn encrypted_chunk_vector_and_every_truncation() {
    let mut reader = Reader::new(ENCRYPTED);
    assert_eq!(
        read_encrypted(&mut reader, RIO_KEY).unwrap(),
        b"Rust rUGP encrypted chunk fixture!"
    );
    assert_eq!(reader.remaining(), 0);
    for length in 0..ENCRYPTED.len() {
        assert!(
            read_encrypted(&mut Reader::new(&ENCRYPTED[..length]), RIO_KEY).is_err(),
            "accepted truncated length {length}"
        );
    }
    let mut corrupt = ENCRYPTED.to_vec();
    corrupt[40] ^= 1;
    assert!(
        read_encrypted(&mut Reader::new(&corrupt), RIO_KEY)
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
    corrupt[0] ^= 1;
    assert!(read_encrypted(&mut Reader::new(&corrupt), RIO_KEY).is_err());
}

#[test]
fn mfc_cp932_extended_and_utf16_strings() {
    let data = [
        4, 0x8c, 0x4e, 0x96, 0x5d, 0xff, 0xfe, 0xff, 2, 0x22, 0x6f, 0x57, 0x5b,
    ];
    let mut r = Reader::new(&data);
    assert_eq!(r.string().unwrap(), "君望");
    assert_eq!(r.string().unwrap(), "漢字");
    assert_eq!(r.remaining(), 0);
    let mut long = vec![0xff, 0xff, 0xff];
    long.extend(256u32.to_le_bytes());
    long.extend(vec![b'A'; 256]);
    assert_eq!(Reader::new(&long).string().unwrap(), "A".repeat(256));
}

#[test]
fn inline_posture_versions_keep_following_objects_aligned() {
    for (version, tail) in [(0, 56), (1, 56), (2, 70)] {
        let mut data = vec![1, version, 0, 0]; // enabled, embedded version, options, ancestor count
        data.extend([0; 10]); // five null references
        data.extend(vec![0; tail]);
        data.extend(0x12345678u32.to_le_bytes());
        let mut r = ObjectReader::new(&data);
        let object = r
            .read_inline(
                &ClassId {
                    name: "CPostureMoment".into(),
                    schema: 4,
                },
                0,
            )
            .unwrap();
        assert_eq!(object.references.len(), 5);
        assert_eq!(r.input.u32().unwrap(), 0x12345678);
        assert_eq!(r.input.remaining(), 0);
    }
}

#[test]
fn corrupt_object_headers_and_class_cache_fail_without_panicking() {
    for len in 0..16 {
        let data = vec![0; len];
        assert!(ObjectReader::new(&data).header().is_err());
    }
    assert!(ObjectReader::new(&[0xff, 0x8f]).class().is_err());
    let data = [0xff, 0xff, 1, 0, 4, 0, b'C', b'T', b'e', b's'];
    for len in 0..data.len() {
        assert!(ObjectReader::new(&data[..len]).class().is_err());
    }
}
