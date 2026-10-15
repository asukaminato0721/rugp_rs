//! Frozen positioned glyphs, not font/cache handles or a rerun of the credit
//! parser. A resumed roll keeps exactly the layout and coverage it was using.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
};

const MAX_GLYPHS: usize = 65_536;
const MAX_BYTES: usize = 64 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StaffRollSnapshot {
    version: u32,
    window: u64,
    width: u32,
    height: u32,
    content_height: i32,
    glyphs: Vec<SavedGlyph>,
    placements: Vec<(u32, i32, i32)>,
    color: [u8; 3],
    elapsed: u32,
    duration: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedGlyph {
    advance: i32,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    alpha: Vec<u8>,
}
struct Identity(Arc<Glyph>);
impl PartialEq for Identity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Identity {}
impl Hash for Identity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

impl StaffRoll {
    pub(crate) fn snapshot(&self) -> Result<StaffRollSnapshot> {
        ensure!(
            self.glyphs.len() <= MAX_GLYPHS,
            "staff-roll snapshot placement count exceeds limit"
        );
        let mut glyphs = Vec::new();
        let mut placements = Vec::new();
        let mut identities = HashMap::new();
        let mut bytes = 0;
        for (glyph, x, y) in &self.glyphs {
            let identity = Identity(Arc::clone(glyph));
            let id = if let Some(id) = identities.get(&identity) {
                *id
            } else {
                validate_glyph(glyph.advance, glyph.width, glyph.height, glyph.alpha.len())?;
                ensure!(
                    glyph.alpha.len() <= MAX_BYTES - bytes,
                    "staff-roll snapshot masks exceed limit"
                );
                bytes += glyph.alpha.len();
                let id = glyphs.len() as u32;
                glyphs.push(SavedGlyph {
                    advance: glyph.advance,
                    left: glyph.left,
                    top: glyph.top,
                    width: glyph.width,
                    height: glyph.height,
                    alpha: glyph.alpha.clone(),
                });
                identities.insert(identity, id);
                id
            };
            placements.push((id, *x, *y));
        }
        let snapshot = StaffRollSnapshot {
            version: 1,
            window: self.window,
            width: self.width,
            height: self.height,
            content_height: self.content_height,
            glyphs,
            placements,
            color: self.color,
            elapsed: self.elapsed,
            duration: self.duration,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
}
impl StaffRollSnapshot {
    pub(crate) fn restore(&self) -> Result<StaffRoll> {
        self.validate()?;
        let glyphs = self
            .glyphs
            .iter()
            .map(|g| {
                Arc::new(Glyph {
                    advance: g.advance,
                    left: g.left,
                    top: g.top,
                    width: g.width,
                    height: g.height,
                    alpha: g.alpha.clone(),
                })
            })
            .collect::<Vec<_>>();
        Ok(StaffRoll {
            window: self.window,
            width: self.width,
            height: self.height,
            content_height: self.content_height,
            glyphs: self
                .placements
                .iter()
                .map(|(id, x, y)| (Arc::clone(&glyphs[*id as usize]), *x, *y))
                .collect(),
            color: self.color,
            elapsed: self.elapsed,
            duration: self.duration,
        })
    }
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported staff-roll snapshot version");
        ensure!(
            self.width > 0
                && self.width <= 4096
                && self.height > 0
                && self.height <= 4096
                && self.content_height > 0
                && self.content_height <= 1_000_000,
            "invalid staff-roll snapshot viewport/content"
        );
        ensure!(
            self.duration > 0 && self.elapsed <= self.duration,
            "invalid staff-roll snapshot clock"
        );
        ensure!(
            !self.placements.is_empty()
                && self.placements.len() <= MAX_GLYPHS
                && !self.glyphs.is_empty()
                && self.glyphs.len() <= self.placements.len(),
            "invalid staff-roll snapshot glyph count"
        );
        let mut bytes = 0;
        for glyph in &self.glyphs {
            validate_glyph(glyph.advance, glyph.width, glyph.height, glyph.alpha.len())?;
            ensure!(
                glyph.alpha.len() <= MAX_BYTES - bytes,
                "staff-roll snapshot masks exceed limit"
            );
            bytes += glyph.alpha.len();
        }
        let mut used = HashSet::new();
        for (id, x, y) in &self.placements {
            let glyph = self
                .glyphs
                .get(*id as usize)
                .context("staff-roll snapshot glyph index out of range")?;
            used.insert(*id);
            ensure!(
                (0..=self.width as i32).contains(x) && (0..=self.content_height).contains(y),
                "invalid staff-roll snapshot glyph position"
            );
            let left = i64::from(*x) + i64::from(glyph.left);
            let top = i64::from(*y) + i64::from(glyph.top);
            ensure!(
                i32::try_from(left).is_ok()
                    && i32::try_from(left + i64::from(glyph.width)).is_ok()
                    && i32::try_from(top - i64::from(self.content_height)).is_ok()
                    && i32::try_from(top + i64::from(self.height) + i64::from(glyph.height))
                        .is_ok(),
                "staff-roll snapshot glyph coordinates overflow"
            );
        }
        ensure!(
            used.len() == self.glyphs.len(),
            "unreachable staff-roll snapshot glyph"
        );
        Ok(())
    }
}
fn validate_glyph(advance: i32, width: u32, height: u32, bytes: usize) -> Result<()> {
    ensure!(
        (0..=4096).contains(&advance)
            && width <= 512
            && height <= 512
            && bytes == width as usize * height as usize,
        "invalid staff-roll snapshot glyph mask"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn roll() -> StaffRoll {
        let glyph = Arc::new(Glyph {
            advance: 1,
            left: 0,
            top: 0,
            width: 1,
            height: 1,
            alpha: vec![255],
        });
        StaffRoll {
            window: 1,
            width: 4,
            height: 4,
            content_height: 8,
            glyphs: vec![(Arc::clone(&glyph), 1, 0), (glyph, 2, 4)],
            color: [10, 20, 30],
            elapsed: 250,
            duration: 1000,
        }
    }
    fn wire(roll: &StaffRoll) -> StaffRollSnapshot {
        serde_json::from_slice(&serde_json::to_vec(&roll.snapshot().unwrap()).unwrap()).unwrap()
    }
    #[test]
    fn positioned_glyph_masks_sharing_and_scroll_clock_survive_without_a_font() -> Result<()> {
        let mut original = roll();
        let saved = wire(&original);
        assert_eq!(saved.glyphs.len(), 1);
        let mut restored = saved.restore()?;
        assert!(Arc::ptr_eq(&restored.glyphs[0].0, &restored.glyphs[1].0));
        assert!(!Arc::ptr_eq(&restored.glyphs[0].0, &original.glyphs[0].0));
        for dt in [0, 1, 49, 200, 499, 1, 1000] {
            assert!(original.surface()?.rgba == restored.surface()?.rgba);
            assert_eq!(original.advance(dt), restored.advance(dt));
            assert_eq!(original.finished(), restored.finished());
            assert_eq!(original.top(), restored.top());
        }
        assert!(restored.finished());
        Ok(())
    }
    #[test]
    fn zero_coverage_glyphs_and_distinct_equal_bitmaps_keep_their_own_storage() -> Result<()> {
        let mut original = roll();
        original.glyphs.push((
            Arc::new(Glyph {
                advance: 1,
                left: 0,
                top: 0,
                width: 0,
                height: 0,
                alpha: vec![],
            }),
            0,
            2,
        ));
        original.glyphs.push((
            Arc::new(Glyph {
                advance: 1,
                left: 0,
                top: 0,
                width: 1,
                height: 1,
                alpha: vec![255],
            }),
            0,
            3,
        ));
        let saved = wire(&original);
        assert_eq!(saved.glyphs.len(), 3);
        let restored = saved.restore()?;
        assert!(!Arc::ptr_eq(&restored.glyphs[0].0, &restored.glyphs[3].0));
        assert!(restored.surface()?.rgba == original.surface()?.rgba);
        Ok(())
    }
    #[test]
    fn corrupt_masks_clocks_placements_and_coordinate_overflow_fail_before_rendering() -> Result<()>
    {
        let original = roll();
        let mut bad = wire(&original);
        bad.version = 9;
        assert!(bad.restore().is_err());
        let mut bad = wire(&original);
        bad.duration = 0;
        assert!(bad.restore().is_err());
        let mut bad = wire(&original);
        bad.elapsed = 1001;
        assert!(bad.restore().is_err());
        let mut bad = wire(&original);
        bad.glyphs[0].alpha.clear();
        assert!(bad.restore().is_err());
        let mut bad = wire(&original);
        bad.placements[0].0 = 99;
        assert!(bad.restore().is_err());
        let mut bad = wire(&original);
        bad.placements[0].1 = -1;
        assert!(bad.restore().is_err());
        let mut bad = wire(&original);
        bad.glyphs[0].left = i32::MAX;
        assert!(bad.restore().is_err());
        let mut bad = wire(&original);
        bad.glyphs[0].top = i32::MIN;
        assert!(bad.restore().is_err());
        Ok(())
    }
}
