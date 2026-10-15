use crate::render::Texture;
use anyhow::{Context, Result, ensure};
use rugp_assets::{
    RioArchive,
    background::Background,
    crmt::Crmt,
    image::decode_r6ti,
    object::ObjectRef,
    rip::{decode_rip007, decode_rip008},
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};
pub struct ImageManager {
    archive: Arc<RioArchive>,
    cache: BTreeMap<u64, Arc<Texture>>,
    ages: VecDeque<u64>,
    bytes: usize,
    budget: usize,
}
impl ImageManager {
    pub fn new(archive: Arc<RioArchive>) -> Self {
        Self {
            archive,
            cache: BTreeMap::new(),
            ages: VecDeque::new(),
            bytes: 0,
            budget: 128 * 1024 * 1024,
        }
    }
    pub fn load(&mut self, reference: &ObjectRef) -> Result<Arc<Texture>> {
        if let Some(inline) = &reference.inline {
            return Ok(Arc::new(Texture::from_image(inline.decode_bitmap()?)?));
        }
        let offset = reference.offset.context("image reference has no extent")?;
        if let Some(texture) = self.cache.get(&offset) {
            self.ages.retain(|k| *k != offset);
            self.ages.push_back(offset);
            return Ok(Arc::clone(texture));
        }
        let class = reference
            .class
            .as_ref()
            .context("image reference class missing")?;
        let bytes = self.archive.read_at(
            offset,
            reference.size.context("image extent missing")? as usize,
        )?;
        let shift = self.archive.address_shift;
        let image = match class.name.as_str() {
            "Cr6Ti" => decode_r6ti(&bytes, shift)?,
            "CRip007" => decode_rip007(&bytes, class.schema, shift)?,
            "CRip008" => decode_rip008(&bytes, class.schema, shift)?,
            "CS5i" => rugp_assets::s5i::decode_s5i(&bytes, class.schema)?,
            "CRmt" => Crmt::parse(&bytes, shift)?
                .levels
                .first()
                .context("texture has no mipmaps")?
                .decode()?,
            "CBg2d" => {
                let bg = Background::parse(&bytes, class.schema, shift)?;
                ensure!(
                    bg.planes.len() == 1,
                    "background needs {}-plane/motion rendering",
                    bg.planes.len()
                );
                let inline = bg.planes[0]
                    .image
                    .inline
                    .as_ref()
                    .context("external background image plane")?;
                inline.decode_bitmap()?
            }
            "CRio" => anyhow::bail!("unbound CRio image argument"),
            other => anyhow::bail!("unsupported texture class {other}"),
        };
        let texture = Arc::new(Texture::from_image(image)?);
        while self.bytes + texture.rgba.len() > self.budget {
            let Some(key) = self.ages.pop_front() else {
                break;
            };
            if let Some(old) = self.cache.remove(&key) {
                self.bytes -= old.rgba.len();
            }
        }
        self.bytes += texture.rgba.len();
        self.cache.insert(offset, Arc::clone(&texture));
        self.ages.push_back(offset);
        Ok(texture)
    }
}
