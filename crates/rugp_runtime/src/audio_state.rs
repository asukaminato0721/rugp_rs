//! Host-independent playback identities and virtual deadlines. Device output
//! reports the same identities when a real handle stops; muting supplies a
//! decoded duration instead. Source identities are wrapper objects, not PCM keys.
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Bgm,
    Effect,
    Voice,
}
struct Playing {
    source: u64,
    bus: Bus,
    started: u64,
    deadline: Option<u64>,
}
#[derive(Default)]
pub struct PlaybackRegistry {
    clock: u64,
    next: u64,
    playing: BTreeMap<u64, Playing>,
}
impl PlaybackRegistry {
    pub fn play(&mut self, source: u64, bus: Bus) -> Result<u64> {
        ensure!(self.playing.len() < 4096, "active audio budget exceeded");
        let ticket = self
            .next
            .checked_add(1)
            .context("audio identity overflow")?;
        self.next = ticket;
        if bus != Bus::Effect {
            self.stop_bus(bus);
        }
        self.playing.insert(
            ticket,
            Playing {
                source,
                bus,
                started: self.clock,
                deadline: None,
            },
        );
        Ok(ticket)
    }
    pub fn advance(&mut self, dt_ms: u32) {
        self.clock = self.clock.saturating_add(u64::from(dt_ms));
        self.playing
            .retain(|_, p| p.deadline.is_none_or(|end| end > self.clock));
    }
    /// Start time is the VM's actual play instruction, not a later drain of
    /// its output queue. A stale completion can never finish a newer instance.
    pub fn set_duration(&mut self, ticket: u64, ms: u32) {
        if let Some(p) = self.playing.get_mut(&ticket) {
            let end = p.started.saturating_add(u64::from(ms));
            p.deadline = Some(p.deadline.map_or(end, |old| old.min(end)));
        }
        self.advance(0);
    }
    pub fn finish(&mut self, ticket: u64) {
        self.playing.remove(&ticket);
    }
    pub fn finished(&self, ticket: u64) -> bool {
        !self.playing.contains_key(&ticket)
    }
    pub fn source_tickets(&self, source: u64) -> Vec<u64> {
        self.playing
            .iter()
            .filter(|(_, p)| p.source == source)
            .map(|(&id, _)| id)
            .collect()
    }
    pub fn stop_source(&mut self, source: u64) {
        self.playing.retain(|_, p| p.source != source);
    }
    pub fn fade_stop_source(&mut self, source: u64, duration_ms: u32) {
        let end = self.clock.saturating_add(u64::from(duration_ms));
        for p in self.playing.values_mut().filter(|p| p.source == source) {
            p.deadline = Some(p.deadline.map_or(end, |old| old.min(end)));
        }
        self.advance(0);
    }
    pub fn stop_bus(&mut self, bus: Bus) {
        self.playing.retain(|_, p| p.bus != bus);
    }
    pub fn stop_all(&mut self) {
        self.playing.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fade_stop_uses_existing_instances_and_survives_late_pcm_duration() -> Result<()> {
        let mut p = PlaybackRegistry::default();
        let first = p.play(7, Bus::Bgm)?;
        p.advance(2000);
        p.fade_stop_source(7, 1000);
        p.set_duration(first, 10000); // draining the original play is not a restart
        p.advance(999);
        assert!(!p.finished(first));
        p.advance(1);
        assert!(p.finished(first));
        let second = p.play(7, Bus::Bgm)?;
        p.set_duration(second, 100);
        p.fade_stop_source(7, 1000); // fade must not extend natural completion
        p.advance(100);
        assert!(p.finished(second));
        let third = p.play(7, Bus::Bgm)?;
        p.fade_stop_source(7, 1000);
        let replacement = p.play(7, Bus::Bgm)?;
        p.advance(1000);
        assert!(p.finished(third));
        assert!(!p.finished(replacement));
        p.fade_stop_source(7, 0);
        assert!(p.finished(replacement));
        Ok(())
    }
    #[test]
    fn elapsed_async_duration_exact_boundary_and_stale_completion() -> Result<()> {
        let mut p = PlaybackRegistry::default();
        let first = p.play(7, Bus::Effect)?;
        p.advance(400);
        p.set_duration(first, 1000); // queue drained after play, not a restart
        p.advance(599);
        assert!(!p.finished(first));
        p.advance(1);
        assert!(p.finished(first));
        let second = p.play(7, Bus::Effect)?;
        p.finish(first);
        assert_eq!(p.source_tickets(7), [second]);
        assert!(!p.finished(second));
        p.stop_source(7);
        assert!(p.finished(second));
        assert!(p.source_tickets(99).is_empty());
        Ok(())
    }
    #[test]
    fn buses_and_looping_instances_are_independent() -> Result<()> {
        let mut p = PlaybackRegistry::default();
        let bg = p.play(1, Bus::Bgm)?;
        let effect = p.play(2, Bus::Effect)?;
        let voice = p.play(3, Bus::Voice)?;
        p.advance(u32::MAX);
        assert!(!p.finished(bg)); // no supplied deadline: device-owned or looping
        let next_bg = p.play(4, Bus::Bgm)?;
        assert!(p.finished(bg));
        assert!(!p.finished(voice));
        p.stop_bus(Bus::Effect);
        assert!(p.finished(effect));
        assert!(!p.finished(next_bg));
        p.stop_all();
        assert!(p.finished(voice));
        assert!(p.finished(next_bg));
        Ok(())
    }
}
