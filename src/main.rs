use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rugp_assets::RioArchive;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Parser)]
#[command(about = "Native Rust rUGP archive and engine tools", version)]
struct Args {
    #[arg(long, default_value = ".", global = true)]
    project_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect the install manifest, split volumes, and project classes.
    Inspect,
    /// List serialized project objects.
    List {
        #[arg(long)]
        class: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Write one object's original stored bytes (fails if the output exists).
    Extract { id: usize, output: PathBuf },
    /// Decode a scene's serialized VM instructions.
    Disasm { id: usize },
    /// Validate every script and report unsupported serialized classes.
    Check {
        #[arg(long)]
        verbose: bool,
    },
}

fn main() -> Result<()> {
    let args = Args::parse();
    let archive = RioArchive::open(&args.project_dir)?;
    match args.command {
        Command::Inspect => {
            println!(
                "{}\nICI version: {}\nTOC: {:#x}, {} bytes\nAddress shift: {}\nVolumes: {} ({} bytes)\nObjects: {}\nDecrypted project: {} bytes",
                archive.manifest.title,
                archive.manifest.version,
                archive.manifest.toc_offset,
                archive.manifest.toc_size,
                archive.address_shift,
                archive.volumes.len(),
                archive.len(),
                archive.nodes.len(),
                archive.project_data.len()
            );
            let mut classes = BTreeMap::new();
            for node in &archive.nodes {
                *classes.entry(&node.class.name).or_insert(0usize) += 1;
            }
            for (class, count) in classes {
                println!("{count:6}  {class}");
            }
        }
        Command::List { class, json } => {
            let nodes: Vec<_> = archive
                .nodes
                .iter()
                .filter(|n| class.as_ref().is_none_or(|c| n.class.name == *c))
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&nodes)?);
            } else {
                for n in nodes {
                    println!(
                        "{:6} {:12x} {:10} {:4} {}",
                        n.id,
                        n.offset.unwrap_or(0),
                        n.size.unwrap_or(0),
                        n.class.schema,
                        n.class.name
                    );
                }
            }
        }
        Command::Extract { id, output } => {
            use std::io::Write;
            let bytes = archive.read_node(id)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
                .with_context(|| format!("create {}", output.display()))?;
            file.write_all(&bytes)?;
            println!("Wrote {} bytes to {}", bytes.len(), output.display());
        }
        Command::Disasm { id } => {
            let root = archive.volumes[0]
                .path
                .parent()
                .context("game root missing")?;
            let native = rugp_assets::native_schema::NativeSchema::from_directory(root)?;
            let script = rugp_assets::script::Script::load(&archive, id)?;
            let scene = rugp_scene_vm::Scenario::decode_with_schema(
                &script,
                archive.address_shift,
                Some(&native),
            )?;
            println!("{}", serde_json::to_string_pretty(&scene)?);
        }
        Command::Check { verbose } => {
            let root = archive.volumes[0]
                .path
                .parent()
                .context("game root missing")?;
            let native = rugp_assets::native_schema::NativeSchema::from_directory(root)?;
            let mut ok = 0;
            let mut failures = BTreeMap::new();
            for node in archive.nodes.iter().filter(|n| n.class.name == "CRsa") {
                match rugp_assets::script::Script::load(&archive, node.id).and_then(|s| {
                    rugp_scene_vm::Scenario::decode_with_schema(
                        &s,
                        archive.address_shift,
                        Some(&native),
                    )
                }) {
                    Ok(scene) => {
                        ok += 1;
                        if scene.trailing_bytes != 0 {
                            println!("Scene {}: {} trailing bytes", node.id, scene.trailing_bytes);
                        }
                    }
                    Err(e) => {
                        if verbose {
                            println!("Scene {}: {e:#}", node.id);
                        }
                        *failures.entry(e.root_cause().to_string()).or_insert(0) += 1;
                    }
                }
            }
            println!(
                "Decoded {ok} scenes; {} failed",
                failures.values().sum::<usize>()
            );
            for (reason, count) in &failures {
                println!("{count:5} {reason}");
            }
            anyhow::ensure!(failures.is_empty(), "some scenes could not be decoded");
        }
    }
    Ok(())
}
