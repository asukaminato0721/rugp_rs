use super::*;

/// CWaitableTimer uses unsigned GetTickCount differences from one playback
/// epoch. Changing the interval does NOT restart the epoch (rvmm 10079510).
#[derive(Default)]
pub(super) struct Timer {
    enabled: bool,
    elapsed: u32,
    interval: u32,
}
impl Timer {
    fn running(&self) -> bool {
        self.enabled && self.elapsed < self.interval
    }
    fn play(&mut self) {
        self.enabled = true;
        self.elapsed = 0;
        self.interval = 0;
    }
    fn advance(&mut self, dt_ms: u32) {
        if self.enabled {
            self.elapsed = self.elapsed.wrapping_add(dt_ms);
        }
    }
}
impl Game {
    fn ensure_timer(&mut self, reference: &ObjectRef) -> Result<u64> {
        ensure!(
            reference
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CWaitableTimer" && c.schema == 1),
            "unsupported timer class/schema"
        );
        let id = reference.offset.context("timer identity missing")?;
        if !self.timers.contains_key(&id) {
            ensure!(
                reference.size == Some(12),
                "unsupported timer resource extent"
            );
            let data = self.archive.read_at(id, 12)?;
            let mut r = rugp_assets::binary::Reader::new(&data);
            r.skip(3)?;
            let enabled = r.u8()?;
            ensure!(enabled <= 1, "invalid timer enabled state");
            let elapsed = r.u32()?;
            let interval = r.u32()?;
            self.timers.insert(
                id,
                Timer {
                    enabled: enabled != 0,
                    elapsed,
                    interval,
                },
            );
        }
        Ok(id)
    }
    pub(super) fn timer_message(
        &mut self,
        target: &ObjectRef,
        class: &str,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        // Validate before changing the timer state.
        if class == "OM_PlayMedia" {
            ensure!(
                number(fields, "同期属性", 0)? == 0x8000 && number(fields, "ループ回数", 0)? == 0,
                "unsupported timer playback attributes"
            );
            let id = self.ensure_timer(target)?;
            self.timers.get_mut(&id).unwrap().play();
        } else {
            ensure!(class == "OM_SetTimeInterval", "unsupported timer message");
            let interval = number(fields, "時間", 0)? as u32;
            let id = self.ensure_timer(target)?;
            self.timers.get_mut(&id).unwrap().interval = interval;
        }
        Ok(())
    }
    pub(super) fn synchronize_timer(
        &mut self,
        target: &ObjectRef,
        mode: i32,
        timeout: i32,
    ) -> Result<()> {
        ensure!(
            [0, 16, 32].contains(&mode) && timeout == 0,
            "unsupported timer synchronization mode/timeout"
        );
        let id = self.ensure_timer(target)?;
        let running = self.timers[&id].running();
        // Vm60 10072c5d copies the media interface's active result. For
        // this class rvmm 10079470 returns enabled && elapsed < interval.
        if mode == 32 {
            return self.vm.acknowledge_sync_query(running);
        }
        if running {
            self.timer_wait = Some(id);
            Ok(())
        } else {
            self.vm.acknowledge_host()
        }
    }
    pub(super) fn advance_timers(&mut self, dt_ms: u32) -> Result<()> {
        for timer in self.timers.values_mut() {
            timer.advance(dt_ms);
        }
        if let Some(id) = self.timer_wait
            && !self
                .timers
                .get(&id)
                .context("waiting timer missing")?
                .running()
        {
            self.timer_wait = None;
            self.vm.acknowledge_host()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interval_changes_keep_the_play_epoch_even_after_a_completed_wait() {
        let mut timer = Timer {
            interval: 100,
            ..Timer::default()
        };
        timer.advance(99);
        assert!(!timer.running()); // unstarted timer is not active
        timer.play();
        timer.interval = 100;
        timer.advance(99);
        assert!(timer.running());
        timer.advance(1);
        assert!(!timer.running());
        timer.interval = 200;
        assert!(timer.running());
        timer.advance(99);
        assert!(timer.running());
        timer.advance(1);
        assert!(!timer.running());
        timer.interval = 150;
        assert!(!timer.running());
        timer.play();
        assert_eq!(timer.elapsed, 0);
        assert_eq!(timer.interval, 0);
        timer.interval = u32::MAX;
        timer.advance(u32::MAX);
        assert!(!timer.running());
        timer.advance(1);
        assert!(timer.running()); // same wrapping elapsed subtraction as native
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_timer_play_interval_sync_and_query_keep_absolute_clock_and_no_media_side_effects()
    -> Result<()> {
        use rugp_assets::{object::ClassId, script::Script};
        use rugp_scene_vm::{Argument, Scenario};
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let native = NativeSchema::from_directory(root.as_ref())?;
        let scene = Scenario::decode_with_schema(
            &Script::load(&game.archive, 201)?,
            game.archive.address_shift,
            Some(&native),
        )?;
        let play = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x44fac)
            .context("timer play packet")?;
        let play = game.vm.message(play)?;
        let target = play.target.clone();
        let Value::Reference(reference) = &target else {
            bail!("timer target missing");
        };
        let reference = reference.clone();
        let mut sync = scene
            .instructions
            .iter()
            .find(|i| i.address == 0x4512c)
            .context("timer sync packet")?
            .clone();
        assert!(matches!(sync.arguments[1], Argument::Number(0)));
        sync.address = 0;
        sync.next_address = 24;
        let mut program = Program::default();
        program.insert(
            0,
            Scenario {
                object_id: 1,
                memory_size: 32,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    sync,
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
        let program = Arc::new(program);
        game.vm = Vm::new(Arc::clone(&program), 1)?;
        game.message(play)?;
        let interval = |n| Message {
            class: "OM_SetTimeInterval".into(),
            target: target.clone(),
            fields: BTreeMap::from([("時間".into(), Value::Integer(n))]),
        };
        game.message(interval(500))?;
        let before = game.frame().to_vec();
        assert_eq!(game.step(0)?, RunState::Waiting);
        assert_eq!(game.step(200)?, RunState::Waiting);
        game.message(interval(400))?;
        assert_eq!(game.step(199)?, RunState::Waiting);
        assert_eq!(game.step(1)?, RunState::Ended);
        assert_eq!(game.frame(), before);
        assert!(game.pending_sounds().is_empty());
        assert!(game.video.is_none() && game.staffroll.is_none());
        // A larger threshold revives the same running epoch; query is
        // nonblocking and reports active, not the opposite completion bit.
        game.message(interval(450))?;
        game.vm = Vm::new(Arc::clone(&program), 1)?;
        assert!(matches!(game.vm.step()?, Step::Host { .. }));
        game.synchronize_timer(&reference, 32, 0)?;
        assert_eq!(game.vm.synchronization_status, Some(true));
        assert_eq!(game.timers[&reference.offset.unwrap()].elapsed, 400);
        game.timers
            .get_mut(&reference.offset.unwrap())
            .unwrap()
            .advance(50);
        game.vm = Vm::new(program, 1)?;
        assert!(matches!(game.vm.step()?, Step::Host { .. }));
        game.synchronize_timer(&reference, 32, 0)?;
        assert_eq!(game.vm.synchronization_status, Some(false));
        Ok(())
    }
}
