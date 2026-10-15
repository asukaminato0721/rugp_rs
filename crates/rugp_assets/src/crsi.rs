//! CRsi#1 stream-image index. Frame payloads remain in the split RIO archive.
use crate::{
    RioArchive,
    binary::Reader,
    object::{ObjectReader, ObjectRef},
};
use anyhow::{Context, Result, ensure};

pub mod codec;

/// Stored CVideoSeq schema 4 defaults. Play messages replace source, target,
/// style, frame/loop counters; position and rate override remain on the object.
pub struct VideoSequence {
    pub source: ObjectRef,
    pub target: ObjectRef,
    pub rect: [i16; 4],
    pub frame: u32,
    pub style: u32,
    pub loop_iteration: u32,
    pub loops: i32,
    pub x: i32,
    pub y: i32,
    pub rate: u32,
    pub restore_auto_update: bool,
}
impl VideoSequence {
    pub fn parse(data: &[u8], shift: u32) -> Result<Self> {
        let mut r = ObjectReader::new(data);
        r.input.skip(3)?;
        r.scrambled = true;
        r.pad(3)?;
        let source = r.reference(shift)?;
        let target = r.reference(shift)?;
        let mut rect = [0; 4];
        for value in &mut rect {
            *value = r.input.u16()? as i16;
        }
        let frame = r.input.u32()?;
        let style = r.input.u32()?;
        let loop_iteration = r.input.u32()?;
        let loops = r.input.u32()? as i32;
        let x = r.input.u32()? as i32;
        let y = r.input.u32()? as i32;
        let rate = r.input.u32()?;
        let restore_auto_update = r.input.u8()? != 0;
        ensure!(r.input.remaining() == 0, "trailing CVideoSeq bytes");
        Ok(Self {
            source,
            target,
            rect,
            frame,
            style,
            loop_iteration,
            loops,
            x,
            y,
            rate,
            restore_auto_update,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FrameIndex {
    pub kind: u8,
    pub offset: u32,
    pub unknown: u16,
    pub codec: u16,
    pub auxiliary: Option<u16>,
    pub size: u32,
}
impl FrameIndex {
    pub fn channel_depths(&self) -> [u8; 3] {
        [
            ((self.codec & 7) + 1) as u8,
            (((self.codec >> 3) & 7) + 1) as u8,
            (((self.codec >> 6) & 7) + 1) as u8,
        ]
    }
    pub fn codec_id(&self) -> u8 {
        ((self.codec >> 10) & 7) as u8
    }
    pub fn is_keyframe(&self) -> bool {
        self.kind & 1 != 0
    }
}

#[derive(Debug, serde::Serialize)]
pub struct Crsi {
    pub flags: u16,
    pub modes: [u8; 2],
    pub unknown: u32,
    pub width: u16,
    pub height: u16,
    pub parameter: u32,
    /// Hundredths of a frame per second (3000 means 30 fps).
    pub rate: u16,
    pub frames: Vec<FrameIndex>,
    /// Relative to the complete CRsi extent, including its three-byte prefix.
    pub payload_start: u32,
    pub payload_size: u32,
    pub extent_size: u32,
    /// The retail extents have 17 bytes after indexed frame payloads.
    pub trailing_data: Vec<u8>,
}
impl Crsi {
    const MAX_FRAMES: usize = 1_000_000;
    fn count(r: &mut Reader<'_>) -> Result<usize> {
        r.skip(21)?; // prefix, common parameters, dimensions, rate
        let count = r.count()?;
        ensure!(
            (1..=Self::MAX_FRAMES).contains(&count),
            "invalid CRsi frame count {count}"
        );
        Ok(count)
    }
    /// Parses only the index. `data` need not contain any compressed frames.
    pub fn parse_index(data: &[u8], extent_size: u32) -> Result<Self> {
        let count = Self::count(&mut Reader::new(data))?;
        let mut r = Reader::new(data);
        r.skip(3)?;
        let flags = r.u16()?;
        let modes = [r.u8()?, r.u8()?];
        let unknown = r.u32()?;
        let width = r.u16()?;
        let height = r.u16()?;
        let parameter = r.u32()?;
        let rate = r.u16()?;
        r.count()?;
        ensure!(
            width > 0 && height > 0 && u64::from(width) * u64::from(height) <= 16_777_216,
            "invalid CRsi dimensions"
        );
        ensure!(rate > 0, "zero CRsi frame rate");
        ensure!(count <= r.remaining() / 13, "truncated CRsi frame index");
        let mut frames = Vec::with_capacity(count);
        let mut end = 0u32;
        for index in 0..count {
            let kind = r.u8()?;
            let offset = r.u32()?;
            let unknown = r.u16()?;
            let codec = r.u16()?;
            let auxiliary = if codec & 0x1c00 == 0x400 {
                Some(r.u16()?)
            } else {
                None
            };
            let size = r.u32()?;
            ensure!(size > 0, "empty CRsi frame {index}");
            ensure!(offset >= end, "overlapping/out-of-order CRsi frame {index}");
            end = offset
                .checked_add(size)
                .context("CRsi frame offset overflow")?;
            frames.push(FrameIndex {
                kind,
                offset,
                unknown,
                codec,
                auxiliary,
                size,
            });
        }
        let payload_start = u32::try_from(r.position)?;
        ensure!(
            u64::from(payload_start) + u64::from(end) <= u64::from(extent_size),
            "CRsi frame exceeds resource extent"
        );
        Ok(Self {
            flags,
            modes,
            unknown,
            width,
            height,
            parameter,
            rate,
            frames,
            payload_start,
            payload_size: end,
            extent_size,
            trailing_data: Vec::new(),
        })
    }
    pub fn read(archive: &RioArchive, offset: u64, size: u32) -> Result<Self> {
        let header = archive.read_at(offset, (size as usize).min(27))?;
        let count = Self::count(&mut Reader::new(&header))?;
        // An entry uses 13 or 15 bytes; the count uses 2 or 6. No frame-sized
        // allocation is needed to discover or validate the complete index.
        let max_index_size = (27 + count * 15).min(size as usize);
        let data = archive.read_at(offset, max_index_size)?;
        let mut stream = Self::parse_index(&data, size)?;
        let end = stream
            .payload_start
            .checked_add(stream.payload_size)
            .context("CRsi payload offset overflow")?;
        let tail = size - end;
        ensure!(tail <= 1024 * 1024, "oversized CRsi trailer");
        stream.trailing_data = archive.read_at(offset + u64::from(end), tail as usize)?;
        Ok(stream)
    }
    pub fn frame_data(&self, archive: &RioArchive, offset: u64, index: usize) -> Result<Vec<u8>> {
        let frame = self
            .frames
            .get(index)
            .context("CRsi frame index out of range")?;
        ensure!(
            frame.size <= 64 * 1024 * 1024,
            "oversized CRsi compressed frame"
        );
        archive.read_at(
            offset + u64::from(self.payload_start) + u64::from(frame.offset),
            frame.size as usize,
        )
    }
}
