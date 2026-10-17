//! UUC object state, independent of RIO resource serialization. These records
//! use the same reference cache as the enclosing runtime stream.
use super::reference_writer::ReferenceWriter;
use crate::object::ObjectRef;
use anyhow::{Result, ensure};

pub struct BoxState<'a> {
    pub enabled: bool,
    pub flags: u32,
    pub attributes: u32,
    pub rectangle: [i16; 4],
    pub body_rectangle: [i16; 4],
    pub text: &'a str,
}

impl BoxState<'_> {
    /// rvmm ordinal 529, version 2.
    pub fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        writer.data.extend(2u32.to_le_bytes());
        writer.data.push(u8::from(self.enabled));
        writer.data.extend(self.flags.to_le_bytes());
        writer.data.extend(self.attributes.to_le_bytes());
        for value in self.rectangle.iter().chain(&self.body_rectangle) {
            writer.data.extend(value.to_le_bytes());
        }
        writer.string(self.text)
    }
}

pub struct ImageBoxState<'a> {
    pub base: BoxState<'a>,
    /// Native bytes +108, +109, +10a.
    pub flags: [u8; 3],
    pub draw_mode: i32,
    /// Native bytes +111, +110, +112, in archive order.
    pub alpha_flags: [u8; 3],
    pub vram: Option<&'a ObjectRef>,
    pub image: Option<&'a ObjectRef>,
    pub overlay: Option<&'a ObjectRef>,
}

impl ImageBoxState<'_> {
    /// rvmm ordinal 532, version 5. The caller selects VRAM only when the
    /// native serializer's nonblack, opaque surface condition holds.
    pub fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        self.base.write(writer)?;
        writer.data.extend(5u32.to_le_bytes());
        writer.data.extend(self.flags);
        writer.data.extend(self.draw_mode.to_le_bytes());
        writer.data.extend(self.alpha_flags);
        writer.data.push(u8::from(self.vram.is_some()));
        if let Some(vram) = self.vram {
            writer.reference(vram)?;
            writer.cached(0)?;
            writer.cached(0)?;
        } else {
            reference(writer, self.image)?;
            reference(writer, self.overlay)?;
        }
        Ok(())
    }
}

pub struct ImageAreaState<'a> {
    pub image: ImageBoxState<'a>,
    /// Members +118 and +11c.
    pub references: [Option<&'a ObjectRef>; 2],
    pub word_120: u32,
    pub flags: [u8; 2],
    pub text: &'a str,
    pub reference_12c: Option<&'a ObjectRef>,
    pub word_138: u32,
}

impl ImageAreaState<'_> {
    /// rvmm ordinal 531, version 4, for UUC archive version greater than 2.
    pub fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        self.image.write(writer)?;
        writer.data.extend(4u32.to_le_bytes());
        for value in self.references {
            reference(writer, value)?;
        }
        writer.data.extend(self.word_120.to_le_bytes());
        writer.data.extend(self.flags);
        writer.string(self.text)?;
        reference(writer, self.reference_12c)?;
        writer.data.extend(self.word_138.to_le_bytes());
        Ok(())
    }
}

pub struct FrameBoxState<'a> {
    pub base: BoxState<'a>,
    pub frame: Option<&'a ObjectRef>,
    pub mode: u8,
    pub rectangle: [i16; 4],
}

impl FrameBoxState<'_> {
    /// rvmm ordinal 534, version 3 (UUC archive version greater than 1).
    pub fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        self.base.write(writer)?;
        writer.data.extend(3u32.to_le_bytes());
        reference(writer, self.frame)?;
        writer.data.push(self.mode);
        for value in self.rectangle {
            writer.data.extend(value.to_le_bytes());
        }
        Ok(())
    }
}

pub struct CharacterFaceState<'a> {
    pub mode: u32,
    pub flags: u32,
    pub parts: [Option<&'a ObjectRef>; 12],
    pub vectors: [f32; 4],
}

impl CharacterFaceState<'_> {
    /// Vm60 1004a220, version 2. CCharBox2 writes this before CCharBox state.
    pub fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        ensure!(
            self.vectors.iter().all(|value| value.is_finite()),
            "nonfinite UUC character face vector"
        );
        writer.data.push(2);
        writer.data.extend(self.mode.to_le_bytes());
        writer.data.extend(self.flags.to_le_bytes());
        for part in self.parts {
            reference(writer, part)?;
        }
        for value in self.vectors {
            writer.data.extend(value.to_le_bytes());
        }
        Ok(())
    }
}

fn reference(writer: &mut ReferenceWriter, value: Option<&ObjectRef>) -> Result<()> {
    match value {
        Some(value) => writer.reference(value),
        None => writer.cached(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> BoxState<'static> {
        BoxState {
            enabled: true,
            flags: 0x1234,
            attributes: 0x5678,
            rectangle: [-2, 3, 16, 20],
            body_rectangle: [-4, 5, 30, 40],
            text: "",
        }
    }

    fn samples() -> Result<Vec<(&'static str, Vec<u8>)>> {
        let mut output = Vec::new();
        let mut writer = ReferenceWriter::new(0)?;
        base().write(&mut writer)?;
        output.push(("box.bin", std::mem::take(&mut writer.data)));
        ImageBoxState {
            base: base(),
            flags: [1, 2, 3],
            draw_mode: 0x12345678,
            alpha_flags: [4, 5, 6],
            vram: None,
            image: None,
            overlay: None,
        }
        .write(&mut writer)?;
        output.push(("image-box.bin", std::mem::take(&mut writer.data)));
        ImageAreaState {
            image: ImageBoxState {
                base: base(),
                flags: [1, 2, 3],
                draw_mode: 0x12345678,
                alpha_flags: [4, 5, 6],
                vram: None,
                image: None,
                overlay: None,
            },
            references: [None; 2],
            word_120: 0x11223344,
            flags: [0x5a, 0xa5],
            text: "",
            reference_12c: None,
            word_138: 0x55667788,
        }
        .write(&mut writer)?;
        output.push(("image-area.bin", std::mem::take(&mut writer.data)));
        FrameBoxState {
            base: base(),
            frame: None,
            mode: 7,
            rectangle: [-4, 5, 30, 40],
        }
        .write(&mut writer)?;
        output.push(("frame-box.bin", std::mem::take(&mut writer.data)));
        CharacterFaceState {
            mode: 0x11223344,
            flags: 0xaabbccdd,
            parts: [None; 12],
            vectors: [1.0, -1.0, 0.5, 2.0],
        }
        .write(&mut writer)?;
        output.push(("face.bin", writer.data));
        Ok(output)
    }

    #[test]
    fn state_records_use_native_versions_signed_rectangles_and_float_widths() -> Result<()> {
        let records = samples()?;
        assert_eq!(
            records.iter().map(|(_, b)| b.len()).collect::<Vec<_>>(),
            [30, 49, 70, 45, 49]
        );
        assert_eq!(&records[0].1[13..21], &[254, 255, 3, 0, 16, 0, 20, 0]);
        assert_eq!(
            &records[1].1[30..],
            &[
                5, 0, 0, 0, 1, 2, 3, 0x78, 0x56, 0x34, 0x12, 4, 5, 6, 0, 0, 0, 0, 0
            ]
        );
        assert_eq!(&records[3].1[30..37], &[3, 0, 0, 0, 0, 0, 7]);
        assert_eq!(&records[4].1[33..37], &1.0f32.to_le_bytes());
        Ok(())
    }
}
