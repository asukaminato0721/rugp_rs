//! Coordinated scene/object state, not a screenshot-only save. Texture and RIO
//! reference tables must be shared with the enclosing VM/UI/actor snapshot.
//! This component does not restore VM, audio, menus, dialogue, profiles or input.
use super::*;
use crate::{
    image_manager::WorldImagePlane,
    posture::snapshot::PostureSnapshot,
    render::snapshot::{RestoredTextures, TextureSnapshotWriter},
    world::{CameraTween, Plane, WorldLayerFlags},
};
use rugp_assets::{mime::MimeCurve, object::snapshot::ReferenceSnapshotWriter};
use rugp_scene_vm::vm::snapshot::ValueSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const MAX_OBJECTS: usize = 4096;
const MAX_VALUES: usize = 1_000_000;
const MAX_AUX_BYTES: usize = 64 * 1024 * 1024;
type Properties = Vec<(String, ValueSnapshot)>;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneSnapshot {
    version: u32,
    primary_buffer: u64,
    frame: u32,
    objects: Vec<(u64, SavedObject)>,
    buffers: Vec<(u64, u32)>,
    transition: Option<SavedTransition>,
    parse: Option<SavedParse>,
    parse_wait: bool,
    posture_wait: Option<SavedPostureWait>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedObject {
    enabled: bool,
    visible: bool,
    input: bool,
    sprite: bool,
    auto_update: bool,
    rect: (i32, i32, u32, u32),
    properties: Properties,
    camera: Camera,
    camera_start: Option<Camera>,
    pending_camera: Option<CameraTween>,
    postures: Vec<SavedPosture>,
    camera_center_y: i16,
    world_layers: WorldLayerFlags,
    opacity: u8,
    parent: Option<u64>,
    pending_draw_ratio: Option<MimeCurve>,
    menu: Option<u64>,
    backdrop: Option<crate::message_window::Backdrop>,
    layers: Vec<(i32, SavedLayer)>,
    character: SavedCharacter,
}
#[derive(Serialize, Deserialize)]
enum SavedLayer {
    Screen(u32),
    World {
        planes: Vec<SavedPlane>,
        boundary: Plane,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPlane {
    texture: u32,
    plane: Plane,
    draw_flags: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedCharacter {
    face: Properties,
    layers: Vec<(i32, [f32; 2])>,
    texture: Option<u32>,
    position: [i32; 3],
    style: i32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPosture {
    source: u32,
    motion: PostureSnapshot,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPostureWait {
    source: u32,
    snap: bool,
    remaining_ms: Option<u32>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedTransition {
    from: u32,
    to: u32,
    elapsed: u32,
    duration: u32,
    curve: Option<FadeCurve>,
    spatial: Option<spatial_fade::SpatialFade>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedParse {
    scene: Vec<(u64, SavedObject)>,
    curves: Vec<(u64, MimeCurve)>,
    cameras: Vec<(u64, CameraTween)>,
    elapsed: u32,
    duration: u32,
}

#[derive(Default)]
struct Budget {
    values: usize,
    bytes: usize,
    texture_uses: usize,
    postures: usize,
    roots: HashSet<u32>,
}
impl Budget {
    fn bytes(&mut self, bytes: usize) -> Result<()> {
        ensure!(
            bytes <= MAX_AUX_BYTES.saturating_sub(self.bytes),
            "scene snapshot auxiliary payload exceeds limit"
        );
        self.bytes += bytes;
        Ok(())
    }
    fn root(&mut self, root: u32) -> Result<()> {
        ensure!(
            self.roots.insert(root),
            "reused scene snapshot reference root"
        );
        Ok(())
    }
    fn texture(&mut self) -> Result<()> {
        ensure!(
            self.texture_uses < 65_536,
            "scene snapshot texture-use count exceeds limit"
        );
        self.texture_uses += 1;
        Ok(())
    }
    fn properties(&mut self, properties: &Properties) -> Result<()> {
        ensure!(
            properties.len() <= 4096,
            "scene snapshot property count exceeds limit"
        );
        let mut names = HashSet::new();
        for (name, value) in properties {
            ensure!(
                name.len() <= 4096 && names.insert(name),
                "invalid or duplicate scene snapshot property"
            );
            ensure!(
                self.values < MAX_VALUES,
                "scene snapshot value count exceeds limit"
            );
            self.values += 1;
            self.bytes(name.len())?;
            match value {
                ValueSnapshot::Real(value) => {
                    ensure!(value.is_finite(), "nonfinite scene snapshot value")
                }
                ValueSnapshot::Text(text) => self.bytes(text.len())?,
                ValueSnapshot::Bytes(bytes) => self.bytes(bytes.len())?,
                ValueSnapshot::Reference(root) => self.root(*root)?,
                _ => {}
            }
        }
        Ok(())
    }
}
impl Game {
    /// Scene component only; callers must capture the other game components
    /// without advancing the game between captures. Diagnostic no-raster output
    /// is not a portable visual save and is deliberately rejected.
    pub fn snapshot_scene(
        &self,
        references: &mut ReferenceSnapshotWriter,
        textures: &mut TextureSnapshotWriter,
    ) -> Result<SceneSnapshot> {
        ensure!(
            self.rasterize_scene,
            "cannot save a scene with rasterization disabled"
        );
        let frame = textures.frame(&self.frame)?;
        let objects = capture_objects(&self.objects, references, textures)?;
        let buffers = self
            .buffers
            .iter()
            .map(|(id, texture)| Ok((*id, textures.insert(texture)?)))
            .collect::<Result<_>>()?;
        let transition = self
            .transition
            .as_ref()
            .map(|transition| -> Result<_> {
                Ok(SavedTransition {
                    from: textures.rgba_frame(
                        self.frame.width,
                        self.frame.height,
                        &transition.from,
                    )?,
                    to: {
                        let (width, height) = transition
                            .spatial
                            .as_ref()
                            .map_or(self.size(), |s| s.source_size(self.size()));
                        textures.rgba_frame(width, height, &transition.to)?
                    },
                    elapsed: transition.elapsed,
                    duration: transition.duration,
                    curve: transition.curve.clone(),
                    spatial: transition.spatial,
                })
            })
            .transpose()?;
        let parse = self
            .parse_transition
            .as_ref()
            .map(|parse| -> Result<_> {
                Ok(SavedParse {
                    scene: capture_objects(&parse.scene, references, textures)?,
                    curves: parse
                        .curves
                        .iter()
                        .map(|(id, c)| (*id, c.clone()))
                        .collect(),
                    cameras: parse.cameras.iter().map(|(id, c)| (*id, *c)).collect(),
                    elapsed: parse.elapsed,
                    duration: parse.duration,
                })
            })
            .transpose()?;
        let posture_wait = self
            .posture_wait
            .as_ref()
            .map(|wait| -> Result<_> {
                Ok(SavedPostureWait {
                    source: references.insert(&wait.reference)?,
                    snap: wait.snap,
                    remaining_ms: wait.remaining_ms,
                })
            })
            .transpose()?;
        let snapshot = SceneSnapshot {
            version: 1,
            primary_buffer: self.primary_buffer,
            frame,
            objects,
            buffers,
            transition,
            parse,
            parse_wait: self.parse_wait,
            posture_wait,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
}
impl SceneSnapshot {
    pub(super) fn wait_count(&self) -> usize {
        usize::from(self.parse_wait) + usize::from(self.posture_wait.is_some())
    }
    pub(super) fn validate_vm(&self, vm: &Vm) -> Result<()> {
        if self.wait_count() > 0 {
            let expected = if self.parse_wait {
                "CVmGenericMsg"
            } else {
                "CVmSync"
            };
            ensure!(
                vm.pending_instruction()
                    .is_some_and(|i| i.class.name == expected),
                "scene snapshot has a different pending VM instruction"
            );
        }
        Ok(())
    }
}
fn capture_properties(
    properties: &BTreeMap<String, Value>,
    references: &mut ReferenceSnapshotWriter,
) -> Result<Properties> {
    ensure!(
        properties.len() <= 4096,
        "scene snapshot property count exceeds limit"
    );
    properties
        .iter()
        .map(|(name, value)| Ok((name.clone(), ValueSnapshot::capture(value, references)?)))
        .collect()
}
fn capture_objects(
    objects: &BTreeMap<u64, ObjectState>,
    references: &mut ReferenceSnapshotWriter,
    textures: &mut TextureSnapshotWriter,
) -> Result<Vec<(u64, SavedObject)>> {
    ensure!(
        objects.len() <= MAX_OBJECTS,
        "scene snapshot object count exceeds limit"
    );
    objects
        .iter()
        .map(|(id, o)| {
            let layers = o
                .layers
                .iter()
                .map(|(index, layer)| {
                    Ok((
                        *index,
                        match layer {
                            Layer::Screen(texture) => SavedLayer::Screen(textures.insert(texture)?),
                            Layer::World(world) => SavedLayer::World {
                                planes: world
                                    .planes
                                    .iter()
                                    .map(|p| {
                                        Ok(SavedPlane {
                                            texture: textures.insert(&p.texture)?,
                                            plane: p.plane,
                                            draw_flags: p.draw_flags,
                                        })
                                    })
                                    .collect::<Result<_>>()?,
                                boundary: world.boundary,
                            },
                        },
                    ))
                })
                .collect::<Result<_>>()?;
            let postures = o
                .postures
                .iter()
                .map(|p| {
                    ensure!(
                        p.source == posture::PostureIdentity::of(&p.reference)?,
                        "posture source identity changed before capture"
                    );
                    Ok(SavedPosture {
                        source: references.insert(&p.reference)?,
                        motion: p.motion.snapshot()?,
                    })
                })
                .collect::<Result<_>>()?;
            Ok((
                *id,
                SavedObject {
                    enabled: o.enabled,
                    visible: o.visible,
                    input: o.input,
                    sprite: o.sprite,
                    auto_update: o.auto_update,
                    rect: o.rect,
                    properties: capture_properties(&o.properties, references)?,
                    camera: o.camera,
                    camera_start: o.camera_start,
                    pending_camera: o.pending_camera,
                    postures,
                    camera_center_y: o.camera_center_y.get(),
                    world_layers: o.world_layers,
                    opacity: o.opacity,
                    parent: o.parent,
                    pending_draw_ratio: o.pending_draw_ratio.clone(),
                    menu: o.menu,
                    backdrop: o.backdrop,
                    layers,
                    character: SavedCharacter {
                        face: capture_properties(&o.character.face, references)?,
                        layers: o.character.layers.iter().map(|(id, v)| (*id, *v)).collect(),
                        texture: o
                            .character
                            .texture
                            .as_ref()
                            .map(|t| textures.insert(t))
                            .transpose()?,
                        position: o.character.position,
                        style: o.character.style,
                    },
                },
            ))
        })
        .collect()
}
impl SceneSnapshot {
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported scene snapshot version");
        ensure!(
            !(self.parse_wait && self.posture_wait.is_some()),
            "overlapping scene snapshot waits"
        );
        let mut budget = Budget::default();
        validate_objects(&self.objects, &mut budget)?;
        ensure!(
            self.buffers.len() <= MAX_OBJECTS,
            "scene snapshot buffer count exceeds limit"
        );
        let mut buffers = HashSet::new();
        for (id, _) in &self.buffers {
            ensure!(buffers.insert(id), "duplicate scene snapshot buffer");
            budget.texture()?;
        }
        if let Some(transition) = &self.transition {
            ensure!(
                transition.duration > 0 && transition.elapsed <= transition.duration,
                "invalid scene snapshot transition clock"
            );
            if let Some(spatial) = &transition.spatial {
                if let spatial_fade::Operator::Curtain { direction } = spatial.operator {
                    ensure!(direction <= 3, "invalid curtain direction");
                }
                if let spatial_fade::Operator::Stretch {
                    parameters, source, ..
                } = spatial.operator
                {
                    ensure!(
                        spatial.source_size.0 > 0
                            && spatial.source_size.1 > 0
                            && spatial.source_size.0 <= 65535
                            && spatial.source_size.1 <= 65535,
                        "invalid stretch source size"
                    );
                    ensure!(
                        source[2..]
                            .iter()
                            .chain(&parameters.source_end[2..])
                            .chain(&parameters.destination_end[2..])
                            .all(|n| *n >= 0),
                        "negative stretch extent"
                    );
                    ensure!(
                        parameters.frames <= 0
                            && parameters.opacity.iter().all(|n| (0..=8192).contains(n)),
                        "invalid stretch clock or opacity"
                    );
                }
                ensure!(
                    spatial.previous_alpha.is_none_or(|alpha| alpha <= 256),
                    "invalid spatial fade alpha"
                );
                ensure!(
                    spatial.rectangle[2] >= 0 && spatial.rectangle[3] >= 0,
                    "negative spatial fade extent"
                );
            }
            if let Some(curve) = &transition.curve {
                curve.validate_snapshot()?;
            }
        }
        if let Some(parse) = &self.parse {
            ensure!(
                parse.elapsed <= parse.duration,
                "invalid scene snapshot ParseScreen clock"
            );
            validate_objects(&parse.scene, &mut budget)?;
            let contains = |id: &u64| parse.scene.iter().any(|(object, _)| object == id);
            let mut curves = HashSet::new();
            for (id, _) in &parse.curves {
                ensure!(
                    contains(id) && curves.insert(id),
                    "invalid scene snapshot ParseScreen curve owner"
                );
            }
            let mut cameras = HashSet::new();
            for (id, camera) in &parse.cameras {
                ensure!(
                    contains(id) && cameras.insert(id),
                    "invalid scene snapshot ParseScreen camera owner"
                );
                camera.validate_snapshot()?;
            }
        }
        if let Some(wait) = &self.posture_wait {
            ensure!(
                wait.remaining_ms != Some(0),
                "expired scene snapshot posture wait"
            );
            budget.root(wait.source)?;
        }
        Ok(())
    }
    /// Builds a candidate first. Applying it is a separate, infallible data
    /// swap after compatibility checks, for the enclosing full-game restore.
    pub fn restore(
        &self,
        game: &Game,
        references: &[ObjectRef],
        textures: &RestoredTextures,
    ) -> Result<RestoredScene> {
        self.validate()?;
        ensure!(
            self.primary_buffer == game.primary_buffer,
            "scene snapshot primary framebuffer differs"
        );
        let frame = textures.frame(self.frame)?;
        ensure!(
            (frame.width, frame.height) == game.size(),
            "scene snapshot framebuffer size differs"
        );
        let objects = restore_objects(&self.objects, references, textures)?;
        let buffers = self
            .buffers
            .iter()
            .map(|(id, index)| Ok((*id, textures.get(*index)?)))
            .collect::<Result<_>>()?;
        let transition = self
            .transition
            .as_ref()
            .map(|t| {
                let from = textures.get(t.from)?;
                let to = textures.get(t.to)?;
                ensure!(
                    (from.width, from.height) == game.size()
                        && (to.width, to.height)
                            == t.spatial
                                .as_ref()
                                .map_or(game.size(), |s| s.source_size(game.size())),
                    "scene snapshot transition surface size differs"
                );
                Ok(Transition {
                    from: from.rgba.clone(),
                    to: to.rgba.clone(),
                    elapsed: t.elapsed,
                    duration: t.duration,
                    curve: t.curve.clone(),
                    spatial: t.spatial,
                })
            })
            .transpose()?;
        let parse_transition = self
            .parse
            .as_ref()
            .map(|p| -> Result<_> {
                Ok(parse_screen::ParseTransition {
                    scene: restore_objects(&p.scene, references, textures)?,
                    curves: p.curves.iter().map(|(id, c)| (*id, c.clone())).collect(),
                    cameras: p.cameras.iter().map(|(id, c)| (*id, *c)).collect(),
                    elapsed: p.elapsed,
                    duration: p.duration,
                })
            })
            .transpose()?;
        let posture_wait = self
            .posture_wait
            .as_ref()
            .map(|w| -> Result<_> {
                let reference = posture_source(w.source, references)?;
                Ok(posture::PostureWait {
                    source: posture::PostureIdentity::of(&reference)?,
                    reference,
                    snap: w.snap,
                    remaining_ms: w.remaining_ms,
                })
            })
            .transpose()?;
        Ok(RestoredScene {
            primary_buffer: self.primary_buffer,
            frame,
            objects,
            buffers,
            transition,
            parse_transition,
            parse_wait: self.parse_wait,
            posture_wait,
        })
    }
}
fn validate_objects(objects: &[(u64, SavedObject)], budget: &mut Budget) -> Result<()> {
    ensure!(
        objects.len() <= MAX_OBJECTS,
        "scene snapshot object count exceeds limit"
    );
    let mut ids = HashSet::new();
    for (id, o) in objects {
        ensure!(ids.insert(*id), "duplicate scene snapshot object");
        ensure!(
            o.rect.2 <= 65535 && o.rect.3 <= 65535,
            "scene snapshot viewport exceeds native range"
        );
        o.world_layers.validate_snapshot()?;
        if let Some(camera) = o.pending_camera {
            camera.validate_snapshot()?;
        }
        if let Some(backdrop) = o.backdrop {
            ensure!(
                backdrop.source <= 256 && backdrop.destination_shift <= 8,
                "invalid scene snapshot backdrop"
            );
        }
        budget.properties(&o.properties)?;
        budget.properties(&o.character.face)?;
        ensure!(
            o.layers.len() <= 256 && o.character.layers.len() <= 4096,
            "scene snapshot layer count exceeds limit"
        );
        let mut indices = HashSet::new();
        for (index, layer) in &o.layers {
            ensure!(indices.insert(index), "duplicate scene snapshot layer");
            match layer {
                SavedLayer::Screen(_) => budget.texture()?,
                SavedLayer::World { planes, boundary } => {
                    ensure!(
                        !planes.is_empty() && planes.len() <= 8,
                        "invalid scene snapshot world-plane count"
                    );
                    boundary.validate_snapshot()?;
                    for plane in planes {
                        plane.plane.validate_snapshot()?;
                        budget.texture()?;
                    }
                }
            }
        }
        let mut character_layers = HashSet::new();
        for (index, vector) in &o.character.layers {
            ensure!(
                character_layers.insert(index) && vector.iter().all(|v| v.is_finite()),
                "invalid scene snapshot character layer"
            );
        }
        if o.character.texture.is_some() {
            budget.texture()?;
        }
        for posture in &o.postures {
            ensure!(
                budget.postures < 8192,
                "scene snapshot posture count exceeds limit"
            );
            budget.postures += 1;
            budget.root(posture.source)?;
            budget.bytes(posture.motion.payload_bytes()?)?;
        }
    }
    let parents = objects
        .iter()
        .map(|(id, o)| (*id, o.parent))
        .collect::<HashMap<_, _>>();
    let mut complete = HashSet::new();
    for (id, _) in objects {
        let mut chain = HashSet::new();
        let mut current = Some(*id);
        while let Some(id) = current {
            if complete.contains(&id) {
                break;
            }
            ensure!(chain.insert(id), "cyclic scene snapshot parent links");
            current = parents.get(&id).copied().flatten();
        }
        complete.extend(chain);
    }
    Ok(())
}
fn restore_properties(
    properties: &Properties,
    references: &[ObjectRef],
) -> Result<BTreeMap<String, Value>> {
    properties
        .iter()
        .map(|(name, value)| Ok((name.clone(), value.restore(references)?)))
        .collect()
}
fn posture_source(index: u32, references: &[ObjectRef]) -> Result<ObjectRef> {
    let reference = references
        .get(index as usize)
        .context("posture snapshot reference root out of range")?;
    ensure!(
        reference
            .class
            .as_ref()
            .is_some_and(|c| c.name == "CPostureMoment" && c.schema == 4),
        "invalid scene snapshot posture source"
    );
    posture::PostureIdentity::of(reference)?;
    Ok(reference.clone())
}
fn restore_objects(
    objects: &[(u64, SavedObject)],
    references: &[ObjectRef],
    textures: &RestoredTextures,
) -> Result<BTreeMap<u64, ObjectState>> {
    let mut identities = HashSet::new();
    objects
        .iter()
        .map(|(id, o)| {
            let layers = o
                .layers
                .iter()
                .map(|(index, layer)| {
                    Ok((
                        *index,
                        match layer {
                            SavedLayer::Screen(index) => Layer::Screen(textures.get(*index)?),
                            SavedLayer::World { planes, boundary } => Layer::World(WorldImage {
                                planes: planes
                                    .iter()
                                    .map(|p| {
                                        Ok(WorldImagePlane {
                                            texture: textures.get(p.texture)?,
                                            plane: p.plane,
                                            draw_flags: p.draw_flags,
                                        })
                                    })
                                    .collect::<Result<_>>()?,
                                boundary: *boundary,
                            }),
                        },
                    ))
                })
                .collect::<Result<_>>()?;
            let postures = o
                .postures
                .iter()
                .map(|p| {
                    let reference = posture_source(p.source, references)?;
                    let source = posture::PostureIdentity::of(&reference)?;
                    ensure!(
                        identities.insert(source.clone()),
                        "duplicate active posture identity in scene snapshot"
                    );
                    Ok(posture::PostureInstance {
                        source,
                        reference: Arc::new(reference),
                        motion: p.motion.restore()?,
                    })
                })
                .collect::<Result<_>>()?;
            let object = ObjectState {
                enabled: o.enabled,
                visible: o.visible,
                input: o.input,
                sprite: o.sprite,
                auto_update: o.auto_update,
                rect: o.rect,
                properties: restore_properties(&o.properties, references)?,
                camera: o.camera,
                camera_start: o.camera_start,
                pending_camera: o.pending_camera,
                postures,
                camera_center_y: Cell::new(o.camera_center_y),
                world_layers: o.world_layers,
                opacity: o.opacity,
                parent: o.parent,
                pending_draw_ratio: o.pending_draw_ratio.clone(),
                menu: o.menu,
                backdrop: o.backdrop,
                layers,
                character: character::CharacterState {
                    face: restore_properties(&o.character.face, references)?,
                    layers: o.character.layers.iter().copied().collect(),
                    texture: o.character.texture.map(|id| textures.get(id)).transpose()?,
                    position: o.character.position,
                    style: o.character.style,
                },
            };
            // Projection validation writes the cached center; restore the actual
            // saved cache afterwards because later native boundary correction uses it.
            if object.visible {
                object.validate_projection()?;
            }
            object.camera_center_y.set(o.camera_center_y);
            Ok((*id, object))
        })
        .collect()
}
pub struct RestoredScene {
    primary_buffer: u64,
    frame: Framebuffer,
    objects: BTreeMap<u64, ObjectState>,
    buffers: BTreeMap<u64, Arc<Texture>>,
    transition: Option<Transition>,
    parse_transition: Option<parse_screen::ParseTransition>,
    parse_wait: bool,
    posture_wait: Option<posture::PostureWait>,
}
impl RestoredScene {
    pub(crate) fn objects(&self) -> &BTreeMap<u64, ObjectState> {
        &self.objects
    }
    pub(crate) fn object(&self, id: u64) -> Option<&ObjectState> {
        self.objects.get(&id)
    }
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.frame.width, self.frame.height)
    }
    pub(crate) fn wait_count(&self) -> usize {
        usize::from(self.parse_wait) + usize::from(self.posture_wait.is_some())
    }
    /// Component application only, not a public save-slot operation. The caller
    /// coordinates VM, actor identities, audio and UI before resuming the game.
    pub fn apply(self, game: &mut Game) -> Result<()> {
        ensure!(
            self.primary_buffer == game.primary_buffer
                && (self.frame.width, self.frame.height) == game.size(),
            "scene restore target changed during preparation"
        );
        self.commit(game);
        Ok(())
    }
    /// All compatibility checks must have succeeded before the full-game swap.
    pub(super) fn commit(self, game: &mut Game) {
        game.frame = self.frame;
        game.objects = self.objects;
        game.buffers = self.buffers;
        game.transition = self.transition;
        game.parse_transition = self.parse_transition;
        game.parse_wait = self.parse_wait;
        game.posture_wait = self.posture_wait;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::snapshot::TextureSnapshotTable;
    use rugp_assets::object::{ClassId, snapshot::ReferenceSnapshot};
    use rugp_scene_vm::vm::snapshot::VmSnapshot;
    fn texture() -> Arc<Texture> {
        Arc::new(Texture {
            width: 4,
            height: 4,
            display_scale: 1.0,
            rgba: [40, 80, 120, 200].repeat(16),
        })
    }
    fn copy(snapshot: &SceneSnapshot) -> SceneSnapshot {
        serde_json::from_slice(&serde_json::to_vec(snapshot).unwrap()).unwrap()
    }
    fn tables(
        references: ReferenceSnapshotWriter,
        textures: TextureSnapshotWriter,
    ) -> Result<(Vec<ObjectRef>, RestoredTextures)> {
        let wire = serde_json::to_vec(&(references.finish()?, textures.finish()?))?;
        let (references, textures): (ReferenceSnapshot, TextureSnapshotTable) =
            serde_json::from_slice(&wire)?;
        Ok((references.restore()?, textures.restore()?))
    }
    #[test]
    fn object_layers_character_properties_and_shared_surfaces_survive_without_flattening()
    -> Result<()> {
        let texture = texture();
        let plane = Plane::bitmap_at(&texture, [3, -2, 1440])?;
        let actor = ObjectRef::transient(ClassId {
            name: "CGenericUso".into(),
            schema: 1,
        });
        let mut references = ReferenceSnapshotWriter::default();
        let actor_root = references.insert(&actor)?;
        let mut textures = TextureSnapshotWriter::default();
        let buffer = textures.insert(&texture)?;
        let mut object = ObjectState {
            rect: (0, 0, 4, 4),
            enabled: false,
            input: true,
            sprite: true,
            auto_update: false,
            opacity: 192,
            parent: Some(999),
            menu: Some(77),
            pending_draw_ratio: Some(MimeCurve::alpha_ramp(true)),
            ..Default::default()
        };
        object.camera_start = Some(Camera {
            position: [1, 2, 3],
            ..Default::default()
        });
        object.pending_camera = Some(CameraTween::new(
            Camera::default(),
            Camera {
                angles_q4: [10, 20, 0],
                ..Default::default()
            },
            3,
        )?);
        object.world_layers.set_sprite_pass_suppressed(3, true);
        object
            .layers
            .insert(-1, Layer::Screen(Arc::clone(&texture)));
        object.layers.insert(
            2,
            Layer::World(WorldImage {
                boundary: plane,
                planes: vec![WorldImagePlane {
                    texture: Arc::clone(&texture),
                    plane,
                    draw_flags: 2,
                }],
            }),
        );
        object.character.texture = Some(Arc::clone(&texture));
        object.character.position = [3, -2, 1120];
        object.character.style = 7;
        object.character.layers.insert(4, [0.5, 1.5]);
        object
            .character
            .face
            .insert("part".into(), Value::Reference(actor.clone()));
        object.properties = BTreeMap::from([
            ("actor".into(), Value::Reference(actor)),
            ("integer".into(), Value::Integer(-7)),
            ("real".into(), Value::Real(0.25)),
            ("text".into(), Value::Text("synthetic".into())),
            ("bytes".into(), Value::Bytes(vec![1, 2, 3])),
            ("null".into(), Value::Null),
        ]);
        object.camera_center_y.set(-123);
        let original = BTreeMap::from([(1, object)]);
        let saved = capture_objects(&original, &mut references, &mut textures)?;
        validate_objects(&saved, &mut Budget::default())?;
        let (references, textures) = tables(references, textures)?;
        let restored = restore_objects(&saved, &references, &textures)?;
        let object = &restored[&1];
        assert!(!object.enabled && object.input && object.sprite && !object.auto_update);
        assert_eq!(object.camera_center_y.get(), -123);
        assert_eq!(object.parent, Some(999));
        assert_eq!(object.menu, Some(77));
        assert_eq!(object.opacity, 192);
        assert_eq!(object.camera_start, original[&1].camera_start);
        assert_eq!(object.pending_camera, original[&1].pending_camera);
        assert!(!object.world_layers.draws_in_sprite_pass(3));
        assert_eq!(object.pending_draw_ratio, original[&1].pending_draw_ratio);
        let Layer::Screen(screen) = &object.layers[&-1] else {
            panic!("screen changed kind")
        };
        let Layer::World(world) = &object.layers[&2] else {
            panic!("world was flattened")
        };
        assert_eq!(
            world.boundary.quad(&object.camera, object.rect)?,
            plane.quad(&object.camera, object.rect)?
        );
        assert_eq!(
            world.planes[0].plane.quad(&object.camera, object.rect)?,
            plane.quad(&object.camera, object.rect)?
        );
        assert!(Arc::ptr_eq(screen, &world.planes[0].texture));
        assert!(Arc::ptr_eq(
            screen,
            object.character.texture.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(screen, &textures.get(buffer)?));
        let Value::Reference(actor) = &object.properties["actor"] else {
            panic!("actor lost")
        };
        assert_eq!(
            actor.inline.as_ref().unwrap().identity(),
            references[actor_root as usize]
                .inline
                .as_ref()
                .unwrap()
                .identity()
        );
        assert!(
            serde_json::to_value(&object.properties)?
                == serde_json::to_value(&original[&1].properties)?
        );
        let expected = Game::render_objects(&original, (4, 4))?;
        let actual = Game::render_objects(&restored, (4, 4))?;
        assert_eq!(actual.rgba, expected.rgba);
        Ok(())
    }
    #[test]
    fn active_posture_sources_remain_bound_to_external_aliases_and_keep_delta_history() -> Result<()>
    {
        use rugp_assets::{motion::MotionCurve, posture::Posture};
        let source = posture::tests::media(100, 1);
        let parsed = Posture::from_inline(
            source.inline.as_ref().unwrap(),
            source.class.as_ref().unwrap(),
        )?;
        let channel = &parsed.references[2];
        let curve = MotionCurve::from_inline(
            channel.inline.as_ref().unwrap(),
            channel.class.as_ref().unwrap(),
        )?;
        let mut motion = crate::posture::PostureMotion::new(
            &parsed.fields,
            [Some(crate::motion::Motion::new(curve)?), None, None],
        )?;
        motion.configure_playback(0, 0.25, [0.0; 3])?;
        let camera = motion.advance(Camera::default(), 101);
        let mut objects = BTreeMap::from([(
            1,
            ObjectState {
                camera,
                postures: vec![posture::PostureInstance {
                    source: posture::PostureIdentity::of(&source)?,
                    reference: Arc::new(source.clone()),
                    motion,
                }],
                ..Default::default()
            },
        )]);
        let mut references = ReferenceSnapshotWriter::default();
        let external = references.insert(&source)?;
        let mut textures = TextureSnapshotWriter::default();
        let saved = capture_objects(&objects, &mut references, &mut textures)?;
        let (roots, textures) = tables(references, textures)?;
        let mut restored = restore_objects(&saved, &roots, &textures)?;
        assert!(
            restored[&1].postures[0].source
                == posture::PostureIdentity::of(&roots[external as usize])?
        );
        assert!(restored[&1].postures[0].source != objects[&1].postures[0].source);
        for dt in [0, 1, 2, 3, 77, 400] {
            for object in [&mut objects, &mut restored].map(|map| map.get_mut(&1).unwrap()) {
                object.camera = object.postures[0].motion.advance(object.camera, dt);
            }
            assert_eq!(restored[&1].camera, objects[&1].camera);
            assert_eq!(
                restored[&1].postures[0].motion.elapsed_ms(),
                objects[&1].postures[0].motion.elapsed_ms()
            );
        }
        Ok(())
    }
    fn simple() -> Result<SceneSnapshot> {
        let mut references = ReferenceSnapshotWriter::default();
        let mut textures = TextureSnapshotWriter::default();
        let object = ObjectState {
            rect: (0, 0, 4, 4),
            layers: BTreeMap::from([(0, Layer::Screen(texture()))]),
            ..Default::default()
        };
        Ok(SceneSnapshot {
            version: 1,
            primary_buffer: 9,
            frame: 0,
            objects: capture_objects(
                &BTreeMap::from([(1, object)]),
                &mut references,
                &mut textures,
            )?,
            buffers: vec![],
            transition: None,
            parse: None,
            parse_wait: false,
            posture_wait: None,
        })
    }
    #[test]
    fn malformed_scene_ids_parent_cycles_clocks_and_auxiliary_values_are_rejected() -> Result<()> {
        let saved = simple()?;
        saved.validate()?;
        let mut bad = copy(&saved);
        bad.version = 9;
        assert!(bad.validate().is_err());
        let mut bad = copy(&saved);
        bad.objects.push((1, copy(&saved).objects.pop().unwrap().1));
        assert!(bad.validate().is_err());
        let mut bad = copy(&saved);
        bad.objects[0].1.parent = Some(1);
        assert!(bad.validate().is_err());
        let mut bad = copy(&saved);
        bad.objects[0].1.properties = vec![
            ("a".into(), ValueSnapshot::Reference(0)),
            ("b".into(), ValueSnapshot::Reference(0)),
        ];
        assert!(bad.validate().is_err());
        let mut bad = copy(&saved);
        bad.objects[0].1.character.layers = vec![(0, [f32::NAN, 0.0])];
        assert!(bad.validate().is_err());
        let mut bad = copy(&saved);
        bad.objects[0].1.backdrop = Some(crate::message_window::Backdrop {
            color: [0; 3],
            source: 256,
            destination_shift: 255,
        });
        assert!(bad.validate().is_err());
        let mut bad = copy(&saved);
        bad.transition = Some(SavedTransition {
            from: 0,
            to: 0,
            elapsed: 1,
            duration: 0,
            curve: None,
            spatial: None,
        });
        assert!(bad.validate().is_err());
        let mut bad = copy(&saved);
        bad.parse = Some(SavedParse {
            scene: vec![],
            curves: vec![(999, MimeCurve::alpha_ramp(false))],
            cameras: vec![],
            elapsed: 0,
            duration: 100,
        });
        assert!(bad.validate().is_err());
        let mut budget = Budget {
            bytes: MAX_AUX_BYTES,
            ..Default::default()
        };
        assert!(budget.bytes(1).is_err());
        Ok(())
    }
    #[test]
    fn private_projection_and_fade_invariants_are_checked_during_deserialization() -> Result<()> {
        let plane = Plane::bitmap(&texture())?;
        let mut wire = serde_json::to_value(plane)?;
        wire["depth"] = serde_json::json!(-1);
        assert!(serde_json::from_value::<Plane>(wire).is_err());
        let mut wire = serde_json::to_value(plane)?;
        wire["origin"] = serde_json::json!([i32::MIN, 0]);
        assert!(serde_json::from_value::<Plane>(wire).is_err());
        let tween = CameraTween::new(Camera::default(), Camera::default(), 0)?;
        let mut wire = serde_json::to_value(tween)?;
        wire["style"] = serde_json::json!(255);
        assert!(serde_json::from_value::<CameraTween>(wire).is_err());
        let mut wire = serde_json::to_value(tween)?;
        wire["delta_position"] = serde_json::json!([i32::MAX, 0, 0]);
        wire["start"]["position"] = serde_json::json!([1, 0, 0]);
        assert!(serde_json::from_value::<CameraTween>(wire).is_err());
        assert!(
            serde_json::from_value::<WorldLayerFlags>(
                serde_json::json!({"suppressed_in_sprite_pass":1})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<FadeCurve>(serde_json::json!([[0, 0], [0, 8192]])).is_err()
        );
        assert!(serde_json::from_value::<FadeCurve>(serde_json::json!([[257, 8192]])).is_err());
        let curve: FadeCurve = serde_json::from_value(serde_json::json!([[0, 0], [256, 8192]]))?;
        assert_eq!(curve.alpha(50, 100), 128);
        Ok(())
    }
    fn pending_host() -> Result<Vm> {
        use rugp_assets::object::ClassId;
        use rugp_scene_vm::Scenario;
        let mut program = Program::default();
        program.insert(
            10,
            Scenario {
                object_id: 1,
                memory_size: 48,
                null_records: vec![],
                trailing_bytes: 0,
                instructions: vec![
                    Instruction {
                        index: 0,
                        stream_offset: 0,
                        class: ClassId {
                            name: "CVmSync".into(),
                            schema: 17,
                        },
                        address: 0,
                        next_address: 24,
                        flags: 0,
                        arguments: vec![],
                    },
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
        let mut vm = Vm::new(Arc::new(program), 1)?;
        ensure!(
            matches!(vm.step()?, Step::Host { .. }),
            "synthetic host was not suspended"
        );
        Ok(vm)
    }
    fn rewind_scene(game: &mut Game) -> Result<()> {
        let frame = game.frame().to_vec();
        let mut references = ReferenceSnapshotWriter::default();
        let mut textures = TextureSnapshotWriter::default();
        let vm = game.vm.snapshot(&mut references)?;
        let scene = game.snapshot_scene(&mut references, &mut textures)?;
        let wire = serde_json::to_vec(&(vm, scene, references.finish()?, textures.finish()?))?;
        let (vm, scene, references, textures): (
            VmSnapshot,
            SceneSnapshot,
            ReferenceSnapshot,
            TextureSnapshotTable,
        ) = serde_json::from_slice(&wire)?;
        let (vm, references) = game.vm.restore_snapshot(&vm, &references)?;
        let textures = textures.restore()?;
        let candidate = scene.restore(game, &references, &textures)?;
        game.objects.clear();
        game.buffers.clear();
        game.transition = None;
        game.parse_transition = None;
        game.posture_wait = None;
        game.frame.clear([1, 2, 3]);
        candidate.apply(game)?;
        game.vm = vm;
        ensure!(
            game.frame() == frame,
            "component restore changed current pixels"
        );
        Ok(())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_host_scene_components_continue_staged_parse_fade_and_posture_waits() -> Result<()> {
        // Uses the retail host/archive but synthetic surfaces and suspended
        // instructions to isolate scene clocks. It is not a full save-slot test.
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        game.frame = Framebuffer::new(4, 4)?;
        game.vm = pending_host()?;
        let red = Arc::new(Texture {
            width: 4,
            height: 4,
            display_scale: 1.0,
            rgba: [255, 0, 0, 255].repeat(16),
        });
        let green = Arc::new(Texture {
            width: 4,
            height: 4,
            display_scale: 1.0,
            rgba: [0, 255, 0, 255].repeat(16),
        });
        let tween = CameraTween::new(
            Camera::default(),
            Camera {
                angles_q4: [64, 32, 0],
                ..Default::default()
            },
            3,
        )?;
        game.objects.insert(
            1,
            ObjectState {
                rect: (0, 0, 4, 4),
                layers: BTreeMap::from([(0, Layer::Screen(Arc::clone(&red)))]),
                pending_camera: Some(tween),
                pending_draw_ratio: Some(MimeCurve::alpha_ramp(false)),
                ..Default::default()
            },
        );
        assert!(game.start_parse_transition(1000)?);
        game.parse_wait = true;
        game.objects
            .get_mut(&1)
            .unwrap()
            .layers
            .insert(0, Layer::Screen(Arc::clone(&green)));
        game.buffers.insert(1, Arc::clone(&green));
        assert_eq!(game.step(300)?, RunState::Waiting);
        rewind_scene(&mut game)?;
        let Layer::Screen(live) = &game.objects[&1].layers[&0] else {
            panic!("live surface lost")
        };
        let Layer::Screen(staged) = &game.parse_transition.as_ref().unwrap().scene[&1].layers[&0]
        else {
            panic!("staged surface lost")
        };
        assert!(!Arc::ptr_eq(live, staged));
        assert!(Arc::ptr_eq(live, &game.buffers[&1]));
        assert_eq!(live.rgba, green.rgba);
        assert_eq!(staged.rgba, red.rgba);
        assert_eq!(game.step(699)?, RunState::Waiting);
        assert_eq!(game.vm.cursor().unwrap().address, 0);
        assert_eq!(game.step(1)?, RunState::Ended);
        assert!(game.parse_transition.is_none());
        assert_eq!(game.objects[&1].camera, tween.target());
        // Delta-history posture, fractional clock and a shorter timed sync.
        game.ended = false;
        game.vm = pending_host()?;
        game.objects.clear();
        let source = posture::tests::media(100, 1);
        let parsed = rugp_assets::posture::Posture::from_inline(
            source.inline.as_ref().unwrap(),
            source.class.as_ref().unwrap(),
        )?;
        let channel = &parsed.references[2];
        let curve = rugp_assets::motion::MotionCurve::from_inline(
            channel.inline.as_ref().unwrap(),
            channel.class.as_ref().unwrap(),
        )?;
        let mut motion = crate::posture::PostureMotion::new(
            &parsed.fields,
            [Some(crate::motion::Motion::new(curve)?), None, None],
        )?;
        motion.configure_playback(0, 0.25, [0.0; 3])?;
        game.vm
            .variables
            .insert(1, Value::Reference(source.clone()));
        game.objects.insert(
            1,
            ObjectState {
                rect: (0, 0, 4, 4),
                postures: vec![posture::PostureInstance {
                    source: posture::PostureIdentity::of(&source)?,
                    reference: Arc::new(source.clone()),
                    motion,
                }],
                ..Default::default()
            },
        );
        game.posture_wait = Some(posture::PostureWait {
            source: posture::PostureIdentity::of(&source)?,
            reference: source,
            snap: true,
            remaining_ms: Some(200),
        });
        assert_eq!(game.step(101)?, RunState::Waiting);
        rewind_scene(&mut game)?;
        let Value::Reference(alias) = &game.vm.variables[&1] else {
            panic!("posture variable lost")
        };
        assert!(game.posture_wait.as_ref().unwrap().source == posture::PostureIdentity::of(alias)?);
        assert!(game.objects[&1].postures[0].source == posture::PostureIdentity::of(alias)?);
        assert_eq!(game.step(98)?, RunState::Waiting);
        assert_eq!(game.vm.cursor().unwrap().address, 0);
        assert_eq!(game.step(1)?, RunState::Ended);
        assert!(game.posture_wait.is_none() && game.objects[&1].postures.is_empty());
        assert_eq!(game.objects[&1].camera.position, [4, 0, 0]);
        // Plain framebuffer fade retains both endpoints and its exact wait.
        game.ended = false;
        game.vm = pending_host()?;
        game.parse_wait = true;
        game.transition = Some(Transition {
            from: red.rgba.clone(),
            to: [0, 0, 255, 255].repeat(16),
            elapsed: 300,
            duration: 1000,
            curve: None,
            spatial: None,
        });
        game.update_frame();
        rewind_scene(&mut game)?;
        assert_eq!(game.step(699)?, RunState::Waiting);
        assert_eq!(game.step(1)?, RunState::Ended);
        assert!(game.transition.is_none());
        assert!(game.frame() == [0, 0, 255, 255].repeat(16));
        // Curtain output depends on the preceding rendered sample, so both
        // its operator clock and the current frame must survive restoration.
        game.ended = false;
        game.vm = pending_host()?;
        game.parse_wait = false;
        game.transition = Some(Transition {
            from: red.rgba.clone(),
            to: [0, 0, 255, 255].repeat(16),
            elapsed: 128,
            duration: 256,
            curve: Some(serde_json::from_str("[[0,0],[256,8192]]")?),
            spatial: spatial_fade::SpatialFade::from_class("CFadeRSideCarten", [0; 4]),
        });
        game.update_frame();
        let mut expected = game.frame.rgba.clone();
        let mut operator = game.transition.as_ref().unwrap().spatial.unwrap();
        operator.render_q13(
            &mut expected,
            &game.transition.as_ref().unwrap().to,
            (4, 4),
            160 * 32,
        );
        rewind_scene(&mut game)?;
        game.transition.as_mut().unwrap().elapsed = 160;
        game.update_frame();
        assert_eq!(game.frame(), expected);
        Ok(())
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_scene_vm_roundtrip_keeps_startup_title_and_dialogue_pixels_and_typed_layers()
    -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let mut game = Game::open(root.as_ref())?;
        let mut audio = None;
        let mut checks = 0;
        let mut title = false;
        let mut dialogue = false;
        for tick in 0..300 {
            crate::audio::mute_requests(&mut game, &mut audio)?;
            game.step(1000)?;
            let check = tick == 0
                || (game.video.is_some() && checks == 1)
                || (game.waiting_for_selection() && !title)
                || (game.waiting_for_dialogue() && !dialogue);
            if check {
                ensure!(
                    game.async_images.is_empty(),
                    "checkpoint needs coordinated image actors"
                );
                let frame = game.frame().to_vec();
                let cursor = game.vm.cursor().cloned();
                let count = game.objects.len();
                let mut references = ReferenceSnapshotWriter::default();
                let mut textures = TextureSnapshotWriter::default();
                let vm = game.vm.snapshot(&mut references)?;
                let scene = game.snapshot_scene(&mut references, &mut textures)?;
                let wire =
                    serde_json::to_vec(&(vm, scene, references.finish()?, textures.finish()?))?;
                let (vm, scene, references, textures): (
                    VmSnapshot,
                    SceneSnapshot,
                    ReferenceSnapshot,
                    TextureSnapshotTable,
                ) = serde_json::from_slice(&wire)?;
                let (vm, references) = game.vm.restore_snapshot(&vm, &references)?;
                let textures = textures.restore()?;
                let candidate = scene.restore(&game, &references, &textures)?;
                game.objects.clear();
                game.buffers.clear();
                game.frame.clear([1, 2, 3]);
                game.vm = vm;
                candidate.apply(&mut game)?;
                ensure!(game.frame() == frame, "restored retail scene pixels differ");
                assert_eq!(game.vm.cursor(), cursor.as_ref());
                assert_eq!(game.objects.len(), count);
                checks += 1;
            }
            if game.waiting_for_selection() && !title {
                title = true;
                game.navigate_selection(1)?;
                game.activate_selection()?;
            }
            if game.waiting_for_dialogue() {
                dialogue = true;
                break;
            }
        }
        ensure!(
            title && dialogue && checks >= 4,
            "retail scene checkpoints not reached"
        );
        Ok(())
    }
}
