//! CParseBgBox state, independent of its RIO scene resource. Unknown scalar
//! fields use native member offsets. All references share the UUC cache.
use super::{array::ScalarArray, reference_writer::ReferenceWriter, state::ImageAreaState};
use crate::object::ObjectRef;
use anyhow::{Result, ensure};

pub struct BackgroundLayer<'a> {
    pub source: Option<&'a ObjectRef>,
    pub corners: [[i16; 3]; 4],
    pub texture_coordinates: [[u32; 2]; 4],
    pub word_48: u32,
    pub word_4c: i32,
}

pub struct BackgroundCamera {
    pub words_8d0: [u32; 9],
    pub word_8f8: u32,
    pub shorts_8fc: [i16; 2],
    pub words_900: [u32; 5],
}

pub struct ParseBackgroundState<'a> {
    pub image: ImageAreaState<'a>,
    pub reference_858: Option<&'a ObjectRef>,
    pub layer_corners: [[[i16; 3]; 4]; 8],
    pub words_87c: [u32; 7],
    pub words_898: [u32; 7],
    pub flag_95b: u8,
    /// Outer Option is native flag +961. The inner Option is reference +178.
    pub scene_source: Option<Option<&'a ObjectRef>>,
    pub words_968: [u32; 4],
    pub references_860: [Option<&'a ObjectRef>; 2],
    pub words_870: [u32; 3],
    pub words_19c: [u32; 4],
    pub words_1ac: [u32; 2],
    pub camera: BackgroundCamera,
    pub reference_1b54: Option<&'a ObjectRef>,
    pub short_1b58: u16,
    pub flag_1b5a: u8,
    pub layers: &'a [BackgroundLayer<'a>],
    pub auxiliary: ScalarArray<'a>,
    /// Members +1bbc, +1bbd, +9f0, +1bc4.
    pub flags_1bbc: [u8; 4],
    pub short_9ec: u16,
    pub word_9e4: u32,
    /// Members +95f, +95c, +95d, +95e, +9ff.
    pub flags_95f: [u8; 5],
    pub words_8b4: [u32; 4],
    pub shorts_8c4: [i16; 3],
    pub word_8cc: u32,
    pub words_1bcc: [u32; 2],
    pub words_99c: [u32; 9],
    pub words_9c0: [u32; 9],
    pub flag_1be8: u8,
    pub word_1bfc: u32,
    /// Members +1b90, +1b8c.
    pub references_1b90: [Option<&'a ObjectRef>; 2],
    pub reference_f4: Option<&'a ObjectRef>,
}

impl ParseBackgroundState<'_> {
    /// Vm60 ordinal 146, version 16. Layer deformation uses version 2 with
    /// eight entries, each containing four signed 16-bit coordinate triples.
    pub fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        ensure!(self.layers.len() <= 4096, "too many UUC background layers");
        words(writer, &[16]);
        reference(writer, self.reference_858)?;
        writer.data.extend(2u16.to_le_bytes());
        writer.data.push(8);
        for corners in self.layer_corners {
            writer.data.push(1);
            for corner in corners {
                shorts(writer, &corner);
            }
        }
        words(writer, &self.words_87c);
        words(writer, &self.words_898);
        writer
            .data
            .extend([self.flag_95b, u8::from(self.scene_source.is_some())]);
        if let Some(source) = self.scene_source {
            reference(writer, source)?;
        }
        words(writer, &self.words_968);
        for value in self.references_860 {
            reference(writer, value)?;
        }
        words(writer, &self.words_870);
        words(writer, &self.words_19c);
        words(writer, &self.words_1ac);
        words(writer, &self.camera.words_8d0);
        writer.data.push(1);
        words(writer, &[self.camera.word_8f8]);
        shorts(writer, &self.camera.shorts_8fc);
        words(writer, &self.camera.words_900);
        reference(writer, self.reference_1b54)?;
        writer.data.extend(self.short_1b58.to_le_bytes());
        writer.data.push(self.flag_1b5a);
        writer.data.extend((self.layers.len() as u16).to_le_bytes());
        for layer in self.layers {
            reference(writer, layer.source)?;
            for (corner, coordinates) in layer.corners.iter().zip(layer.texture_coordinates) {
                shorts(writer, corner);
                words(writer, &coordinates);
            }
            words(writer, &[layer.word_48, layer.word_4c as u32]);
        }
        self.auxiliary.write(writer)?;
        writer.data.extend(self.flags_1bbc);
        writer.data.extend(self.short_9ec.to_le_bytes());
        words(writer, &[self.word_9e4]);
        writer.data.extend(self.flags_95f);
        words(writer, &self.words_8b4);
        shorts(writer, &self.shorts_8c4);
        words(writer, &[self.word_8cc]);
        words(writer, &self.words_1bcc);
        words(writer, &self.words_99c);
        words(writer, &self.words_9c0);
        writer.data.push(self.flag_1be8);
        words(writer, &[self.word_1bfc]);
        for value in self.references_1b90 {
            reference(writer, value)?;
        }
        self.image.write(writer)?;
        reference(writer, self.reference_f4)
    }
}

fn shorts(writer: &mut ReferenceWriter, values: &[i16]) {
    for value in values {
        writer.data.extend(value.to_le_bytes());
    }
}

fn words(writer: &mut ReferenceWriter, values: &[u32]) {
    for value in values {
        writer.data.extend(value.to_le_bytes());
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
    use crate::uuc::state::{BoxState, ImageBoxState};

    fn sample(patterned: bool) -> ParseBackgroundState<'static> {
        let word = |offset: u32| {
            if patterned {
                0x8000_0000 | (offset * 0x010101)
            } else {
                0
            }
        };
        let short = |offset: u16| if patterned { 0x8000 | offset } else { 0 };
        let byte = |offset: u16| if patterned { offset as u8 } else { 0 };
        ParseBackgroundState {
            image: ImageAreaState {
                image: ImageBoxState {
                    base: BoxState {
                        enabled: false,
                        flags: 0,
                        attributes: 0,
                        rectangle: [0, 0, 64, 64],
                        body_rectangle: [0; 4],
                        text: "",
                    },
                    flags: [0; 3],
                    draw_mode: 0,
                    alpha_flags: [255, 255, 0],
                    vram: None,
                    image: None,
                    overlay: None,
                },
                references: [None; 2],
                word_120: 0,
                flags: [0; 2],
                text: "",
                reference_12c: None,
                word_138: 0,
            },
            reference_858: None,
            layer_corners: std::array::from_fn(|layer| {
                std::array::from_fn(|corner| {
                    std::array::from_fn(|axis| {
                        short((0x17c + layer * 0xe0 + corner * 8 + axis * 2) as u16) as i16
                    })
                })
            }),
            words_87c: std::array::from_fn(|i| word(0x87c + 4 * i as u32)),
            words_898: std::array::from_fn(|i| word(0x898 + 4 * i as u32)),
            flag_95b: byte(0x95b),
            scene_source: None,
            words_968: std::array::from_fn(|i| word(0x968 + 4 * i as u32)),
            references_860: [None; 2],
            words_870: std::array::from_fn(|i| word(0x870 + 4 * i as u32)),
            words_19c: std::array::from_fn(|i| word(0x19c + 4 * i as u32)),
            words_1ac: std::array::from_fn(|i| word(0x1ac + 4 * i as u32)),
            camera: BackgroundCamera {
                words_8d0: std::array::from_fn(|i| word(0x8d0 + 4 * i as u32)),
                word_8f8: word(0x8f8),
                shorts_8fc: [short(0x8fc) as i16, short(0x8fe) as i16],
                words_900: std::array::from_fn(|i| word(0x900 + 4 * i as u32)),
            },
            reference_1b54: None,
            short_1b58: short(0x1b58),
            flag_1b5a: byte(0x1b5a),
            layers: &[],
            auxiliary: ScalarArray::Empty,
            flags_1bbc: [byte(0x1bbc), byte(0x1bbd), byte(0x9f0), byte(0x1bc4)],
            short_9ec: short(0x9ec),
            word_9e4: word(0x9e4),
            flags_95f: [
                byte(0x95f),
                byte(0x95c),
                byte(0x95d),
                byte(0x95e),
                byte(0x9ff),
            ],
            words_8b4: std::array::from_fn(|i| word(0x8b4 + 4 * i as u32)),
            shorts_8c4: [
                short(0x8c4) as i16,
                short(0x8c6) as i16,
                short(0x8c8) as i16,
            ],
            word_8cc: word(0x8cc),
            words_1bcc: std::array::from_fn(|i| word(0x1bcc + 4 * i as u32)),
            words_99c: std::array::from_fn(|i| word(0x99c + 4 * i as u32)),
            words_9c0: std::array::from_fn(|i| word(0x9c0 + 4 * i as u32)),
            flag_1be8: byte(0x1be8),
            word_1bfc: word(0x1bfc),
            references_1b90: [None; 2],
            reference_f4: None,
        }
    }

    #[test]
    fn empty_background_contains_all_eight_layer_entries() -> Result<()> {
        let mut writer = ReferenceWriter::new(0)?;
        sample(false).write(&mut writer)?;
        assert_eq!(writer.data.len(), 604);
        assert_eq!(&writer.data[6..9], &[2, 0, 8]);
        Ok(())
    }

}
