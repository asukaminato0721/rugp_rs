use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rugp_assets::RioArchive;
use std::{collections::BTreeMap, path::PathBuf};
#[cfg(all(
    feature = "desktop",
    any(target_os = "linux", target_os = "windows", target_os = "macos")
))]
mod desktop;

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
    /// Run the work-in-progress desktop runtime; unsupported operations stop with an error.
    #[cfg(all(
        feature = "desktop",
        any(target_os = "linux", target_os = "windows", target_os = "macos")
    ))]
    Play {
        #[arg(long)]
        mute: bool,
        #[arg(long)]
        quit_after_ms: Option<u32>,
    },
    /// Run the current runtime headlessly and export its RGBA frame (development diagnostic).
    Frame {
        output: PathBuf,
        #[arg(long, default_value_t = 1000)]
        time_ms: u32,
    },
    /// Validate OOR audio framing and setup headers; optionally decode every packet.
    CheckAudio {
        #[arg(long)]
        decode: bool,
        /// Only sources discovered inside sound wrappers, excluding direct scene references.
        #[arg(long)]
        additional_only: bool,
    },
    /// Decode one referenced OOR stream to 16-bit PCM WAV.
    Audio {
        #[arg(value_parser = parse_offset)]
        offset: u64,
        output: PathBuf,
    },
    /// Decode all referenced bitmaps and report format coverage.
    CheckImages,
    /// Decode a referenced bitmap to PNG (offset accepts decimal or 0x-prefixed hex).
    Image {
        #[arg(value_parser = parse_offset)]
        offset: u64,
        output: PathBuf,
    },
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
    /// Inventory resources referenced by every scene.
    Resources {
        #[arg(long)]
        class: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        operations: bool,
    },
}

fn main() -> Result<()> {
    let args = Args::parse();
    let archive = RioArchive::open(&args.project_dir)?;
    let game_root = archive.volumes[0]
        .path
        .parent()
        .context("game root missing")?;
    match args.command {
        #[cfg(all(
            feature = "desktop",
            any(target_os = "linux", target_os = "windows", target_os = "macos")
        ))]
        Command::Play {
            mute,
            quit_after_ms,
        } => desktop::run(
            rugp_runtime::Game::open(&args.project_dir)?,
            mute,
            quit_after_ms,
        )?,
        Command::Frame { output, time_ms } => {
            let mut game = rugp_runtime::Game::open(&args.project_dir)?;
            game.step(0)?;
            let mut elapsed = 0;
            while elapsed < time_ms {
                let dt = (time_ms - elapsed).min(16);
                game.step(dt)?;
                elapsed += dt;
            }
            let (width, height) = game.size();
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)?;
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header()?.write_image_data(game.frame())?;
            println!(
                "Rendered {elapsed} ms at PC {:?} to {} (audio requests not played)",
                game.vm.cursor(),
                output.display()
            );
        }
        Command::CheckAudio {
            decode,
            additional_only,
        } => {
            let native = rugp_assets::native_schema::NativeSchema::from_directory(game_root)?;
            let mut index = rugp_scene_vm::resources::ResourceIndex::scan(&archive, &native)?;
            let direct: std::collections::BTreeSet<_> = index.resources.keys().copied().collect();
            index.expand_sound_sources(&archive)?;
            let plugin = std::fs::read(game_root.join("Plugins/riooor.rpo"))?;
            let headers = rugp_assets::oor::SetupHeaders::from_plugin(&plugin);
            let mut ok = 0;
            let mut failures = BTreeMap::new();
            let mut frames = 0u64;
            for resource in index.resources.values().filter(|r| {
                r.class.name == "COptimizedObs" && (!additional_only || !direct.contains(&r.offset))
            }) {
                let result = (|| -> Result<()> {
                    let bytes = archive.read_at(resource.offset, resource.size as usize)?;
                    let stream = rugp_assets::oor::Oor::parse(&bytes)?;
                    let mut decoder = rugp_assets::oor::VorbisDecoder::new(stream, &headers)?;
                    if decode {
                        while let Some(samples) = decoder.next_samples()? {
                            frames += samples.len() as u64 / decoder.info.channels as u64;
                        }
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => ok += 1,
                    Err(error) => {
                        eprintln!("Audio {:#x}: {error:#}", resource.offset);
                        *failures.entry(format!("{error:#}")).or_insert(0usize) += 1;
                    }
                }
                if (ok + failures.values().sum::<usize>()) % 1000 == 0 {
                    eprintln!(
                        "Checked {} audio streams",
                        ok + failures.values().sum::<usize>()
                    );
                }
            }
            println!(
                "Validated {ok} audio streams; {} failed; {frames} decoded sample frames",
                failures.values().sum::<usize>()
            );
            for (reason, count) in &failures {
                println!("{count:8} {reason}");
            }
            anyhow::ensure!(failures.is_empty(), "some audio streams failed");
        }
        Command::Audio { offset, output } => {
            use std::io::Write;
            let native = rugp_assets::native_schema::NativeSchema::from_directory(game_root)?;
            let index = rugp_scene_vm::resources::ResourceIndex::scan(&archive, &native)?;
            let resource = index
                .resources
                .get(&offset)
                .context("unknown audio resource")?;
            anyhow::ensure!(
                resource.class.name == "COptimizedObs",
                "resource is not OOR audio"
            );
            let bytes = index.read(&archive, offset)?;
            let stream = rugp_assets::oor::Oor::parse(&bytes)?;
            let headers = rugp_assets::oor::SetupHeaders::from_plugin(&std::fs::read(
                game_root.join("Plugins/riooor.rpo"),
            )?);
            let mut decoder = rugp_assets::oor::VorbisDecoder::new(stream, &headers)?;
            let mut pcm = Vec::new();
            while let Some(samples) = decoder.next_samples()? {
                anyhow::ensure!(
                    pcm.len() + samples.len() * 2 <= 512 * 1024 * 1024,
                    "WAV exceeds export memory limit"
                );
                pcm.extend(samples.into_iter().flat_map(i16::to_le_bytes));
            }
            let mut file = std::io::BufWriter::new(
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&output)?,
            );
            let rate = decoder.info.sample_rate;
            let channels = decoder.info.channels as u16;
            file.write_all(b"RIFF")?;
            file.write_all(&(36 + pcm.len() as u32).to_le_bytes())?;
            file.write_all(b"WAVEfmt ")?;
            file.write_all(&16u32.to_le_bytes())?;
            file.write_all(&1u16.to_le_bytes())?;
            file.write_all(&channels.to_le_bytes())?;
            file.write_all(&rate.to_le_bytes())?;
            file.write_all(&(rate * channels as u32 * 2).to_le_bytes())?;
            file.write_all(&(channels * 2).to_le_bytes())?;
            file.write_all(&16u16.to_le_bytes())?;
            file.write_all(b"data")?;
            file.write_all(&(pcm.len() as u32).to_le_bytes())?;
            file.write_all(&pcm)?;
            file.flush()?;
            println!(
                "Decoded {} Hz, {channels} channels, {:.3} seconds to {}",
                rate,
                pcm.len() as f64 / (rate as f64 * channels as f64 * 2.0),
                output.display()
            );
        }
        Command::Image { offset, output } => {
            let root = archive.volumes[0]
                .path
                .parent()
                .context("game root missing")?;
            let native = rugp_assets::native_schema::NativeSchema::from_directory(root)?;
            let index = rugp_scene_vm::resources::ResourceIndex::scan(&archive, &native)?;
            let resource = index
                .resources
                .get(&offset)
                .context("unknown resource offset")?;
            let decoded = decode_image(&archive, resource)?;
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)?;
            let mut encoder =
                png::Encoder::new(std::io::BufWriter::new(file), decoded.width, decoded.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header()?.write_image_data(&decoded.rgba)?;
            println!(
                "Decoded {}x{} bitmap to {}",
                decoded.width,
                decoded.height,
                output.display()
            );
        }
        Command::CheckImages => {
            let root = archive.volumes[0]
                .path
                .parent()
                .context("game root missing")?;
            let native = rugp_assets::native_schema::NativeSchema::from_directory(root)?;
            let index = rugp_scene_vm::resources::ResourceIndex::scan(&archive, &native)?;
            let mut ok = 0;
            let mut failures = BTreeMap::new();
            for resource in index.resources.values().filter(|r| {
                ["Cr6Ti", "CRip007", "CRip008", "CRmt", "CBg2d"].contains(&r.class.name.as_str())
                    && r.size > 3
            }) {
                let result = if resource.class.name == "CRmt" {
                    archive
                        .read_at(resource.offset, resource.size as usize)
                        .and_then(|bytes| {
                            let container =
                                rugp_assets::crmt::Crmt::parse(&bytes, archive.address_shift)?;
                            for (i, level) in container.levels.iter().enumerate() {
                                level.decode().with_context(|| format!("mip {i}"))?;
                            }
                            Ok(container.levels.len())
                        })
                } else if resource.class.name == "CBg2d" {
                    archive
                        .read_at(resource.offset, resource.size as usize)
                        .and_then(|bytes| {
                            let bg = rugp_assets::background::Background::parse(
                                &bytes,
                                resource.class.schema,
                                archive.address_shift,
                            )?;
                            for plane in &bg.planes {
                                plane
                                    .image
                                    .inline
                                    .as_ref()
                                    .context("external background image not handled by checker")?
                                    .decode_bitmap()?;
                            }
                            Ok(bg.planes.len())
                        })
                } else {
                    decode_image(&archive, resource).map(|_| 1)
                };
                match result {
                    Ok(count) => {
                        ok += count;
                    }
                    Err(error) => {
                        *failures
                            .entry(format!("{}: {error:#}", resource.class.name))
                            .or_insert(0usize) += 1;
                    }
                }
            }
            println!(
                "Decoded {ok} bitmaps; {} failed",
                failures.values().sum::<usize>()
            );
            for (reason, count) in &failures {
                println!("{count:8} {reason}");
            }
            anyhow::ensure!(failures.is_empty(), "some bitmaps could not be decoded");
        }
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
        Command::Resources {
            class,
            json,
            operations,
        } => {
            let root = archive.volumes[0]
                .path
                .parent()
                .context("game root missing")?;
            let native = rugp_assets::native_schema::NativeSchema::from_directory(root)?;
            let index = rugp_scene_vm::resources::ResourceIndex::scan(&archive, &native)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&index)?);
            } else if operations {
                for (name, count) in &index.instructions {
                    println!("{count:8} {name}");
                }
                for (name, count) in &index.messages {
                    println!("{count:8} {name}");
                }
            } else if let Some(class) = class {
                for resource in index.resources.values().filter(|r| r.class.name == class) {
                    println!(
                        "{:12x} {:10} {}#{}",
                        resource.offset, resource.size, resource.class.name, resource.class.schema
                    );
                }
            } else {
                let mut classes = BTreeMap::new();
                for resource in index.resources.values() {
                    *classes.entry(&resource.class.name).or_insert(0usize) += 1;
                }
                println!(
                    "{} unique resources, {} dialogue records",
                    index.resources.len(),
                    index.dialogue_count
                );
                for (name, count) in classes {
                    println!("{count:8} {name}");
                }
            }
        }
    }
    Ok(())
}

fn parse_offset(value: &str) -> std::result::Result<u64, std::num::ParseIntError> {
    if let Some(hex) = value.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else {
        value.parse()
    }
}

fn decode_image(
    archive: &RioArchive,
    resource: &rugp_scene_vm::resources::Resource,
) -> Result<rugp_assets::image::Image> {
    let bytes = archive.read_at(resource.offset, resource.size as usize)?;
    match resource.class.name.as_str() {
        "Cr6Ti" => rugp_assets::image::decode_r6ti(&bytes, archive.address_shift),
        "CRip007" => {
            rugp_assets::rip::decode_rip007(&bytes, resource.class.schema, archive.address_shift)
        }
        "CRip008" => {
            rugp_assets::rip::decode_rip008(&bytes, resource.class.schema, archive.address_shift)
        }
        "CRmt" => rugp_assets::crmt::Crmt::parse(&bytes, archive.address_shift)?
            .levels
            .first()
            .context("CRmt has no mipmaps")?
            .decode(),
        "CBg2d" => rugp_assets::background::Background::parse(
            &bytes,
            resource.class.schema,
            archive.address_shift,
        )?
        .planes
        .first()
        .context("background has no image planes")?
        .image
        .inline
        .as_ref()
        .context("background plane is not inline")?
        .decode_bitmap(),
        other => anyhow::bail!("unsupported bitmap class {other}"),
    }
}
