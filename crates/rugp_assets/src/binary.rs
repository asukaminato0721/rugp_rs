use anyhow::{Result, ensure};

pub fn decode_text(bytes: &[u8]) -> String {
    encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned()
}

/// Bounded little-endian input; errors include the byte offset.
#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pub position: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.data.len() - self.position
    }
    pub fn consumed_since(&self, start: usize) -> Result<&'a [u8]> {
        self.data
            .get(start..self.position)
            .ok_or_else(|| anyhow::anyhow!("invalid input range"))
    }
    pub fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        ensure!(
            len <= self.remaining(),
            "truncated input at {:#x}: need {len}, have {}",
            self.position,
            self.remaining()
        );
        let start = self.position;
        self.position += len;
        Ok(&self.data[start..self.position])
    }
    pub fn skip(&mut self, len: usize) -> Result<()> {
        self.bytes(len)?;
        Ok(())
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into()?))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into()?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into()?))
    }
    pub fn count(&mut self) -> Result<usize> {
        let n = self.u16()?;
        Ok(if n == 0xffff {
            self.u32()? as usize
        } else {
            n as usize
        })
    }
    pub fn short_count(&mut self) -> Result<usize> {
        let n = self.u8()?;
        Ok(if n == 0xff {
            self.u16()? as usize
        } else {
            n as usize
        })
    }
    pub fn string(&mut self) -> Result<String> {
        let n = self.u8()?;
        let mut wide = false;
        let len = if n < 0xff {
            n as usize
        } else {
            let mut n = self.u16()?;
            if n == 0xfffe {
                wide = true;
                let b = self.u8()?;
                n = if b == 0xff { self.u16()? } else { b as u16 };
            }
            if n == 0xffff {
                self.u32()? as usize
            } else {
                n as usize
            }
        };
        if wide {
            let bytes = self.bytes(
                len.checked_mul(2)
                    .ok_or_else(|| anyhow::anyhow!("string too long"))?,
            )?;
            let words: Vec<_> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            Ok(String::from_utf16(&words)?)
        } else {
            // MFC CStrings also carry binary identifiers in ICI manifests.
            // Match the original reader's replacement behavior for those bytes.
            let (text, _, _) = encoding_rs::SHIFT_JIS.decode(self.bytes(len)?);
            Ok(text.into_owned())
        }
    }
}

pub struct Bits<'a> {
    data: &'a [u8],
    position: usize,
}
impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }
    pub fn read(&mut self, count: usize) -> Option<u32> {
        if count > 32 || self.position.checked_add(count)? > self.data.len() * 8 {
            return None;
        }
        let mut value = 0;
        for i in 0..count {
            value |= (((self.data[self.position / 8] >> (self.position % 8)) & 1) as u32) << i;
            self.position += 1;
        }
        Some(value)
    }
}
