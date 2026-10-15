//! Immutable message/frame resources reload from the archive; retained pages
//! are independent from a currently streaming message and must survive too.
use super::*;
use rugp_assets::object::snapshot::ReferenceSnapshotWriter;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageWindowSnapshot {
    version: u32,
    source: u32,
    page: Vec<TextPart>,
}
impl MessageWindow {
    pub(crate) fn piece_bytes(&self) -> usize {
        self.pieces.iter().map(|texture| texture.rgba.len()).sum()
    }
    pub fn snapshot(
        &self,
        references: &mut ReferenceSnapshotWriter,
    ) -> Result<MessageWindowSnapshot> {
        crate::dialogue::snapshot::validate_page(&self.page)?;
        Ok(MessageWindowSnapshot {
            version: 1,
            source: references.insert(&self.source)?,
            page: self.page.clone(),
        })
    }
    pub(crate) fn source_offset(&self) -> Option<u64> {
        self.source.offset
    }
}
impl MessageWindowSnapshot {
    pub(crate) fn source_root(&self) -> u32 {
        self.source
    }
    pub(crate) fn page(&self) -> &[TextPart] {
        &self.page
    }
    pub fn restore(
        &self,
        archive: &RioArchive,
        images: &mut ImageManager,
        references: &[ObjectRef],
    ) -> Result<MessageWindow> {
        ensure!(
            self.version == 1,
            "unsupported message-window snapshot version"
        );
        crate::dialogue::snapshot::validate_page(&self.page)?;
        let source = references
            .get(self.source as usize)
            .context("message-window snapshot source root out of range")?;
        let mut window = MessageWindow::load(archive, images, source)?;
        window.page = self.page.clone();
        Ok(window)
    }
}
