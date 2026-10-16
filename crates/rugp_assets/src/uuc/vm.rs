//! Native VM state. MFC stack objects share the archive reference cache but
//! omit the project-object flags used by threads, scenes and timers.
use super::reference_writer::ReferenceWriter;
use super::value::NativeValue;
use crate::object::{ClassId, ObjectRef};
use anyhow::{Result, ensure};

pub struct SceneTimerState<'a> {
    pub name: &'a str,
    pub flags: u32,
    pub pending_ticks: u32,
    pub position: u32,
    pub target: u32,
    pub numerator: u32,
    pub denominator: u32,
}

impl SceneTimerState<'_> {
    /// UnivUI 1003f880, schema 1, state version 2. The loader resets pending
    /// ticks and starts its host clock after reading these fields.
    pub fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        writer.data.push(2);
        writer.string(self.name)?;
        for value in [
            self.flags,
            self.pending_ticks,
            self.position,
            self.target,
            self.numerator,
            self.denominator,
        ] {
            writer.data.extend(value.to_le_bytes());
        }
        Ok(())
    }
}

pub struct StackFrameState<'a> {
    pub context: Option<&'a ObjectRef>,
    pub scene: Option<&'a ObjectRef>,
    pub return_instruction: Option<u32>,
    pub arguments: &'a [NativeValue<'a>],
}

impl StackFrameState<'_> {
    /// Vm60 1002a1d0. A return instruction exists only when the frame has a
    /// scene. Arguments use a 16-bit count and the native tagged-value writer.
    pub fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        ensure!(
            self.scene.is_some() == self.return_instruction.is_some()
                && self
                    .return_instruction
                    .is_none_or(|id| id != 0 && id <= 0xfffff),
            "invalid UUC stack return instruction"
        );
        ensure!(
            self.arguments.len() <= u16::MAX as usize,
            "too many UUC stack arguments"
        );
        for reference in [self.context, self.scene] {
            match reference {
                Some(reference) => writer.reference(reference)?,
                None => writer.cached(0)?,
            }
        }
        if let Some(instruction) = self.return_instruction {
            writer.data.extend(instruction.to_le_bytes());
        }
        writer
            .data
            .extend((self.arguments.len() as u16).to_le_bytes());
        for argument in self.arguments {
            argument.write(writer)?;
        }
        Ok(())
    }
}

pub struct DialogueThreadState<'a> {
    pub scene: &'a ObjectRef,
    pub instruction: u32,
    pub timer: SceneTimerState<'a>,
    pub status: u32,
}

impl DialogueThreadState<'_> {
    /// CVm60Thread schema 4 at a dialogue with one empty current frame and
    /// one null stack slot. The enclosing CProcessOcean writes CStdb next.
    pub fn write(&self, writer: &mut ReferenceWriter) -> Result<()> {
        ensure!(
            self.instruction != 0 && self.instruction <= 0xfffff,
            "invalid UUC dialogue instruction"
        );
        writer.reference(self.scene)?;
        writer.data.extend(self.instruction.to_le_bytes());
        writer.inline(&class("CSceneTimer", 1), |writer| self.timer.write(writer))?;
        writer.data.extend(255u32.to_le_bytes());
        writer.mfc_object(&class("CVmStackFrame", 1), |writer| {
            StackFrameState {
                context: None,
                scene: None,
                return_instruction: None,
                arguments: &[],
            }
            .write(writer)
        })?;
        writer.cached(0)?;
        writer.data.extend(self.status.to_le_bytes());
        Ok(())
    }

    pub fn write_process(&self, writer: &mut ReferenceWriter, local_text: &str) -> Result<()> {
        writer.inline(&class("CVm60Thread", 4), |writer| self.write(writer))?;
        writer.inline(&class("CStdb", 1), |writer| writer.string(local_text))?;
        Ok(())
    }
}

fn class(name: &str, schema: u16) -> ClassId {
    ClassId {
        name: name.into(),
        schema,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Result<Vec<(&'static str, Vec<u8>)>> {
        let mut writer = ReferenceWriter::new(0)?;
        StackFrameState {
            context: None,
            scene: None,
            return_instruction: None,
            arguments: &[],
        }
        .write(&mut writer)?;
        let frame = std::mem::take(&mut writer.data);
        SceneTimerState {
            name: "",
            flags: 0x20201,
            pending_ticks: 0x11223344,
            position: 0x55667788,
            target: 0xaabbccdd,
            numerator: 7,
            denominator: 1000,
        }
        .write(&mut writer)?;
        let timer = std::mem::take(&mut writer.data);
        writer.string("")?;
        Ok(vec![
            ("stack-frame.bin", frame),
            ("scene-timer.bin", timer),
            ("stdb.bin", writer.data),
        ])
    }

    #[test]
    fn empty_frame_has_word_argument_count_and_no_project_flags() -> Result<()> {
        let mut writer = ReferenceWriter::new(0)?;
        let index = writer.mfc_object(&class("CVmStackFrame", 1), |writer| {
            writer.data.extend(&samples()?[0].1);
            Ok(())
        })?;
        assert_eq!(index, 4);
        assert_eq!(
            writer.data,
            b"\xff\xff\x01\0\x0d\0CVmStackFrame\0\0\0\0\0\0"
        );
        writer.cached(index)?;
        assert!(writer.data.ends_with(&[4, 0]));
        assert!(writer.cached(3).is_err()); // class entry
        Ok(())
    }

    #[test]
    #[ignore = "set RUGP_NATIVE_ORACLE_DIR to files generated by dev/native_uuc/probe.c"]
    fn retail_vm_state_writers_match_original_dll_serializers() -> Result<()> {
        let directory = std::path::PathBuf::from(
            std::env::var_os("RUGP_NATIVE_ORACLE_DIR")
                .ok_or_else(|| anyhow::anyhow!("native oracle directory missing"))?,
        );
        for (name, bytes) in samples()? {
            assert_eq!(bytes, std::fs::read(directory.join(name))?, "{name}");
        }
        Ok(())
    }
}
