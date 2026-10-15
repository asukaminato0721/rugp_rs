use crate::render::Texture;
use anyhow::{Context, Result, ensure};
use rugp_assets::{
    RioArchive,
    background::SceneBackground,
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
    /// Scoped host-instruction observation, not a cache/preload unlock hook.
    observed: Option<Vec<ObjectRef>>,
}
#[derive(Clone)]
pub struct WorldImagePlane {
    pub texture: Arc<Texture>,
    pub plane: crate::world::Plane,
    /// Serialized base +18 flags. Vm60 1003bff3 skips bit 1 in the
    /// no-sprite pass; this is distinct from runtime +60 suppression flags.
    pub draw_flags: u32,
}
/// One native image replacement, with independent per-slot world geometry.
#[derive(Clone)]
pub struct WorldImage {
    pub planes: Vec<WorldImagePlane>,
    pub boundary: crate::world::Plane,
}
impl ImageManager {
    pub fn load_world(&mut self, reference: &ObjectRef) -> Result<WorldImage> {
        if reference
            .class
            .as_ref()
            .is_some_and(|c| c.name == "CDcAgesModelQsT")
        {
            ensure!(
                reference.class.as_ref().unwrap().schema == 1,
                "unsupported textured model schema"
            );
            let inline = reference
                .inline
                .as_ref()
                .context("stored textured models are not implemented")?;
            // The original object reader has already bounded the two geometry
            // blobs and resolved material references in their shared context.
            // The final reference is the declared base image, not an arbitrary
            // material. Use its real texture/planes without the 3D mesh warp.
            let base = inline
                .references
                .last()
                .context("textured model has no base image")?;
            ensure!(
                base.class
                    .as_ref()
                    .is_some_and(|c| ["CBg2d", "Cr6Ti", "CRmt", "CRip007", "CRip008"]
                        .contains(&c.name.as_str())),
                "unsupported textured-model base image"
            );
            return self.load_world(base);
        }
        if reference.class.as_ref().is_some_and(|c| c.name == "CBg2d") {
            let bytes = self.archive.read_at(
                reference.offset.context("background offset missing")?,
                reference.size.context("background extent missing")? as usize,
            )?;
            let scene = SceneBackground::parse(
                &bytes,
                reference.class.as_ref().unwrap().schema,
                self.archive.address_shift,
            )?;
            // The bounded asset parser preserves extended color/range/property
            // packets. Use the actual image planes and basic projection for now;
            // those additional presentation modifiers must not stop the story.
            let boundary = crate::world::Plane::scene_background(&scene)?;
            let mut planes = Vec::new();
            if scene.layers.is_empty() {
                ensure!(
                    scene.background.planes.len() == 1,
                    "legacy background canvas composition is not implemented yet"
                );
                let texture = self.load(&scene.background.planes[0].image)?;
                ensure!(
                    texture.width == u32::from(scene.background.width)
                        && texture.height == u32::from(scene.background.height),
                    "background canvas composition is not implemented yet"
                );
                planes.push(WorldImagePlane {
                    texture,
                    plane: boundary,
                    draw_flags: 0,
                });
            } else {
                // Native slots are capped at eight, in linked-list order.
                for layer in scene.layers.iter().take(8) {
                    let texture = self.load(&layer.image)?;
                    let plane = crate::world::Plane::background_layer(&scene, layer, &texture)?;
                    planes.push(WorldImagePlane {
                        texture,
                        plane,
                        draw_flags: layer.parameters[2] as u32,
                    });
                }
            }
            Ok(WorldImage { planes, boundary })
        } else {
            let texture = self.load(reference)?;
            let plane = crate::world::Plane::bitmap(&texture)?;
            Ok(WorldImage {
                boundary: plane,
                planes: vec![WorldImagePlane {
                    texture,
                    plane,
                    draw_flags: 0,
                }],
            })
        }
    }
    pub fn new(archive: Arc<RioArchive>) -> Self {
        Self {
            archive,
            cache: BTreeMap::new(),
            ages: VecDeque::new(),
            bytes: 0,
            budget: 128 * 1024 * 1024,
            observed: None,
        }
    }
    pub(crate) fn begin_observation(&mut self) -> Result<()> {
        ensure!(self.observed.is_none(), "nested image observation");
        self.observed = Some(Vec::new());
        Ok(())
    }
    pub(crate) fn end_observation(&mut self) -> Vec<ObjectRef> {
        self.observed.take().unwrap_or_default()
    }
    fn observe(&mut self, reference: &ObjectRef) -> Result<()> {
        if let Some(observed) = &mut self.observed {
            ensure!(
                observed.len() < 4096,
                "host image observation exceeds limit"
            );
            observed.push(reference.clone());
        }
        Ok(())
    }
    pub fn load(&mut self, reference: &ObjectRef) -> Result<Arc<Texture>> {
        if let Some(inline) = &reference.inline {
            return Ok(Arc::new(Texture::from_image(inline.decode_bitmap()?)?));
        }
        let offset = reference.offset.context("image reference has no extent")?;
        if let Some(texture) = self.cache.get(&offset) {
            let texture = Arc::clone(texture);
            self.ages.retain(|k| *k != offset);
            self.ages.push_back(offset);
            self.observe(reference)?;
            return Ok(texture);
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
                let scene = SceneBackground::parse(&bytes, class.schema, shift)?;
                ensure!(
                    scene.layers.is_empty(),
                    "world background cannot be loaded as a flat texture"
                );
                let bg = scene.background;
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
        self.observe(reference)?;
        Ok(texture)
    }
}
