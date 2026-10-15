use anyhow::{Result, ensure};
use rugp_assets::{RioArchive, native_schema::NativeSchema, script::Script};
use rugp_scene_vm::Scenario;

/// Uses the user's own game; no retail assets or dialogue are committed.
#[test]
#[ignore = "set RUGP_TEST_GAME to a KIMINOZO_LE game directory"]
fn kiminozo_le_all_project_scenes() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME")
        .ok_or_else(|| anyhow::anyhow!("RUGP_TEST_GAME missing"))?;
    let root = std::path::Path::new(&root);
    let archive = RioArchive::open(root)?;
    ensure!(archive.nodes.len() == 666, "unexpected game build");
    let native = NativeSchema::from_directory(root)?;
    ensure!(native.messages.len() >= 262, "incomplete native schema");
    let mut scenes = 0;
    let mut instructions = 0;
    for node in archive.nodes.iter().filter(|n| n.class.name == "CRsa") {
        let script = Script::load(&archive, node.id)?;
        let scene = Scenario::decode_with_schema(&script, archive.address_shift, Some(&native))?;
        ensure!(
            scene.null_records.is_empty(),
            "unexpected null records in scene {}",
            node.id
        );
        ensure!(scene.trailing_bytes == 0, "unparsed scene bytes");
        instructions += scene.instructions.len();
        scenes += 1;
    }
    ensure!(scenes == 283, "unexpected scene count {scenes}");
    println!("Validated {scenes} scenes and {instructions} instructions");
    Ok(())
}
