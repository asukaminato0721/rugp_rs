//! Inline posture/motion resource metadata only: no dialogue or sample arrays.
use anyhow::{Context, Result};
use rugp_assets::{RioArchive, native_schema::NativeSchema, posture::Posture};
use rugp_scene_vm::vm::Program;
use std::collections::BTreeMap;

fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("game root")?;
    let archive = RioArchive::open(&root)?;
    let native = NativeSchema::from_directory(root.as_ref())?;
    let program = Program::load(&archive, &native)?;
    let mut counts = BTreeMap::new();
    let mut fields = BTreeMap::new();
    let mut failure = None;
    for scene in program.scenes.values() {
        scene.visit_references(&mut |reference| {
            if reference
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CPostureMoment")
                && let Some(inline) = &reference.inline
            {
                match Posture::from_inline(inline, reference.class.as_ref().unwrap()) {
                    Ok(posture) => {
                        let f = posture.fields;
                        *fields.entry(format!(
                            "version {}, state {}, options {}, modes [{}, {}, {}], scales {:?}, elapsed {}",
                            f.version, f.state, f.options, f.position_mode, f.angle_mode,
                            f.other_mode, f.scales, f.elapsed_ms
                        )).or_insert(0u32) += 1;
                    }
                    Err(error) => failure = Some(error),
                }
                let references: Vec<_> = inline
                    .references
                    .iter()
                    .map(|r| {
                        r.class.as_ref().map_or("null".to_owned(), |c| {
                            format!(
                                "{}#{}{}",
                                c.name,
                                c.schema,
                                if r.inline.is_some() { " inline" } else { "" }
                            )
                        })
                    })
                    .collect();
                *counts
                    .entry(format!(
                        "header {:?}, {} bytes, refs {references:?}",
                        inline.data.get(..3),
                        inline.data.len()
                    ))
                    .or_insert(0u32) += 1;
            }
        });
    }
    if let Some(error) = failure {
        return Err(error);
    }
    for (metadata, count) in counts {
        println!("{count:6} {metadata}");
    }
    for (metadata, count) in fields {
        println!("{count:6} {metadata}");
    }
    Ok(())
}
