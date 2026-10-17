//! Native message-window checkpoint state. A dialogue checkpoint has no active
//! selector list. Resource definitions and GDI font caches are not state bytes.
use super::{reference_writer::ReferenceWriter, state::FrameBoxState};
use crate::object::ObjectRef;
use anyhow::{Result, ensure};

pub struct FontState<'a> {
    pub attributes: Option<&'a ObjectRef>,
    pub flags: u16,
    /// Native members +4 and +8, serialized as two 32-bit words.
    pub parameters: [u32; 2],
}

impl FontState<'_> {
    /// UnivUI ordinal 701.
    fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        reference(writer, self.attributes)?;
        writer.data.extend(self.flags.to_le_bytes());
        for value in self.parameters {
            writer.data.extend(value.to_le_bytes());
        }
        Ok(())
    }
}

pub struct MessageWindowState<'a> {
    pub flag_1cc: u8,
    pub references: [Option<&'a ObjectRef>; 2],
    pub frame: FrameBoxState<'a>,
    pub short_18e: i16,
    pub word_190: u32,
    pub word_194: u32,
    pub flag_22b: u8,
    pub word_170: u32,
    pub font: FontState<'a>,
    /// UnivUI ordinal 1012 stores a map of 16-bit keys and CStrings.
    pub strings: &'a [(u16, &'a str)],
    pub word_1ac4: i32,
}

impl MessageWindowState<'_> {
    /// Vm60 1000e6c0, prefix version 2 and body version 7.
    pub fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        ensure!(self.strings.len() <= 4096, "too many UUC message strings");
        let mut keys = std::collections::BTreeSet::new();
        ensure!(
            self.strings.iter().all(|(key, _)| keys.insert(*key)),
            "duplicate UUC message string key"
        );
        writer.data.extend([2, self.flag_1cc]);
        for value in self.references {
            reference(writer, value)?;
        }
        self.frame.write(writer)?;
        writer.data.extend(7u32.to_le_bytes());
        writer.data.extend(self.short_18e.to_le_bytes());
        writer.data.extend(self.word_190.to_le_bytes());
        writer.data.extend(self.word_194.to_le_bytes());
        writer.data.extend([self.flag_22b, 0]);
        writer.data.extend(self.word_170.to_le_bytes());
        self.font.write(writer)?;
        // Bounded count fits the short form of CArchive::WriteCount.
        writer
            .data
            .extend((self.strings.len() as u16).to_le_bytes());
        for (key, value) in self.strings {
            writer.data.extend(key.to_le_bytes());
            writer.string(value)?;
        }
        writer.data.extend(self.word_1ac4.to_le_bytes());
        Ok(())
    }

    /// CMsgSelBox wrapper at a dialogue, with no active CListExAssoc objects.
    pub fn write_dialogue(&self, writer: &mut ReferenceWriter) -> Result<()> {
        self.write(writer)?;
        writer.data.extend(0u16.to_le_bytes());
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
    use crate::uuc::state::BoxState;

    fn sample() -> MessageWindowState<'static> {
        MessageWindowState {
            flag_1cc: 0x5a,
            references: [None; 2],
            frame: FrameBoxState {
                base: BoxState {
                    enabled: true,
                    flags: 0x1234,
                    attributes: 0x5678,
                    rectangle: [-2, 3, 16, 20],
                    body_rectangle: [-4, 5, 30, 40],
                    text: "",
                },
                frame: None,
                mode: 7,
                rectangle: [-4, 5, 30, 40],
            },
            short_18e: 0x8123u16 as i16,
            word_190: 0x11223344,
            word_194: 0x55667788,
            flag_22b: 0xa5,
            word_170: 0x99aabbcc,
            font: FontState {
                attributes: None,
                flags: 0x3456,
                parameters: [0x12345678, 0xabcdef01],
            },
            strings: &[],
            word_1ac4: 0x87654321u32 as i32,
        }
    }

    #[test]
    fn dialogue_has_empty_selector_count_and_rejects_duplicate_string_keys() -> Result<()> {
        let mut state = sample();
        let mut writer = ReferenceWriter::new(0)?;
        state.write_dialogue(&mut writer)?;
        assert_eq!(writer.data.len(), 91);
        assert_eq!(&writer.data[89..], &[0, 0]);
        state.strings = &[(7, "a"), (7, "b")];
        let mut writer = ReferenceWriter::new(0)?;
        assert!(state.write(&mut writer).is_err());
        assert!(writer.data.is_empty());
        Ok(())
    }
}
