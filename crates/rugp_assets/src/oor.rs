//! OptimizedObs (OOR) framing and streaming, pure-Rust Vorbis decoding.
//! Unlike most RIO objects these extents have no three-byte object prefix.
use anyhow::{Context, Result, ensure};
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone)]
pub struct AudioInfo {
    pub channels: u8,
    pub sample_rate: u32,
    pub block_sizes: (u8, u8),
    pub final_granule: Option<u64>,
}
#[derive(Debug)]
pub struct AudioPacket {
    pub data: Vec<u8>,
    pub final_granule: Option<u64>,
}
pub struct Oor {
    pub info: AudioInfo,
    pub setup_id: u8,
    pub inline_setup: Vec<u8>,
    pub packets: Vec<AudioPacket>,
    /// Bytes in the RIO extent after the logical EOS page. Two retail voice
    /// objects contain such tails; the original codec stops at EOS as well.
    pub trailing_data: Vec<u8>,
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}
impl<'a> Bits<'a> {
    fn read(&mut self, count: usize) -> Result<u64> {
        ensure!(
            count <= 64 && count <= self.data.len() * 8 - self.pos,
            "truncated OOR bitstream"
        );
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | ((self.data[self.pos / 8] >> (7 - self.pos % 8)) & 1) as u64;
            self.pos += 1;
        }
        Ok(value)
    }
    fn zero(&mut self, count: usize) -> Result<()> {
        ensure!(self.read(count)? == 0, "nonzero OOR padding");
        Ok(())
    }
    fn align(&mut self) -> Result<()> {
        self.zero((8 - self.pos % 8) % 8)
    }
    fn bytes(&mut self, count: usize) -> Result<&'a [u8]> {
        ensure!(self.pos.is_multiple_of(8), "unaligned OOR packet");
        let start = self.pos / 8;
        let end = start.checked_add(count).context("OOR extent overflow")?;
        let value = self.data.get(start..end).context("truncated OOR packet")?;
        self.pos = end * 8;
        Ok(value)
    }
}
struct Page<'a> {
    version: u8,
    flags: u8,
    granule: Option<u64>,
    packets: Vec<&'a [u8]>,
}
impl<'a> Page<'a> {
    fn read(bits: &mut Bits<'a>) -> Result<Self> {
        let version = bits.read(2)? as u8;
        ensure!(version <= 1, "unsupported OOR page version {version}");
        let flags = bits.read(4)? as u8;
        let granule = if version == 1 {
            bits.zero(2)?;
            let v = bits.read(64)?;
            bits.zero(6)?;
            Some(v)
        } else {
            None
        };
        let mut lengths = Vec::new();
        if flags & 2 != 0 {
            // The identification payload is byte-aligned, as all packet bodies are.
            let body = bits.pos.div_ceil(8);
            let byte = *bits
                .data
                .get(body)
                .context("truncated OOR identification")?;
            let extended = (byte >> 1) & 3 == 3;
            lengths.push(2 + usize::from(extended) + if version > 0 { 10 } else { 0 });
        } else {
            let width = bits.read(4)? as usize;
            bits.zero(1)?;
            let count = bits.read(8)? as usize;
            let base_width = match bits.read(2)? {
                0 => 0,
                1 => 8,
                2 => 11,
                _ => anyhow::bail!("invalid OOR base length selector"),
            };
            let base = bits.read(base_width)? as usize;
            for _ in 0..count {
                lengths.push(base + bits.read(width)? as usize);
            }
        }
        bits.align()?;
        let mut packets = Vec::with_capacity(lengths.len());
        for length in lengths {
            packets.push(bits.bytes(length)?);
        }
        Ok(Self {
            version,
            flags,
            granule,
            packets,
        })
    }
}

impl Oor {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut bits = Bits { data, pos: 0 };
        let page = Page::read(&mut bits)?;
        ensure!(
            page.flags == 2 && page.packets.len() == 1,
            "OOR does not begin with an identification page"
        );
        let mut id = Bits {
            data: page.packets[0],
            pos: 0,
        };
        let version = id.read(2)? as u8;
        ensure!(
            version == page.version,
            "OOR identification/page versions differ"
        );
        let channels = id.read(3)? as u8;
        ensure!(
            (1..=7).contains(&channels),
            "invalid OOR channel count {channels}"
        );
        let rate = id.read(2)?;
        let sample_rate = if rate < 3 {
            11025 << rate
        } else {
            match (version, id.read(8)?) {
                (0, 3) | (1, 4) => 32000,
                (0, 4) | (1, 5) => 48000,
                (0, 5) | (1, 8) => 96000,
                (1, 6) => 64000,
                (1, 7) => 88200,
                other => anyhow::bail!("unsupported OOR sample rate selector {other:?}"),
            }
        };
        let final_granule = if version == 1 {
            id.read(9)?;
            Some(id.read(64)?)
        } else {
            None
        };
        let block_sizes = (id.read(4)? as u8, id.read(4)? as u8);
        ensure!(
            (6..=13).contains(&block_sizes.0)
                && block_sizes.0 <= block_sizes.1
                && block_sizes.1 <= 13,
            "invalid OOR block sizes"
        );
        ensure!(id.read(1)? == 1, "missing OOR identification framing bit");
        id.align()?;
        // Reassemble logical packets before interpreting either header.
        // Empty external-setup payloads and nonempty inline-setup payloads
        // can both continue across page boundaries, just like audio packets.
        let mut logical = Vec::new();
        let mut partial = Vec::new();
        let mut pending = false;
        let mut eos = false;
        while bits.pos < data.len() * 8 && !eos {
            let page = Page::read(&mut bits)?;
            ensure!(
                page.version == version && page.flags & 2 == 0,
                "OOR page version changed or unexpected BOS"
            );
            ensure!(
                (page.flags & 8 != 0) == pending,
                "OOR packet continuation mismatch"
            );
            ensure!(
                !page.packets.is_empty() || page.flags & 12 == 0,
                "empty fragmented OOR page"
            );
            let count = page.packets.len();
            for (i, payload) in page.packets.into_iter().enumerate() {
                partial.extend_from_slice(payload);
                ensure!(partial.len() <= 16_777_216, "OOR packet exceeds limit");
                pending = i + 1 == count && page.flags & 4 != 0;
                if !pending {
                    logical.push(AudioPacket {
                        data: std::mem::take(&mut partial),
                        final_granule: if i + 1 == count && page.flags & 1 != 0 {
                            page.granule
                        } else {
                            None
                        },
                    });
                }
            }
            eos = page.flags & 1 != 0;
        }
        ensure!(!pending, "truncated OOR final packet");
        ensure!(version == 0 || eos, "missing OOR v1 EOS page");
        let mut logical = logical.into_iter();
        let first = logical
            .next()
            .context("missing OOR setup identifier packet")?
            .data;
        let payload = logical
            .next()
            .context("missing OOR setup payload packet")?
            .data;
        let setup_id = first.first().context("empty OOR setup identifier")? & 63;
        let inline_setup = if setup_id == 0 {
            first[1..].iter().chain(&payload).copied().collect()
        } else {
            ensure!(
                first.len() == 1 && payload.is_empty(),
                "unexpected payload for external OOR setup {setup_id}"
            );
            Vec::new()
        };
        let packets: Vec<_> = logical.collect();
        // Version 0 streams do not always carry an EOS flag or sample count.
        ensure!(!packets.is_empty(), "OOR has no audio packets");
        Ok(Self {
            info: AudioInfo {
                channels,
                sample_rate,
                block_sizes,
                final_granule,
            },
            setup_id,
            inline_setup,
            packets,
            trailing_data: data[bits.pos / 8..].to_vec(),
        })
    }
}

/// Standard libvorbis 1.0 setup packets are recovered from the user's codec
/// plugin. No game binary or codec payload is embedded in this repository.
type SetupKey = (u8, u8, u8, u8);
#[derive(Default)]
pub struct SetupHeaders {
    packets: BTreeMap<u8, Vec<u8>>,
    parsed: Mutex<BTreeMap<SetupKey, Arc<lewton::header::SetupHeader>>>,
}
impl SetupHeaders {
    pub fn from_plugin(data: &[u8]) -> Self {
        const PROFILES: [(usize, &str); 6] = [
            (3288, "44a0a0ebf68508c81999bfee18882e73f08633e9"),
            (2930, "35317cb593d127f30c589ccbdb111802f7f7398c"),
            (3575, "0fd840c8246d4dbea6467eff468ad3476f8483b0"),
            (4133, "3612e56744fbdcdae2241d917f5ad2b430880f24"),
            (3073, "ed285ef5553f492664ee8cd13c98e59857b3a093"),
            (3073, "707c8fb7bd247e3e91f1952107f8872d2b5a3ac9"),
        ];
        let mut headers = Self::default();
        for i in 1..data.len().saturating_sub(3) {
            if &data[i..i + 3] != b"BCV" {
                continue;
            }
            for (id, (size, hash)) in PROFILES.iter().enumerate() {
                let Some(packet) = data.get(i - 1..i - 1 + size) else {
                    continue;
                };
                if format!("{:x}", Sha1::digest(packet)) == *hash {
                    headers.packets.insert(id as u8 + 1, packet.to_vec());
                }
            }
        }
        headers
    }
    pub fn len(&self) -> usize {
        self.packets.len()
    }
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }
}

pub struct VorbisDecoder {
    pub info: AudioInfo,
    ident: lewton::header::IdentHeader,
    setup: Arc<lewton::header::SetupHeader>,
    window: lewton::audio::PreviousWindowRight,
    packets: std::vec::IntoIter<AudioPacket>,
    frames: u64,
}
impl VorbisDecoder {
    pub fn new(stream: Oor, headers: &SetupHeaders) -> Result<Self> {
        let info = stream.info;
        let mut ident = b"\x01vorbis".to_vec();
        ident.extend(0u32.to_le_bytes());
        ident.push(info.channels);
        ident.extend(info.sample_rate.to_le_bytes());
        ident.extend([0; 12]);
        ident.push(info.block_sizes.0 | (info.block_sizes.1 << 4));
        ident.push(1);
        let ident = lewton::header::read_header_ident(&ident)
            .context("decode reconstructed Vorbis identification")?;
        let mut setup = b"\x05vorbis".to_vec();
        setup.extend(if stream.setup_id == 0 {
            &stream.inline_setup
        } else {
            headers.packets.get(&stream.setup_id).with_context(|| {
                format!("OOR setup {} not found in codec plugin", stream.setup_id)
            })?
        });
        let key = (
            stream.setup_id,
            info.channels,
            info.block_sizes.0,
            info.block_sizes.1,
        );
        let cached = if stream.setup_id != 0 {
            headers
                .parsed
                .lock()
                .map_err(|_| anyhow::anyhow!("Vorbis setup cache poisoned"))?
                .get(&key)
                .cloned()
        } else {
            None
        };
        let setup = if let Some(cached) = cached {
            cached
        } else {
            let parsed = Arc::new(
                lewton::header::read_header_setup(&setup, info.channels, info.block_sizes)
                    .with_context(|| {
                        format!(
                            "decode OOR setup {} ({} bytes)",
                            stream.setup_id,
                            setup.len()
                        )
                    })?,
            );
            if stream.setup_id != 0 {
                headers
                    .parsed
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Vorbis setup cache poisoned"))?
                    .insert(key, Arc::clone(&parsed));
            }
            parsed
        };
        Ok(Self {
            info,
            ident,
            setup,
            window: lewton::audio::PreviousWindowRight::new(),
            packets: stream.packets.into_iter(),
            frames: 0,
        })
    }
    pub fn next_samples(&mut self) -> Result<Option<Vec<i16>>> {
        use lewton::samples::InterleavedSamples;
        let Some(packet) = self.packets.next() else {
            return Ok(None);
        };
        let mut samples: InterleavedSamples<i16> = lewton::audio::read_audio_packet_generic(
            &self.ident,
            &self.setup,
            &packet.data,
            &mut self.window,
        )
        .with_context(|| {
            format!(
                "decode Vorbis packet ({} bytes), {} remaining",
                packet.data.len(),
                self.packets.len()
            )
        })?;
        if let Some(end) = packet.final_granule.or_else(|| {
            if self.packets.len() == 0 {
                self.info.final_granule
            } else {
                None
            }
        }) {
            let count = end
                .saturating_sub(self.frames)
                .min(samples.samples.len() as u64 / self.info.channels as u64);
            samples
                .samples
                .truncate(count as usize * self.info.channels as usize);
        }
        self.frames += samples.samples.len() as u64 / self.info.channels as u64;
        Ok(Some(samples.samples))
    }
}
