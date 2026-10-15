//! CRbx metadata/masks are immutable archive resources. Preserve its mutable
//! selector state separately without rebuilding the original enabled defaults.
use super::*;
use rugp_assets::object::snapshot::ReferenceSnapshotWriter;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuSnapshot {
    version: u32,
    source: u32,
    selectors: Vec<(u32, u32, u16)>,
    hover: Option<u32>,
    pressed: Option<u32>,
}
impl MenuState {
    pub fn snapshot(&self, references: &mut ReferenceSnapshotWriter) -> Result<MenuSnapshot> {
        let source = self
            .source
            .as_ref()
            .context("cannot save menu without its original resource reference")?;
        ensure!(
            self.data.selectors.len() <= 4096,
            "menu snapshot selector count exceeds limit"
        );
        ensure!(
            self.hover.is_none_or(|key| self.result(key).is_some())
                && self.pressed.is_none_or(|key| self.result(key).is_some()),
            "invalid menu snapshot hover/press"
        );
        Ok(MenuSnapshot {
            version: 1,
            source: references.insert(source)?,
            selectors: self
                .data
                .selectors
                .iter()
                .map(|s| (s.header, s.id, s.status))
                .collect(),
            hover: self.hover,
            pressed: self.pressed,
        })
    }
    pub(crate) fn source_offset(&self) -> Option<u64> {
        self.source.as_ref().and_then(|r| r.offset)
    }
}
impl MenuSnapshot {
    pub(crate) fn source_root(&self) -> u32 {
        self.source
    }
    pub fn restore(&self, archive: &RioArchive, references: &[ObjectRef]) -> Result<MenuState> {
        ensure!(
            self.version == 1 && self.selectors.len() <= 4096,
            "invalid menu snapshot version/count"
        );
        let source = references
            .get(self.source as usize)
            .context("menu snapshot source root out of range")?;
        let mut menu = MenuState::load(archive, source)?;
        self.apply(&mut menu)?;
        Ok(menu)
    }
    fn apply(&self, menu: &mut MenuState) -> Result<()> {
        ensure!(
            self.selectors.len() == menu.data.selectors.len(),
            "menu snapshot selector count differs from resource"
        );
        for ((header, id, status), selector) in self.selectors.iter().zip(&mut menu.data.selectors)
        {
            ensure!(
                *header == selector.header && *id == selector.id,
                "menu snapshot selector identity differs from resource"
            );
            selector.status = *status;
        }
        ensure!(
            self.hover.is_none_or(|key| menu.result(key).is_some())
                && self.pressed.is_none_or(|key| menu.result(key).is_some()),
            "invalid menu snapshot hover/press"
        );
        menu.hover = self.hover;
        menu.pressed = self.pressed;
        Ok(())
    }
}
