//! Literal-null CVmSync user confirmations, separate from clock-only sleeps.
use super::*;
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InputWait {
    pub remaining_ms: Option<u32>,
}
impl InputWait {
    pub fn new(duration: u32) -> Self {
        Self {
            remaining_ms: (!matches!(duration, 0 | u32::MAX)).then_some(duration),
        }
    }
}
impl Game {
    /// A non-text CVmSync confirmation (e.g. gallery picture/variant viewing).
    /// Text has its own `dialogue_waits_for_input` control-code state machine.
    pub fn waiting_for_input(&self) -> bool {
        self.input_wait.is_some()
    }
    pub(super) fn finish_input_wait(&mut self) -> Result<()> {
        if self.input_wait.is_some() {
            self.vm.acknowledge_host()?;
            self.input_wait = None;
            self.dialogue_pressed = false;
            self.pointer_armed = false;
        }
        Ok(())
    }
    pub(super) fn advance_input_wait(&mut self, dt: u32) -> Result<bool> {
        let Some(wait) = &mut self.input_wait else {
            return Ok(false);
        };
        if let Some(remaining) = &mut wait.remaining_ms {
            *remaining = remaining.saturating_sub(dt);
            if *remaining == 0 {
                self.finish_input_wait()?;
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugp_assets::{object::ClassId, script::Script};
    use rugp_scene_vm::{Argument, Scenario};
    fn vm(packet: &Instruction, mode: u32, timeout: u32) -> Result<Vm> {
        let mut instruction = packet.clone();
        instruction.address = 0;
        instruction.next_address = 24;
        instruction.arguments[1] = Argument::Number(mode);
        instruction.arguments[2] = Argument::Number(timeout);
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 48,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    instruction,
                    Instruction {
                        index: 1,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmRet".into(),
                            schema: 17,
                        },
                        address: 24,
                        next_address: u32::MAX,
                        flags: 0,
                        arguments: vec![],
                    },
                ],
            },
        )?;
        Vm::new(Arc::new(program), 1)
    }
    #[test]
    fn zero_and_infinite_are_confirmations_not_one_millisecond_sleeps() {
        assert!(InputWait::new(0).remaining_ms.is_none());
        assert!(InputWait::new(u32::MAX).remaining_ms.is_none());
        assert_eq!(InputWait::new(1).remaining_ms, Some(1));
        assert_eq!(InputWait::new(250).remaining_ms, Some(250));
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_null_sync_input_waits_for_fresh_input_survives_saves_and_keeps_timeout_boundaries()
    -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let scenario = Scenario::decode_with_schema(
            &Script::load(&game.archive, 239)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let packet = scenario
            .instructions
            .iter()
            .find(|i| i.class.name == "CVmSync")
            .context("original CG confirmation missing")?;
        let mut resources = crate::audio::AudioResources::new(Arc::clone(&game.archive))?;
        game.vm = vm(packet, 0, 0)?;
        assert_eq!(game.step(0)?, RunState::Ended);
        assert!(game.wait_ms.is_none() && !game.waiting_for_input());
        game.ended = false;
        game.vm = vm(packet, 0, 250)?;
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert!(!game.waiting_for_input());
        game.activate_selection()?;
        assert_eq!(game.step(249)?, RunState::Waiting);
        assert_eq!(game.step(1)?, RunState::Ended);
        for timeout in [0, u32::MAX, 250] {
            game.ended = false;
            game.vm = vm(packet, 16, timeout)?;
            assert_eq!(game.step(0)?, RunState::Waiting);
            assert!(game.wait_ms.is_none() && game.waiting_for_input());
            game.step(100)?;
            game.pointer_move(0, 0)?;
            game.pointer_button(true)?;
            let saved = game.snapshot()?;
            let mut wire = Vec::new();
            saved.write(&mut wire)?;
            let saved = super::super::snapshot::GameSnapshot::read(wire.as_slice())?;
            game.step(30)?;
            game.restore_snapshot(&saved, &mut resources)?;
            game.pointer_button(false)?;
            assert!(game.waiting_for_input());
            assert_eq!(game.vm.cursor().unwrap().address, 0);
            if timeout == 250 {
                assert_eq!(game.input_wait.as_ref().unwrap().remaining_ms, Some(150));
                assert_eq!(game.step(149)?, RunState::Waiting);
                assert_eq!(game.step(1)?, RunState::Ended);
                continue;
            }
            assert_eq!(game.step(100000)?, RunState::Waiting);
            assert_eq!(game.vm.cursor().unwrap().address, 0);
            game.pointer_move(0, 0)?;
            game.pointer_button(true)?;
            game.pointer_move(-1, -1)?;
            game.pointer_button(false)?;
            assert!(game.waiting_for_input());
            if timeout == 0 {
                game.pointer_move(0, 0)?;
                game.pointer_button(true)?;
                game.pointer_button(false)?;
            } else {
                game.activate_selection()?;
            }
            assert!(!game.waiting_for_input());
            assert_eq!(game.vm.cursor().unwrap().address, 24);
            assert_eq!(game.step(0)?, RunState::Ended);
        }
        Ok(())
    }
}
