//! Logical audio snapshots and output reconstruction, independent of a device.
//! Capture these in the same reference graph as the VM. Restoring creates fresh
//! tickets above BOTH saved and live high-water marks, so pre-load completion
//! callbacks cannot finish restored sounds. The host must reset old device
//! playback and rebuild its runtime/device ticket map from `playbacks`.
use super::{Bus, FadeStop, GainMotion, PlaybackRegistry, Playing};
use crate::audio_envelope::GainEnvelope;
use anyhow::{Context, Result, ensure};
use rugp_assets::object::{ObjectRef, snapshot::ReferenceSnapshotWriter};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybackSnapshot {
    version: u32,
    clock: u64,
    next: u64,
    playing: Vec<SavedPlaying>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPlaying {
    ticket: u64,
    source: u32,
    bus: Bus,
    started: u64,
    deadline: Option<u64>,
    gain: SavedGain,
    fade_stop: Option<SavedFade>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedGain {
    parameters: [u16; 3],
    started: u64,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedFade {
    started: u64,
    duration_ms: u32,
}

/// Resume gain from its current value, not from the original play envelope.
/// Fade-stop keeps its own phase, separate from ordinary gain motion. Device
/// buffers/tween quantization are presentation approximations, not VM clocks.
#[derive(Debug, Clone)]
pub struct PlaybackResume {
    pub ticket: u64,
    pub source: ObjectRef,
    pub bus: Bus,
    pub elapsed_ms: u64,
    pub envelope: GainEnvelope,
    pub fade_stop: Option<FadeResume>,
}
#[derive(Debug, Clone, Copy)]
pub struct FadeResume {
    pub elapsed_ms: u32,
    pub duration_ms: u32,
    pub remaining_ms: u32,
}
impl PlaybackResume {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.ticket > 0, "invalid resumed audio ticket");
        validate_source(&self.source, self.bus)?;
        if let Some(fade) = self.fade_stop {
            ensure!(
                fade.duration_ms > 0
                    && fade.elapsed_ms < fade.duration_ms
                    && fade.remaining_ms > 0
                    && fade.remaining_ms <= fade.duration_ms - fade.elapsed_ms,
                "invalid resumed fade phase"
            );
        }
        Ok(())
    }
}
pub struct PlaybackRestore {
    pub registry: PlaybackRegistry,
    pub playbacks: Vec<PlaybackResume>,
    tickets: BTreeMap<u64, u64>,
    saved_next: u64,
}
impl PlaybackRestore {
    /// Resolve one-shot deadlines on the restored original epoch. Any decoder
    /// error affects this candidate only; the live registry is not modified.
    pub fn prepare_muted(&mut self, resources: &mut crate::audio::AudioResources) -> Result<()> {
        for playback in &self.playbacks {
            if let Some(duration) = resources.muted_duration(&playback.source)? {
                self.registry.set_duration(playback.ticket, duration);
            }
        }
        self.playbacks.retain(|p| !self.registry.finished(p.ticket));
        Ok(())
    }
    /// Finished saved tickets remain finished. No new allocation can reuse
    /// their old numbers because restored tickets begin above the high-water.
    pub fn remap_ticket(&self, ticket: u64) -> Result<u64> {
        ensure!(
            ticket > 0 && ticket <= self.saved_next,
            "invalid saved audio wait ticket"
        );
        Ok(self.tickets.get(&ticket).copied().unwrap_or(ticket))
    }
    pub fn remap_wait(&self, tickets: &[u64]) -> Result<Vec<u64>> {
        ensure!(tickets.len() <= 4096, "saved audio wait exceeds limit");
        tickets.iter().map(|t| self.remap_ticket(*t)).collect()
    }
}

impl PlaybackRegistry {
    pub fn snapshot(&self, references: &mut ReferenceSnapshotWriter) -> Result<PlaybackSnapshot> {
        let playing = self
            .playing
            .iter()
            .map(|(ticket, p)| {
                let source = p
                    .source_ref
                    .as_ref()
                    .context("cannot save audio without its original source reference")?;
                validate_source(source, p.bus)?;
                Ok(SavedPlaying {
                    ticket: *ticket,
                    source: references.insert(source)?,
                    bus: p.bus,
                    started: p.started,
                    deadline: p.deadline,
                    gain: SavedGain {
                        parameters: p.gain.envelope.native_parameters(),
                        started: p.gain.started,
                    },
                    fade_stop: p.fade_stop.as_ref().map(|f| SavedFade {
                        started: f.started,
                        duration_ms: f.duration_ms,
                    }),
                })
            })
            .collect::<Result<_>>()?;
        let snapshot = PlaybackSnapshot {
            version: 1,
            clock: self.clock,
            next: self.next,
            playing,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
}
impl PlaybackSnapshot {
    pub fn restore(
        &self,
        references: &[ObjectRef],
        live: &PlaybackRegistry,
    ) -> Result<PlaybackRestore> {
        self.validate()?;
        let floor = self.next.max(live.next);
        floor
            .checked_add(self.playing.len() as u64)
            .context("restored audio identity overflow")?;
        let mut result = PlaybackRestore {
            registry: PlaybackRegistry {
                clock: self.clock,
                next: floor,
                playing: BTreeMap::new(),
            },
            tickets: BTreeMap::new(),
            playbacks: Vec::new(),
            saved_next: self.next,
        };
        for saved in &self.playing {
            let source = references
                .get(saved.source as usize)
                .context("audio snapshot source root out of range")?;
            let source_id = validate_source(source, saved.bus)?;
            let ticket = result.registry.next + 1;
            result.registry.next = ticket;
            let gain = GainMotion {
                envelope: GainEnvelope::from_native(saved.gain.parameters)?,
                started: saved.gain.started,
            };
            let fade_stop = saved.fade_stop.as_ref().map(|f| FadeStop {
                started: f.started,
                duration_ms: f.duration_ms,
            });
            let fade_resume = fade_stop.as_ref().map(|f| FadeResume {
                elapsed_ms: (self.clock - f.started) as u32,
                duration_ms: f.duration_ms,
                remaining_ms: (f.started.saturating_add(u64::from(f.duration_ms)) - self.clock)
                    .min(saved.deadline.unwrap() - self.clock) as u32,
            });
            result.playbacks.push(PlaybackResume {
                ticket,
                source: source.clone(),
                bus: saved.bus,
                elapsed_ms: self.clock - saved.started,
                envelope: gain.remaining(self.clock),
                fade_stop: fade_resume,
            });
            result.registry.playing.insert(
                ticket,
                Playing {
                    source: source_id,
                    source_ref: Some(source.clone()),
                    bus: saved.bus,
                    started: saved.started,
                    deadline: saved.deadline,
                    gain,
                    fade_stop,
                },
            );
            result.tickets.insert(saved.ticket, ticket);
        }
        Ok(result)
    }
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported audio snapshot version");
        ensure!(
            self.playing.len() <= 4096,
            "audio snapshot exceeds playback limit"
        );
        let mut tickets = HashSet::new();
        let mut roots = HashSet::new();
        let mut bgm = false;
        let mut voice = false;
        for p in &self.playing {
            ensure!(
                p.ticket > 0 && p.ticket <= self.next && tickets.insert(p.ticket),
                "invalid or duplicate audio snapshot ticket"
            );
            // Each capture inserts a distinct root, even when the underlying
            // source aliases. Reusing a large root must not bypass the shared
            // reference graph's expanded-memory budget during clone/restore.
            ensure!(roots.insert(p.source), "reused audio snapshot source root");
            ensure!(
                p.started <= self.clock
                    && p.gain.started >= p.started
                    && p.gain.started <= self.clock,
                "invalid audio snapshot start clock"
            );
            ensure!(
                p.deadline.is_none_or(|end| end > self.clock),
                "expired audio snapshot playback"
            );
            GainEnvelope::from_native(p.gain.parameters)?;
            if let Some(fade) = &p.fade_stop {
                let end = fade.started.saturating_add(u64::from(fade.duration_ms));
                ensure!(
                    fade.started >= p.started && fade.started <= self.clock && end > self.clock,
                    "invalid audio snapshot fade phase"
                );
                ensure!(
                    p.deadline.is_some_and(|deadline| deadline <= end),
                    "audio snapshot fade has no completion deadline"
                );
            }
            match p.bus {
                Bus::Bgm => {
                    ensure!(!bgm, "duplicate audio snapshot BGM bus");
                    bgm = true;
                }
                Bus::Voice => {
                    ensure!(!voice, "duplicate audio snapshot voice bus");
                    voice = true;
                }
                Bus::Effect => {}
            }
        }
        Ok(())
    }
}
fn validate_source(source: &ObjectRef, bus: Bus) -> Result<u64> {
    ensure!(
        source
            .class
            .as_ref()
            .is_some_and(|c| ["CPmBgm", "CPmEffect", "COptimizedObs"].contains(&c.name.as_str()))
            && source.size.is_some()
            && source.inline.is_none(),
        "unsupported audio snapshot source"
    );
    ensure!(
        Bus::for_source(source) == bus,
        "audio snapshot source bus mismatch"
    );
    source
        .offset
        .context("audio snapshot source offset missing")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugp_assets::object::{ClassId, snapshot::ReferenceSnapshot};
    pub(super) fn source(class: &str, offset: u64) -> ObjectRef {
        ObjectRef {
            class: Some(ClassId {
                name: class.into(),
                schema: 1,
            }),
            cache_index: None,
            flags: 0xc108,
            offset: Some(offset),
            size: Some(7),
            owner: None,
            selector: 0,
            inline: None,
        }
    }
    fn capture(registry: &PlaybackRegistry) -> (PlaybackSnapshot, Vec<ObjectRef>) {
        let mut writer = ReferenceSnapshotWriter::default();
        let snapshot = registry.snapshot(&mut writer).unwrap();
        let snapshot: PlaybackSnapshot =
            serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let references: ReferenceSnapshot =
            serde_json::from_slice(&serde_json::to_vec(&writer.finish().unwrap()).unwrap())
                .unwrap();
        (snapshot, references.restore().unwrap())
    }
    fn copy(snapshot: &PlaybackSnapshot) -> PlaybackSnapshot {
        serde_json::from_slice(&serde_json::to_vec(snapshot).unwrap()).unwrap()
    }
    #[test]
    fn resume_restores_epoch_gain_and_exact_remaining_deadline_with_fresh_tickets() -> Result<()> {
        let mut live = PlaybackRegistry::default();
        let original = live.play_sound(
            &source("COptimizedObs", 7),
            GainEnvelope::from_native([0, 4096, 1000])?,
        )?;
        live.set_duration(original, 2000);
        live.advance(400);
        let (snapshot, references) = capture(&live);
        let future = live.play_sound(&source("COptimizedObs", 8), GainEnvelope::UNITY)?;
        let mut restored = snapshot.restore(&references, &live)?;
        let ticket = restored.remap_ticket(original)?;
        assert!(ticket > future);
        assert_eq!(restored.registry.clock, 400);
        assert_eq!(restored.playbacks[0].elapsed_ms, 400);
        assert_eq!(restored.playbacks[0].source.offset, Some(7));
        assert_eq!(
            restored.playbacks[0].envelope.native_parameters(),
            [1638, 4096, 600]
        );
        restored.registry.finish(original);
        restored.registry.finish(future);
        assert!(!restored.registry.finished(ticket));
        // A late decode of the original full PCM duration is NOT a restart.
        restored.registry.set_duration(ticket, 2000);
        restored.registry.advance(1599);
        assert!(!restored.registry.finished(ticket));
        restored.registry.advance(1);
        assert!(restored.registry.finished(ticket));
        assert_eq!(live.source_tickets(8), [future]);
        Ok(())
    }
    #[test]
    fn independent_buses_effect_overlap_and_completed_waits_keep_their_meaning() -> Result<()> {
        let mut live = PlaybackRegistry::default();
        let completed = live.play_sound(&source("CPmEffect", 1), GainEnvelope::UNITY)?;
        live.finish(completed);
        let bgm = live.play_sound(&source("CPmBgm", 2), GainEnvelope::UNITY)?;
        let voice = live.play_sound(&source("COptimizedObs", 3), GainEnvelope::UNITY)?;
        let effect1 = live.play_sound(&source("CPmEffect", 4), GainEnvelope::UNITY)?;
        let effect2 = live.play_sound(&source("CPmEffect", 4), GainEnvelope::UNITY)?;
        live.advance(12345);
        let (saved, refs) = capture(&live);
        let mut result = saved.restore(&refs, &live)?;
        assert_eq!(result.playbacks.len(), 4);
        assert_eq!(result.registry.source_tickets(4).len(), 2);
        assert_eq!(result.remap_ticket(completed)?, completed);
        assert!(result.registry.finished(completed));
        let wait = result.remap_wait(&[completed, effect1, effect2])?;
        assert_eq!(wait.len(), 3);
        result.registry.stop_bus(Bus::Effect);
        assert!(wait.iter().all(|t| result.registry.finished(*t)));
        assert!(!result.registry.finished(result.remap_ticket(bgm)?));
        assert!(!result.registry.finished(result.remap_ticket(voice)?));
        assert!(result.remap_ticket(0).is_err());
        assert!(result.remap_ticket(saved.next + 1).is_err());
        Ok(())
    }
    #[test]
    fn volume_changes_keep_pcm_epoch_and_zero_duration_uses_the_first_word() -> Result<()> {
        let mut live = PlaybackRegistry::default();
        let ticket = live.play_sound(
            &source("CPmEffect", 1),
            GainEnvelope::from_native([0, 4096, 1000])?,
        )?;
        live.advance(500);
        live.set_source_gain(1, GainEnvelope::from_native([0, 0, 1000])?);
        live.advance(250);
        let (saved, refs) = capture(&live);
        let restored = saved.restore(&refs, &live)?;
        let playback = &restored.playbacks[0];
        assert_eq!(playback.elapsed_ms, 750);
        assert_eq!(playback.envelope.native_parameters(), [1536, 0, 750]);
        assert_eq!(
            restored.registry.source_tickets(1),
            [restored.remap_ticket(ticket)?]
        );
        live.set_source_gain(1, GainEnvelope::from_native([1024, 4096, 0])?);
        live.advance(2000);
        let (saved, refs) = capture(&live);
        let restored = saved.restore(&refs, &live)?;
        assert_eq!(
            restored.playbacks[0].envelope.native_parameters(),
            [1024, 1024, 0]
        );
        assert!(!restored.registry.finished(restored.remap_ticket(ticket)?));
        Ok(())
    }
    #[test]
    fn fade_stop_retains_its_phase_and_does_not_extend_a_shorter_pcm_tail() -> Result<()> {
        let mut live = PlaybackRegistry::default();
        let ticket = live.play_sound(&source("CPmBgm", 1), GainEnvelope::UNITY)?;
        live.set_duration(ticket, 1500);
        live.advance(1000);
        live.fade_stop_source(1, 1000);
        live.advance(200);
        let (saved, refs) = capture(&live);
        let mut restored = saved.restore(&refs, &live)?;
        let fade = restored.playbacks[0].fade_stop.unwrap();
        assert_eq!(
            (fade.elapsed_ms, fade.duration_ms, fade.remaining_ms),
            (200, 1000, 300)
        );
        assert_eq!(restored.playbacks[0].elapsed_ms, 1200);
        restored.registry.advance(299);
        assert!(!restored.registry.finished(restored.remap_ticket(ticket)?));
        restored.registry.advance(1);
        assert!(restored.registry.finished(restored.remap_ticket(ticket)?));
        Ok(())
    }
    #[test]
    fn malformed_clocks_ids_gains_sources_and_buses_are_rejected_without_touching_live()
    -> Result<()> {
        let mut live = PlaybackRegistry::default();
        let ticket = live.play_sound(&source("CPmBgm", 1), GainEnvelope::UNITY)?;
        let (saved, refs) = capture(&live);
        let mut bad = copy(&saved);
        bad.version = 9;
        assert!(bad.restore(&refs, &live).is_err());
        let mut bad = copy(&saved);
        bad.playing[0].ticket = 0;
        assert!(bad.restore(&refs, &live).is_err());
        let mut bad = copy(&saved);
        bad.playing[0].ticket += 1;
        assert!(bad.restore(&refs, &live).is_err());
        let mut bad = copy(&saved);
        bad.playing[0].started = 1;
        assert!(bad.restore(&refs, &live).is_err());
        let mut bad = copy(&saved);
        bad.playing[0].gain.started = 1;
        assert!(bad.restore(&refs, &live).is_err());
        let mut bad = copy(&saved);
        bad.playing[0].gain.parameters[0] = u16::MAX;
        assert!(bad.restore(&refs, &live).is_err());
        let mut bad = copy(&saved);
        bad.playing[0].deadline = Some(0);
        assert!(bad.restore(&refs, &live).is_err());
        let mut bad = copy(&saved);
        bad.playing[0].source = 999;
        assert!(bad.restore(&refs, &live).is_err());
        let mut bad = copy(&saved);
        bad.playing[0].bus = Bus::Effect;
        assert!(bad.restore(&refs, &live).is_err());
        let mut bad = copy(&saved);
        bad.playing[0].fade_stop = Some(SavedFade {
            started: 0,
            duration_ms: 1000,
        });
        assert!(bad.restore(&refs, &live).is_err());
        let mut wrong_refs = refs.clone();
        wrong_refs[0].class.as_mut().unwrap().name = "CInt".into();
        assert!(saved.restore(&wrong_refs, &live).is_err());
        assert_eq!(live.source_tickets(1), [ticket]);
        Ok(())
    }
    #[test]
    fn unbound_legacy_playbacks_expansion_aliases_and_ticket_overflow_fail_explicitly() -> Result<()>
    {
        let mut legacy = PlaybackRegistry::default();
        legacy.play(1, Bus::Effect)?;
        assert!(
            legacy
                .snapshot(&mut ReferenceSnapshotWriter::default())
                .is_err()
        );
        let mut live = PlaybackRegistry::default();
        live.play_sound(&source("CPmEffect", 1), GainEnvelope::UNITY)?;
        live.play_sound(&source("CPmEffect", 1), GainEnvelope::UNITY)?;
        let (saved, refs) = capture(&live);
        let mut bad = copy(&saved);
        bad.playing[1].source = bad.playing[0].source;
        assert!(bad.restore(&refs, &live).is_err());
        let mut bad = copy(&saved);
        bad.playing[1].ticket = bad.playing[0].ticket;
        assert!(bad.restore(&refs, &live).is_err());
        live.next = u64::MAX;
        assert!(saved.restore(&refs, &live).is_err());
        assert_eq!(live.playing.len(), 2);
        let empty = PlaybackRegistry::default();
        let (saved, refs) = capture(&empty);
        let restored = saved.restore(&refs, &live)?;
        assert_eq!(restored.registry.next, u64::MAX);
        assert!(restored.playbacks.is_empty());
        Ok(())
    }
}
