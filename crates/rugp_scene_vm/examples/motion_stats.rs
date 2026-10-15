//! Aggregate motion metadata only; does not print coordinates or sample arrays.
use anyhow::{Context, Result};
use rugp_assets::{
    RioArchive,
    motion::{MotionCurve, MotionResource, TimeCurve},
    native_schema::NativeSchema,
};
use rugp_scene_vm::{resources::ResourceIndex, vm::Program};
use std::collections::BTreeMap;

fn count(curve: &MotionCurve, counts: &mut BTreeMap<String, u32>) {
    *counts
        .entry(format!(
            "mode {}, dimensions {}, flags {:#x}, rate {}",
            curve.timing_mode, curve.dimensions, curve.flags, curve.rate
        ))
        .or_default() += 1;
    for segment in &curve.segments {
        let time = match &segment.time_curve {
            None => "linear".into(),
            Some(TimeCurve::Quadratic(value)) => format!("quadratic {value}"),
            Some(TimeCurve::Bezier(knots)) => format!("Bezier {} knots", knots.len()),
        };
        *counts
            .entry(format!(
                "kind {}, options {}, controls {}, time {time}",
                segment.kind,
                segment.options,
                segment.controls.len()
            ))
            .or_default() += 1;
    }
}
fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game root")?;
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let program = Program::load(&archive, &native)?;
    let mut counts = BTreeMap::new();
    let mut inline_count = 0;
    let mut failure = None;
    for scene in program.scenes.values() {
        scene.visit_references(&mut |reference| {
            if let Some(class) = &reference.class
                && class.name == "CMoveAcsOngen"
                && let Some(inline) = &reference.inline
            {
                match MotionCurve::from_inline(inline, class) {
                    Ok(curve) => {
                        inline_count += 1;
                        count(&curve, &mut counts);
                    }
                    Err(error) => failure = Some(error),
                }
            }
        });
    }
    if let Some(error) = failure {
        return Err(error);
    }
    let index = ResourceIndex::scan(&archive, &native)?;
    let mut external_count = 0;
    for resource in index
        .resources
        .values()
        .filter(|r| r.class.name == "CMoveAcsOngen")
    {
        let bytes = index.read(&archive, resource.offset)?;
        let motion = MotionResource::parse(&bytes, &resource.class)
            .with_context(|| format!("motion at {}", resource.offset))?;
        *counts
            .entry(format!(
                "external trailer {} bytes",
                motion.trailing_data.len()
            ))
            .or_default() += 1;
        external_count += 1;
        count(&motion.curve, &mut counts);
    }
    println!("{inline_count} inline records, {external_count} external resources");
    for (metadata, count) in counts {
        println!("{count:6} {metadata}");
    }
    Ok(())
}
