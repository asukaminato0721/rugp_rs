//! Character state uses a different order from the resource body. Field names
//! with hexadecimal suffixes identify native member offsets whose meaning is
//! not yet established. Words retain their bits, including Q16 coordinates.
use super::{reference_writer::ReferenceWriter, state::ImageBoxState};
use crate::object::ObjectRef;
use anyhow::Result;

pub struct BlinkState {
    pub limits: [i16; 2],
    pub parameters: [u32; 2],
}

impl BlinkState {
    fn write(&self, writer: &mut ReferenceWriter) {
        writer.data.push(0);
        for value in self.limits {
            writer.data.extend(value.to_le_bytes());
        }
        words(writer, &self.parameters);
    }
}

pub struct CharacterBoxState<'a> {
    pub image: ImageBoxState<'a>,
    /// Members +124, +120, in archive order.
    pub styles: [i32; 2],
    pub body: Option<&'a ObjectRef>,
    /// Members +294, +295, +297, +338, +339.
    pub face_flags: [u8; 5],
    pub limits_340: [i16; 2],
    pub word_344: u32,
    pub word_350: u32,
    /// Members +14c, +290, +f8.
    pub expression: [Option<&'a ObjectRef>; 3],
    pub position_q16: [i32; 2],
    pub word_2fc: u32,
    pub target_q16: [i32; 2],
    pub word_30c: u32,
    pub words_310: [u32; 2],
    pub word_318: u32,
    pub word_324: u32,
    pub blink: BlinkState,
    /// Members +2a4, +2b0, +2e4, +2e8.
    pub animations: [Option<&'a ObjectRef>; 4],
    pub word_2f8: i32,
    pub flags_28c: [u8; 4],
    pub short_33c: u16,
    pub flags_359: [u8; 3],
    pub words_35c: [u32; 2],
    pub words_364: [u32; 2],
    pub words_36c: [u32; 4],
    pub words_164: [u32; 3],
    pub word_280: i32,
    pub flag_1fc: u8,
    pub flag_33a: u8,
    pub reference_37c: Option<&'a ObjectRef>,
    pub words_188: [u32; 3],
    pub words_194: [u32; 3],
    pub words_1ac: [u32; 3],
    pub words_208: [u32; 18],
    pub words_3d4: [u32; 3],
    pub words_3e0: [u32; 2],
    pub words_3f8: [u32; 3],
    pub word_428: u32,
    pub flag_42f: u8,
    pub word_430: u32,
    pub word_288: u32,
    pub word_434: u32,
    pub words_438: [u32; 3],
    pub words_444: [u32; 3],
    pub flag_450: u8,
    /// Native flag +451 controls the two vectors at +454 and +460.
    pub pose: Option<[[u32; 3]; 2]>,
    pub word_46c: i32,
    pub reference_470: Option<&'a ObjectRef>,
    pub reference_f4: Option<&'a ObjectRef>,
}

impl CharacterBoxState<'_> {
    /// Vm60 10030030, version 12. CCharBox2 writes its face state first.
    pub fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        words(writer, &[12, self.styles[0] as u32, self.styles[1] as u32]);
        reference(writer, self.body)?;
        writer.data.extend(self.face_flags);
        for value in self.limits_340 {
            writer.data.extend(value.to_le_bytes());
        }
        words(writer, &[self.word_344, self.word_350]);
        for value in self.expression {
            reference(writer, value)?;
        }
        words(writer, &self.position_q16.map(|value| value as u32));
        words(writer, &[self.word_2fc]);
        words(writer, &self.target_q16.map(|value| value as u32));
        words(writer, &[self.word_30c]);
        words(writer, &self.words_310);
        words(writer, &[self.word_318, self.word_324]);
        self.blink.write(writer);
        for value in self.animations {
            reference(writer, value)?;
        }
        words(writer, &[self.word_2f8 as u32, 0]);
        writer.data.extend(self.flags_28c);
        writer.data.extend(self.short_33c.to_le_bytes());
        writer.data.extend(self.flags_359);
        for values in [
            self.words_35c.as_slice(),
            &self.words_364,
            &self.words_36c,
            &self.words_164,
        ] {
            words(writer, values);
        }
        words(writer, &[self.word_280 as u32]);
        writer.data.extend([self.flag_1fc, self.flag_33a]);
        reference(writer, self.reference_37c)?;
        for values in [
            self.words_188.as_slice(),
            &self.words_194,
            &self.words_1ac,
            &self.words_208,
            &self.words_3d4,
            &self.words_3e0,
            &self.words_3f8,
        ] {
            words(writer, values);
        }
        words(writer, &[self.word_428]);
        writer.data.push(self.flag_42f);
        words(writer, &[self.word_430, self.word_288, self.word_434]);
        words(writer, &self.words_438);
        words(writer, &self.words_444);
        writer
            .data
            .extend([self.flag_450, u8::from(self.pose.is_some())]);
        words(writer, &[self.word_46c as u32]);
        if let Some(vectors) = &self.pose {
            for vector in vectors {
                words(writer, vector);
            }
        }
        reference(writer, self.reference_470)?;
        self.image.write(writer)?;
        reference(writer, self.reference_f4)
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
    use crate::uuc::state::BoxState;

    fn word(offset: u32) -> u32 {
        0x8000_0000 | (offset * 0x010101)
    }

    fn words_at<const N: usize>(offset: u32) -> [u32; N] {
        std::array::from_fn(|i| word(offset + 4 * i as u32))
    }

    fn state() -> CharacterBoxState<'static> {
        CharacterBoxState {
            image: ImageBoxState {
                base: BoxState {
                    enabled: true,
                    flags: 0x1234,
                    attributes: 0x5678,
                    rectangle: [-2, 3, 16, 20],
                    body_rectangle: [-4, 5, 30, 40],
                    text: "",
                },
                flags: [1, 2, 3],
                draw_mode: 0x12345678,
                alpha_flags: [4, 5, 6],
                vram: None,
                image: None,
                overlay: None,
            },
            styles: [word(0x124) as i32, word(0x120) as i32],
            body: None,
            face_flags: [0x94, 0x95, 0x97, 0x38, 0x39],
            limits_340: [0x8340u16 as i16, 0x8342u16 as i16],
            word_344: word(0x344),
            word_350: word(0x350),
            expression: [None; 3],
            position_q16: words_at(0x17c).map(|value| value as i32),
            word_2fc: word(0x2fc),
            target_q16: words_at(0x300).map(|value| value as i32),
            word_30c: word(0x30c),
            words_310: words_at(0x310),
            word_318: word(0x318),
            word_324: word(0x324),
            blink: BlinkState {
                limits: [0x8298u16 as i16, 0x829au16 as i16],
                parameters: words_at(0x29c),
            },
            animations: [None; 4],
            word_2f8: word(0x2f8) as i32,
            flags_28c: [0x8c, 0x8d, 0x8e, 0x8f],
            short_33c: 0x833c,
            flags_359: [0x59, 0x5a, 0x5b],
            words_35c: words_at(0x35c),
            words_364: words_at(0x364),
            words_36c: words_at(0x36c),
            words_164: words_at(0x164),
            word_280: word(0x280) as i32,
            flag_1fc: 0xfc,
            flag_33a: 0x3a,
            reference_37c: None,
            words_188: words_at(0x188),
            words_194: words_at(0x194),
            words_1ac: words_at(0x1ac),
            words_208: words_at(0x208),
            words_3d4: words_at(0x3d4),
            words_3e0: words_at(0x3e0),
            words_3f8: words_at(0x3f8),
            word_428: word(0x428),
            flag_42f: 0x2f,
            word_430: word(0x430),
            word_288: word(0x288),
            word_434: word(0x434),
            words_438: words_at(0x438),
            words_444: words_at(0x444),
            flag_450: 0x50,
            pose: None,
            word_46c: word(0x46c) as i32,
            reference_470: None,
            reference_f4: None,
        }
    }

    #[test]
    fn conditional_pose_writes_two_vectors_and_moves_following_fields() -> Result<()> {
        let mut state = state();
        let mut plain = ReferenceWriter::new(0)?;
        state.write(&mut plain)?;
        state.pose = Some([words_at(0x454), words_at(0x460)]);
        let mut pose = ReferenceWriter::new(0)?;
        state.write(&mut pose)?;
        assert_eq!(plain.data.len(), 407);
        assert_eq!(pose.data.len(), plain.data.len() + 24);
        assert_eq!(&plain.data[..12], &pose.data[..12]);
        assert_eq!(
            &plain.data[plain.data.len() - 51..],
            &pose.data[pose.data.len() - 51..]
        );
        Ok(())
    }
}
