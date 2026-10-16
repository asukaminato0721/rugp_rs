//! Alias-aware, archive-independent object references for portable save states.
//! Unlike the diagnostic tree serializer, this format preserves inline instance
//! identity across VM variables, stack arguments, and host-owned actors. IDs are
//! local to one snapshot; neither process addresses nor fake RIO offsets are saved.
use super::{BitmapHeader, ClassId, InlineBitmap, InlineIdentity, InlineObject, ObjectRef};
use crate::{image::R6Header, motion::MotionCurve, posture::PostureFields, rip::RipHeader};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

const MAX_ROOTS: usize = 1_000_000;
const MAX_INLINES: usize = 65_536;
const MAX_NODES: usize = 1_000_000;
const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_DEPTH: usize = 128;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReferenceSnapshot {
    version: u32,
    roots: Vec<SavedReference>,
    inlines: Vec<SavedInline>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct SavedReference {
    class: Option<ClassId>,
    cache_index: Option<u32>,
    flags: u16,
    offset: Option<u64>,
    size: Option<u32>,
    owner: Option<Box<SavedReference>>,
    selector: u32,
    inline: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct SavedInline {
    class: ClassId,
    data: Vec<u8>,
    references: Vec<SavedReference>,
    bitmap: Option<SavedBitmap>,
    posture: Option<Box<PostureFields>>,
    motion: Option<Box<MotionCurve>>,
    #[serde(default)]
    character_pose: Option<[i32; 6]>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct SavedBitmap {
    data_offset: usize,
    header: SavedBitmapHeader,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
enum SavedBitmapHeader {
    R6 {
        version: u32,
        canvas_width: u16,
        canvas_height: u16,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        flags: u32,
        depth: u32,
        data_size: u32,
        display_scale: f32,
        auxiliary: u32,
        image_properties: Box<SavedReference>,
    },
    Rip {
        width: u16,
        height: u16,
        x: u16,
        y: u16,
        crop_width: u16,
        crop_height: u16,
        flags: u32,
        info: [u8; 7],
        data_size: u32,
        modern: bool,
        image_properties: Option<Box<SavedReference>>,
    },
}

#[derive(Default)]
struct Budget {
    nodes: usize,
    bytes: usize,
}
impl Budget {
    fn node(&mut self, depth: usize) -> Result<()> {
        ensure!(depth < MAX_DEPTH, "snapshot reference depth exceeds limit");
        ensure!(
            self.nodes < MAX_NODES,
            "snapshot reference count exceeds limit"
        );
        self.nodes += 1;
        Ok(())
    }
    fn bytes(&mut self, bytes: usize) -> Result<()> {
        ensure!(
            bytes <= MAX_BYTES.saturating_sub(self.bytes),
            "snapshot expanded payload exceeds limit"
        );
        self.bytes += bytes;
        Ok(())
    }
}

/// Use ONE writer for both VM and host references. Returned indices address the
/// restored root vector, not the private inline-identity table.
#[derive(Default)]
pub struct ReferenceSnapshotWriter {
    roots: Vec<SavedReference>,
    inlines: Vec<Option<SavedInline>>,
    identities: HashMap<InlineIdentity, u32>,
    budget: Budget,
    failed: bool,
}
impl ReferenceSnapshotWriter {
    pub fn insert(&mut self, reference: &ObjectRef) -> Result<u32> {
        ensure!(!self.failed, "snapshot writer previously failed");
        let result = (|| {
            ensure!(self.roots.len() < MAX_ROOTS, "too many snapshot roots");
            let saved = self.reference(reference, 0)?;
            let id = self.roots.len() as u32;
            self.roots.push(saved);
            Ok(id)
        })();
        self.failed = result.is_err();
        result
    }
    pub fn finish(self) -> Result<ReferenceSnapshot> {
        ensure!(!self.failed, "cannot finish failed snapshot writer");
        Ok(ReferenceSnapshot {
            version: 1,
            roots: self.roots,
            inlines: self
                .inlines
                .into_iter()
                .map(|v| v.context("unfinished inline snapshot"))
                .collect::<Result<_>>()?,
        })
    }
    fn reference(&mut self, reference: &ObjectRef, depth: usize) -> Result<SavedReference> {
        self.budget.node(depth)?;
        validate_reference(
            &reference.class,
            reference.offset,
            reference.size,
            reference.inline.is_some(),
        )?;
        self.budget
            .bytes(reference.class.as_ref().map_or(0, |c| c.name.len()))?;
        let owner = reference
            .owner
            .as_deref()
            .map(|r| self.reference(r, depth + 1).map(Box::new))
            .transpose()?;
        let inline = if let Some(body) = &reference.inline {
            let class = reference
                .class
                .as_ref()
                .context("inline snapshot has no class")?;
            validate_payload(
                body.posture.as_deref(),
                body.motion.as_deref(),
                &mut self.budget,
            )?;
            self.budget.bytes(body.data.len())?;
            let identity = body.identity();
            let previous = self.identities.get(&identity).copied();
            let id = if let Some(id) = previous {
                ensure!(
                    self.inlines[id as usize].is_some(),
                    "cyclic inline snapshot"
                );
                id
            } else {
                ensure!(
                    self.inlines.len() < MAX_INLINES,
                    "too many inline snapshot objects"
                );
                let id = self.inlines.len() as u32;
                self.identities.insert(identity, id);
                self.inlines.push(None);
                id
            };
            let saved = SavedInline {
                class: class.clone(),
                data: body.data.clone(),
                references: body
                    .references
                    .iter()
                    .map(|r| self.reference(r, depth + 1))
                    .collect::<Result<_>>()?,
                bitmap: body
                    .bitmap
                    .as_ref()
                    .map(|b| self.bitmap(b, depth + 1, body.data.len()))
                    .transpose()?,
                posture: body.posture.clone(),
                motion: body.motion.clone(),
                character_pose: body.character_pose.as_deref().copied(),
            };
            if previous.is_some() {
                ensure!(
                    self.inlines[id as usize].as_ref() == Some(&saved),
                    "conflicting payload for shared inline identity"
                );
            } else {
                self.inlines[id as usize] = Some(saved);
            }
            Some(id)
        } else {
            None
        };
        Ok(SavedReference {
            class: reference.class.clone(),
            cache_index: reference.cache_index,
            flags: reference.flags,
            offset: reference.offset,
            size: reference.size,
            owner,
            selector: reference.selector,
            inline,
        })
    }
    fn bitmap(
        &mut self,
        bitmap: &InlineBitmap,
        depth: usize,
        data_len: usize,
    ) -> Result<SavedBitmap> {
        let header = match &bitmap.header {
            BitmapHeader::R6(h) => {
                validate_bitmap(bitmap.data_offset, data_len, h.data_size, h.width, h.height)?;
                ensure!(
                    h.display_scale.is_finite(),
                    "nonfinite snapshot bitmap scale"
                );
                SavedBitmapHeader::R6 {
                    version: h.version,
                    canvas_width: h.canvas_width,
                    canvas_height: h.canvas_height,
                    x: h.x,
                    y: h.y,
                    width: h.width,
                    height: h.height,
                    flags: h.flags,
                    depth: h.depth,
                    data_size: h.data_size,
                    display_scale: h.display_scale,
                    auxiliary: h.auxiliary,
                    image_properties: Box::new(self.reference(&h.image_properties, depth)?),
                }
            }
            BitmapHeader::Rip(h) => {
                validate_bitmap(bitmap.data_offset, data_len, h.data_size, h.width, h.height)?;
                SavedBitmapHeader::Rip {
                    width: h.width,
                    height: h.height,
                    x: h.x,
                    y: h.y,
                    crop_width: h.crop_width,
                    crop_height: h.crop_height,
                    flags: h.flags,
                    info: h.info,
                    data_size: h.data_size,
                    modern: h.modern,
                    image_properties: h
                        .image_properties
                        .as_deref()
                        .map(|r| self.reference(r, depth).map(Box::new))
                        .transpose()?,
                }
            }
        };
        Ok(SavedBitmap {
            data_offset: bitmap.data_offset,
            header,
        })
    }
}

impl ReferenceSnapshot {
    /// Validate before allocating expanded bodies. The file reader must ALSO cap
    /// input bytes before deserialization; this budget prevents graph expansion.
    pub fn restore(&self) -> Result<Vec<ObjectRef>> {
        self.restore_with_bindings(&[])
    }
    /// Rebind saved constant instances to an already loaded program's objects.
    /// The caller supplies (saved root index, current program reference) pairs.
    /// Entire payloads AND alias relationships must match before identities are
    /// reused; host-created instances remain fresh. This prevents future script
    /// constants from becoming different instances than their saved aliases.
    pub fn restore_with_bindings(&self, bindings: &[(u32, &ObjectRef)]) -> Result<Vec<ObjectRef>> {
        ensure!(self.version == 1, "unsupported reference snapshot version");
        ensure!(
            self.roots.len() <= MAX_ROOTS && self.inlines.len() <= MAX_INLINES,
            "snapshot table exceeds limit"
        );
        let mut validation = Validation {
            snapshot: self,
            budget: Budget::default(),
            path: vec![false; self.inlines.len()],
            used: vec![false; self.inlines.len()],
        };
        for root in &self.roots {
            validation.reference(root, 0)?;
        }
        ensure!(
            validation.used.iter().all(|used| *used),
            "unreachable inline snapshot object"
        );
        ensure!(bindings.len() <= MAX_ROOTS, "too many snapshot bindings");
        let mut identities = (0..self.inlines.len())
            .map(|_| InlineIdentity(Arc::new(())))
            .collect::<Vec<_>>();
        if !bindings.is_empty() {
            let mut expected = ReferenceSnapshotWriter::default();
            let mut actual = ReferenceSnapshotWriter::default();
            for (index, reference) in bindings {
                let root = self
                    .roots
                    .get(*index as usize)
                    .context("snapshot binding root out of range")?;
                expected.insert(reference)?;
                actual.insert(&self.restore_reference(root, &identities))?;
            }
            ensure!(
                expected.finish()? == actual.finish()?,
                "snapshot binding payload or aliases differ from loaded program"
            );
            let mut bound = vec![None; self.inlines.len()];
            let mut reverse = HashMap::new();
            for (index, reference) in bindings {
                self.bind_reference(
                    &self.roots[*index as usize],
                    reference,
                    &mut bound,
                    &mut reverse,
                )?;
            }
            for (identity, binding) in identities.iter_mut().zip(bound) {
                if let Some(binding) = binding {
                    *identity = binding;
                }
            }
        }
        Ok(self
            .roots
            .iter()
            .map(|root| self.restore_reference(root, &identities))
            .collect())
    }
    fn bind_reference(
        &self,
        saved: &SavedReference,
        native: &ObjectRef,
        bound: &mut [Option<InlineIdentity>],
        reverse: &mut HashMap<InlineIdentity, u32>,
    ) -> Result<()> {
        if let (Some(saved), Some(native)) = (&saved.owner, &native.owner) {
            self.bind_reference(saved, native, bound, reverse)?;
        }
        if let (Some(id), Some(native)) = (saved.inline, &native.inline) {
            let identity = native.identity();
            if let Some(previous) = &bound[id as usize] {
                ensure!(
                    *previous == identity,
                    "conflicting snapshot identity binding"
                );
            }
            if let Some(previous) = reverse.insert(identity.clone(), id) {
                ensure!(previous == id, "snapshot binding merges distinct instances");
            }
            bound[id as usize] = Some(identity);
            let body = &self.inlines[id as usize];
            for (saved, native) in body.references.iter().zip(&native.references) {
                self.bind_reference(saved, native, bound, reverse)?;
            }
            if let (Some(saved), Some(native)) = (&body.bitmap, &native.bitmap) {
                match (&saved.header, &native.header) {
                    (
                        SavedBitmapHeader::R6 {
                            image_properties: saved,
                            ..
                        },
                        BitmapHeader::R6(native),
                    ) => self.bind_reference(saved, &native.image_properties, bound, reverse)?,
                    (
                        SavedBitmapHeader::Rip {
                            image_properties: Some(saved),
                            ..
                        },
                        BitmapHeader::Rip(RipHeader {
                            image_properties: Some(native),
                            ..
                        }),
                    ) => self.bind_reference(saved, native, bound, reverse)?,
                    _ => {}
                }
            }
        }
        Ok(())
    }
    fn restore_reference(&self, r: &SavedReference, identities: &[InlineIdentity]) -> ObjectRef {
        ObjectRef {
            class: r.class.clone(),
            cache_index: r.cache_index,
            flags: r.flags,
            offset: r.offset,
            size: r.size,
            owner: r
                .owner
                .as_deref()
                .map(|o| Box::new(self.restore_reference(o, identities))),
            selector: r.selector,
            inline: r.inline.map(|id| {
                let body = &self.inlines[id as usize];
                InlineObject {
                    data: body.data.clone(),
                    references: body
                        .references
                        .iter()
                        .map(|r| self.restore_reference(r, identities))
                        .collect(),
                    bitmap: body
                        .bitmap
                        .as_ref()
                        .map(|b| self.restore_bitmap(b, identities)),
                    posture: body.posture.clone(),
                    motion: body.motion.clone(),
                    character_pose: body.character_pose.map(Box::new),
                    identity: identities[id as usize].clone(),
                }
            }),
        }
    }
    fn restore_bitmap(&self, b: &SavedBitmap, identities: &[InlineIdentity]) -> InlineBitmap {
        let header = match &b.header {
            SavedBitmapHeader::R6 {
                version,
                canvas_width,
                canvas_height,
                x,
                y,
                width,
                height,
                flags,
                depth,
                data_size,
                display_scale,
                auxiliary,
                image_properties,
            } => BitmapHeader::R6(R6Header {
                version: *version,
                canvas_width: *canvas_width,
                canvas_height: *canvas_height,
                x: *x,
                y: *y,
                width: *width,
                height: *height,
                flags: *flags,
                depth: *depth,
                data_size: *data_size,
                display_scale: *display_scale,
                auxiliary: *auxiliary,
                image_properties: Box::new(self.restore_reference(image_properties, identities)),
            }),
            SavedBitmapHeader::Rip {
                width,
                height,
                x,
                y,
                crop_width,
                crop_height,
                flags,
                info,
                data_size,
                modern,
                image_properties,
            } => BitmapHeader::Rip(RipHeader {
                width: *width,
                height: *height,
                x: *x,
                y: *y,
                crop_width: *crop_width,
                crop_height: *crop_height,
                flags: *flags,
                info: *info,
                data_size: *data_size,
                modern: *modern,
                image_properties: image_properties
                    .as_deref()
                    .map(|r| Box::new(self.restore_reference(r, identities))),
            }),
        };
        InlineBitmap {
            data_offset: b.data_offset,
            header,
        }
    }
}

struct Validation<'a> {
    snapshot: &'a ReferenceSnapshot,
    budget: Budget,
    path: Vec<bool>,
    used: Vec<bool>,
}
impl Validation<'_> {
    fn reference(&mut self, r: &SavedReference, depth: usize) -> Result<()> {
        self.budget.node(depth)?;
        validate_reference(&r.class, r.offset, r.size, r.inline.is_some())?;
        self.budget
            .bytes(r.class.as_ref().map_or(0, |c| c.name.len()))?;
        if let Some(owner) = &r.owner {
            self.reference(owner, depth + 1)?;
        }
        if let Some(id) = r.inline {
            let index = id as usize;
            let body = self
                .snapshot
                .inlines
                .get(index)
                .context("inline snapshot ID out of range")?;
            ensure!(!self.path[index], "cyclic inline snapshot");
            ensure!(
                r.class.as_ref() == Some(&body.class),
                "inline snapshot class mismatch"
            );
            self.path[index] = true;
            self.used[index] = true;
            self.budget.bytes(body.data.len())?;
            validate_payload(
                body.posture.as_deref(),
                body.motion.as_deref(),
                &mut self.budget,
            )?;
            for child in &body.references {
                self.reference(child, depth + 1)?;
            }
            if let Some(bitmap) = &body.bitmap {
                match &bitmap.header {
                    SavedBitmapHeader::R6 {
                        data_size,
                        width,
                        height,
                        display_scale,
                        image_properties,
                        ..
                    } => {
                        ensure!(display_scale.is_finite(), "nonfinite snapshot bitmap scale");
                        validate_bitmap(
                            bitmap.data_offset,
                            body.data.len(),
                            *data_size,
                            *width,
                            *height,
                        )?;
                        self.reference(image_properties, depth + 1)?;
                    }
                    SavedBitmapHeader::Rip {
                        data_size,
                        width,
                        height,
                        image_properties,
                        ..
                    } => {
                        validate_bitmap(
                            bitmap.data_offset,
                            body.data.len(),
                            *data_size,
                            *width,
                            *height,
                        )?;
                        if let Some(properties) = image_properties {
                            self.reference(properties, depth + 1)?;
                        }
                    }
                }
            }
            self.path[index] = false;
        }
        Ok(())
    }
}

fn validate_reference(
    class: &Option<ClassId>,
    offset: Option<u64>,
    size: Option<u32>,
    inline: bool,
) -> Result<()> {
    if let Some(class) = class {
        ensure!(
            !class.name.is_empty() && class.name.len() <= 1024,
            "invalid snapshot class name"
        );
    }
    if let (Some(offset), Some(size)) = (offset, size) {
        ensure!(
            offset.checked_add(u64::from(size)).is_some(),
            "snapshot extent overflows"
        );
    }
    ensure!(
        !inline || (class.is_some() && offset.is_none() && size.is_none()),
        "invalid inline snapshot extent/class"
    );
    Ok(())
}
fn validate_bitmap(
    offset: usize,
    data_len: usize,
    size: u32,
    width: u16,
    height: u16,
) -> Result<()> {
    ensure!(
        offset <= data_len && size as usize <= data_len - offset,
        "snapshot bitmap outside payload"
    );
    ensure!(
        u64::from(width) * u64::from(height) <= 16_777_216,
        "snapshot bitmap exceeds pixel limit"
    );
    Ok(())
}
fn validate_payload(
    posture: Option<&PostureFields>,
    motion: Option<&MotionCurve>,
    budget: &mut Budget,
) -> Result<()> {
    if let Some(p) = posture {
        ensure!(
            p.version <= 2 && p.scales.iter().all(|f| f.is_finite()),
            "invalid snapshot posture"
        );
        budget.bytes(std::mem::size_of::<PostureFields>())?;
    }
    if let Some(m) = motion {
        ensure!(
            m.rate.is_finite() && m.segments.len() <= 65_536,
            "invalid snapshot motion"
        );
        budget.bytes(m.segments.len() * std::mem::size_of::<crate::motion::MotionSegment>())?;
        for s in &m.segments {
            ensure!([2, 3].contains(&s.kind), "invalid snapshot motion segment");
            budget.bytes(
                s.controls
                    .len()
                    .checked_mul(12)
                    .context("snapshot control count overflow")?,
            )?;
            if let Some(crate::motion::TimeCurve::Bezier(knots)) = &s.time_curve {
                budget.bytes(
                    knots
                        .len()
                        .checked_mul(24)
                        .context("snapshot knot count overflow")?,
                )?;
                ensure!(
                    knots.iter().flatten().all(|f| f.is_finite()),
                    "nonfinite snapshot timing curve"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(name: &str) -> ObjectRef {
        ObjectRef::transient(ClassId {
            name: name.into(),
            schema: 1,
        })
    }
    fn identity(r: &ObjectRef) -> InlineIdentity {
        r.inline.as_ref().unwrap().identity()
    }
    fn snapshot(roots: &[ObjectRef]) -> ReferenceSnapshot {
        let mut writer = ReferenceSnapshotWriter::default();
        for (index, root) in roots.iter().enumerate() {
            assert_eq!(writer.insert(root).unwrap(), index as u32);
        }
        writer.finish().unwrap()
    }
    fn roundtrip(snapshot: &ReferenceSnapshot) -> ReferenceSnapshot {
        serde_json::from_slice(&serde_json::to_vec(snapshot).unwrap()).unwrap()
    }

    #[test]
    fn aliases_survive_serialization_without_process_addresses_or_fake_extents() {
        let actor = object("CGenericUso");
        let unrelated = object("CGenericUso");
        let mut owner = object("owner");
        owner
            .inline
            .as_mut()
            .unwrap()
            .references
            .push(actor.clone());
        owner.owner = Some(Box::new(actor.clone()));
        owner.selector = 77;
        owner.cache_index = Some(19);
        let saved = snapshot(&[actor.clone(), unrelated, owner]);
        assert_eq!(saved.inlines.len(), 3);
        let restored = roundtrip(&saved).restore().unwrap();
        let shared = identity(&restored[0]);
        assert_ne!(shared, identity(&actor));
        assert_ne!(shared, identity(&restored[1]));
        assert_eq!(shared, identity(restored[2].owner.as_ref().unwrap()));
        assert_eq!(
            shared,
            identity(&restored[2].inline.as_ref().unwrap().references[0])
        );
        assert_eq!(restored[2].selector, 77);
        assert_eq!(restored[2].cache_index, Some(19));
        assert!(restored[0].offset.is_none() && restored[0].size.is_none());
        assert!(
            restored[0]
                .inline
                .as_ref()
                .unwrap()
                .decode_bitmap()
                .is_err()
        );
        // Separate loads must not share any runtime instance.
        assert_ne!(shared, identity(&saved.restore().unwrap()[0]));
    }

    #[test]
    fn bitmap_header_properties_share_the_same_graph_as_regular_references() {
        let properties = object("properties");
        for rip in [false, true] {
            let mut image = object(if rip { "CRip008" } else { "Cr6Ti" });
            let body = image.inline.as_mut().unwrap();
            body.data = vec![0; 12];
            body.references.push(properties.clone());
            body.bitmap = Some(InlineBitmap {
                data_offset: 4,
                header: if rip {
                    BitmapHeader::Rip(RipHeader {
                        width: 1,
                        height: 1,
                        x: 0,
                        y: 0,
                        crop_width: 1,
                        crop_height: 1,
                        flags: 3,
                        info: [0; 7],
                        data_size: 8,
                        modern: true,
                        image_properties: Some(Box::new(properties.clone())),
                    })
                } else {
                    BitmapHeader::R6(R6Header {
                        version: 0,
                        canvas_width: 1,
                        canvas_height: 1,
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1,
                        flags: 0,
                        depth: 24,
                        data_size: 6,
                        display_scale: 1.0,
                        auxiliary: 0,
                        image_properties: Box::new(properties.clone()),
                    })
                },
            });
            let restored = roundtrip(&snapshot(&[image, properties.clone()]))
                .restore()
                .unwrap();
            let body = restored[0].inline.as_ref().unwrap();
            let header_property = match &body.bitmap.as_ref().unwrap().header {
                BitmapHeader::R6(h) => &*h.image_properties,
                BitmapHeader::Rip(h) => h.image_properties.as_deref().unwrap(),
            };
            assert_eq!(identity(&restored[1]), identity(header_property));
            assert_eq!(identity(&restored[1]), identity(&body.references[0]));
            assert_eq!(body.bitmap.as_ref().unwrap().data_offset, 4);
            assert_eq!(body.data, vec![0; 12]);
        }
    }

    #[test]
    fn loaded_program_bindings_preserve_script_constants_without_reusing_host_instances() {
        let constant = object("CPostureMoment");
        let host_actor = object("CGenericUso");
        let mut container = object("container");
        container.inline.as_mut().unwrap().references = vec![constant.clone(), host_actor.clone()];
        let saved = roundtrip(&snapshot(&[
            container,
            constant.clone(),
            host_actor.clone(),
        ]));
        // Represents a freshly loaded program with a different allocation.
        let new_constant = object("CPostureMoment");
        let restored = saved.restore_with_bindings(&[(1, &new_constant)]).unwrap();
        assert_eq!(identity(&restored[1]), identity(&new_constant));
        assert_ne!(identity(&restored[1]), identity(&constant));
        assert_ne!(identity(&restored[2]), identity(&host_actor));
        let references = &restored[0].inline.as_ref().unwrap().references;
        assert_eq!(identity(&references[0]), identity(&new_constant));
        assert_eq!(identity(&references[1]), identity(&restored[2]));
        let mut different = new_constant.clone();
        different.inline.as_mut().unwrap().data.push(1);
        assert!(saved.restore_with_bindings(&[(1, &different)]).is_err());
        assert!(
            saved
                .restore_with_bindings(&[(100, &new_constant)])
                .is_err()
        );
        assert!(
            saved
                .restore_with_bindings(&[(1, &new_constant), (2, &new_constant)])
                .is_err()
        );
    }

    #[test]
    fn bindings_reject_equal_bytes_with_different_alias_relationships() {
        let first = object("constant");
        let second = object("constant");
        let saved = snapshot(&[first.clone(), second]);
        assert!(
            saved
                .restore_with_bindings(&[(0, &first), (1, &first)])
                .is_err()
        );
        let shared = snapshot(&[first.clone(), first.clone()]);
        let unrelated = object("constant");
        assert!(
            shared
                .restore_with_bindings(&[(0, &first), (1, &unrelated)])
                .is_err()
        );
    }

    #[test]
    fn conflicting_clones_poison_the_writer_instead_of_merging_instances() {
        let first = object("CGenericUso");
        let mut second = first.clone();
        second.inline.as_mut().unwrap().data.push(1);
        let mut writer = ReferenceSnapshotWriter::default();
        writer.insert(&first).unwrap();
        assert!(
            writer
                .insert(&second)
                .unwrap_err()
                .to_string()
                .contains("conflicting payload")
        );
        assert!(writer.insert(&first).is_err());
        assert!(writer.finish().is_err());
    }

    #[test]
    fn null_and_stored_parameter_metadata_are_not_reinterpreted() {
        let null = ObjectRef {
            class: None,
            cache_index: Some(0),
            flags: 0,
            offset: None,
            size: None,
            owner: None,
            selector: 0,
            inline: None,
        };
        let stored = ObjectRef {
            class: Some(ClassId {
                name: "CVmVarObj".into(),
                schema: 1,
            }),
            cache_index: Some(99),
            flags: 0x8007,
            offset: Some(0x12340),
            size: Some(37),
            owner: Some(Box::new(null.clone())),
            selector: 4,
            inline: None,
        };
        let restored = roundtrip(&snapshot(&[null, stored.clone()]))
            .restore()
            .unwrap();
        assert_eq!(
            serde_json::to_value(&restored[1]).unwrap(),
            serde_json::to_value(&stored).unwrap()
        );
        assert_eq!(restored[0].cache_index, Some(0));
        assert!(restored[0].class.is_none() && restored[0].inline.is_none());
    }

    #[test]
    fn corrupted_tables_cycles_classes_versions_and_extents_are_rejected() {
        let actor = object("CGenericUso");
        let baseline = snapshot(std::slice::from_ref(&actor));
        let mut bad = roundtrip(&baseline);
        bad.roots[0].inline = Some(900);
        assert!(bad.restore().is_err());
        let mut bad = roundtrip(&baseline);
        bad.inlines[0].references.push(bad.roots[0].clone());
        assert!(bad.restore().unwrap_err().to_string().contains("cyclic"));
        let mut bad = roundtrip(&baseline);
        bad.roots[0].class.as_mut().unwrap().schema = 77;
        assert!(bad.restore().is_err());
        let mut bad = roundtrip(&baseline);
        bad.roots[0].inline = None;
        assert!(
            bad.restore()
                .unwrap_err()
                .to_string()
                .contains("unreachable")
        );
        let mut bad = roundtrip(&baseline);
        bad.roots[0].class = None;
        assert!(bad.restore().is_err());
        let mut bad = roundtrip(&baseline);
        bad.version = 2;
        assert!(bad.restore().is_err());
        let mut bad = roundtrip(&baseline);
        bad.roots[0].inline = None;
        bad.roots[0].offset = Some(u64::MAX);
        bad.roots[0].size = Some(1);
        assert!(bad.restore().unwrap_err().to_string().contains("overflows"));
        let mut too_deep = actor;
        for _ in 0..MAX_DEPTH {
            let mut parent = object("parent");
            parent.owner = Some(Box::new(too_deep));
            too_deep = parent;
        }
        let mut writer = ReferenceSnapshotWriter::default();
        assert!(writer.insert(&too_deep).is_err());
    }

    #[test]
    fn small_alias_dag_cannot_expand_exponentially_during_restore() {
        let objects = (0..21).map(|_| object("node")).collect::<Vec<_>>();
        let mut saved = snapshot(&objects);
        for index in 1..saved.inlines.len() {
            let child = saved.roots[index - 1].clone();
            saved.inlines[index].references = vec![child.clone(), child];
        }
        saved.roots = vec![saved.roots.last().unwrap().clone()];
        assert!(
            saved
                .restore()
                .unwrap_err()
                .to_string()
                .contains("count exceeds")
        );
    }

    #[test]
    fn payload_budget_counts_alias_expansion_and_rejects_nonfinite_motion() {
        let mut budget = Budget {
            nodes: 0,
            bytes: MAX_BYTES - 1,
        };
        assert!(budget.bytes(2).is_err());
        let mut actor = object("CMoveAcsOngen");
        actor.inline.as_mut().unwrap().motion = Some(Box::new(MotionCurve {
            start: [0; 3],
            segments: Vec::new(),
            timing_mode: -1,
            duration_ms: 0,
            rate: f32::NAN,
            dimensions: 3,
            flags: 0,
        }));
        assert!(ReferenceSnapshotWriter::default().insert(&actor).is_err());
        actor.inline.as_mut().unwrap().motion.as_mut().unwrap().rate = 1.0;
        let restored = roundtrip(&snapshot(&[actor])).restore().unwrap();
        assert_eq!(
            restored[0]
                .inline
                .as_ref()
                .unwrap()
                .motion
                .as_ref()
                .unwrap()
                .rate,
            1.0
        );
    }
}
