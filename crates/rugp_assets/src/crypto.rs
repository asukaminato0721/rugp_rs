//! RIO/ICI transforms, based on GARbro's MIT-licensed ArcRIO.cs (see THIRD_PARTY.md).
use crate::binary::Reader;
use anyhow::{Result, ensure};

pub const ICI_KEY: u32 = 0xb29d5a0c;
pub const RIO_KEY: u32 = 0x7e6b8ce2;
pub const MAX_CHUNK_SIZE: usize = 256 * 1024 * 1024;

pub fn read_encrypted(input: &mut Reader<'_>, mut key: u32) -> Result<Vec<u8>> {
    let size = !(input.u32()? ^ 0xc92e568b) as usize;
    let check_size = (input.u32()? ^ 0xc92e568f) >> 3;
    ensure!(
        size == check_size as usize,
        "invalid encrypted chunk length at {:#x}",
        input.position - 8
    );
    ensure!(
        size <= MAX_CHUNK_SIZE,
        "encrypted chunk exceeds memory limit: {size}"
    );
    ensure!(
        size.checked_add(size / 32 * 2)
            .is_some_and(|n| n <= input.remaining()),
        "truncated encrypted chunk"
    );
    let mut output = Vec::with_capacity(size);
    while output.len() < size {
        let portion = (size - output.len()).min(32);
        let mut checksum = 0u16;
        for i in 0..portion {
            let value = input.u8()? ^ key as u8;
            checksum = checksum.wrapping_add(value as u16 * (portion - i) as u16);
            output.push(value);
            key = !(key
                .wrapping_mul(2)
                .wrapping_add(0xa3b376c9)
                .wrapping_add((key >> 15) & 1));
        }
        if portion == 32 {
            ensure!(
                input.u16()? == checksum,
                "encrypted chunk checksum mismatch at {:#x}",
                input.position - 2
            );
        }
    }
    Ok(output)
}

fn interleave(data: &[u8], lanes: usize, masks: &[u8]) -> Vec<u8> {
    let mut out = vec![0; data.len()];
    let count = data.len() / lanes;
    for i in 0..count {
        for lane in 0..lanes {
            out[i * lanes + lane] = data[i + count * lane] ^ masks[lane];
        }
    }
    out[count * lanes..].copy_from_slice(&data[count * lanes..]);
    out
}

pub fn decode_ici(data: &[u8]) -> Result<Vec<u8>> {
    let decrypted = read_encrypted(&mut Reader::new(data), ICI_KEY)?;
    let mut stage = interleave(&decrypted, 6, &[0; 6]);
    let mut previous = 0u8;
    for value in &mut stage {
        let original = *value;
        *value = value.wrapping_sub(previous) ^ 0xa5;
        previous = original;
    }
    let mut stage = interleave(&stage, 5, &[0; 5]);
    previous = 0;
    for value in stage.iter_mut().rev() {
        let original = *value;
        *value = value.wrapping_sub(previous);
        previous = original;
    }
    Ok(interleave(&stage, 3, &[0x18, 0x3f, 0xe2]))
}

pub fn decode_offset(value: u32) -> u64 {
    value.wrapping_sub(0xa2fb6ad1) as u64
}
pub fn decode_size(value: u32) -> u32 {
    let a = value.wrapping_sub(0xe7b5d9f8);
    let b = a >> 13;
    a.wrapping_sub(b & 0xfff).wrapping_shl(19) | b
}
