# rugp_rs

Native Rust reimplementation of rUGP, targeting the 2008 **君が望む永遠
～Latest Edition～** disc. This is an active port: archive and scene decoding
work, along with Cr6Ti/CRip007/CRip008, CRmt mipmaps, CBg2d image planes,
and OOR/Vorbis audio decoding. A first vertical slice runs the real startup VM
through logo loading, timed waits, and crossfades into a software framebuffer.
The full game is **not yet playable**: 3D rendering, effects, audio-device
playback, interactive UI, movies, and save/load remain incomplete.

The workspace follows `../siglus_rs`'s separation between asset formats,
scene VM, and application/platform hosts:

- `crates/rugp_assets`: bounded binary input, ICI manifests and checksums,
  split RIO reads, project/object references, inline objects, encrypted CRsa
  payloads, texture/background/audio decoding, and static native serialization
  metadata recovery.
- `crates/rugp_scene_vm`: typed scene records, operation fields, dialogue,
  literals, effects, resource references, and a host-independent VM core with
  linked addresses, scoped call arguments, calls/returns, labels, scalar state,
  integer branches, and native message-field overrides.
  Display/audio/UI operations yield until a host explicitly handles them.
- `crates/rugp_runtime`: game/object state, bounded resource caches, software
  RGBA composition, and initial startup-message handling, independent of a
  window or audio device.
- Root `rugp_rs` binary: inspection, extraction, disassembly, and validation.
  Its optional desktop host uses winit and presents the same RGBA buffer.

No Windows executable or DLL is loaded or executed. The schema reader reads
PE data, imports/exports, and constant table initialization instructions from
the game's `UnivUI.dll`, `rvmm.dll`, and `Vm60.dll`. It does not emulate the CPU.

## Commands

```sh
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE inspect
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE list --class CRsa
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE disasm 21
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE check --verbose
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE extract 21 scene.bin
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE resources
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE resources --operations
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE image 0xcf1e89 logo.png
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE check-images
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE audio 0xf4800c9 voice.wav
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE check-audio
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE check-audio --decode
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE frame startup.png --time-ms 1000
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE play --mute
```

Pass a game directory or its primary `.rio` file. All inputs remain untouched;
extraction refuses to overwrite an existing output. Objects are addressed by
their project-tree ID. Disassembly emits JSON with exact stream offsets,
in-memory command offsets, operation flags, and references.
Use `--no-default-features` for the CLI without desktop/window dependencies.
`play` is a development runner, not a completed game launcher; unsupported
operations stop at their exact scene/address. `--mute` explicitly disables
audio playback; PCM export/decoding works independently.

## Validation

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
RUGP_TEST_GAME=/run/media/w/KIMINOZO_LE cargo test -p rugp_scene_vm --test game -- --ignored --nocapture
RUGP_TEST_GAME=/run/media/w/KIMINOZO_LE cargo test -p rugp_assets --test game_audio -- --ignored --nocapture
RUGP_TEST_GAME=/run/media/w/KIMINOZO_LE cargo test -p rugp_runtime --test game -- --ignored --nocapture
```

The disc contains two volumes totaling 3,028,167,410 bytes and 666 project
objects. All 283 project CRsa scenes decode sequentially with checksums and
without trailing bytes (249,234 instructions). The reference index contains
34,993 unique resources and 100,923 dialogue records. All 951 directly
referenced Cr6Ti/CRip007/CRip008 bitmaps decode, as do all 9,948 mip levels in
1,658 referenced CRmt texture containers. Another 298 image planes decode
inside the 321 CBg2d background containers (11,197 bitmaps total). Two
three-byte CRmt placeholders are not textures. CRmt parent trailers (mask,
motion, and extra properties) and CBg2d's extra metadata trailers are retained
but not fully interpreted yet. Images
reachable only through other asset objects are not included in this count.
Unsupported formats return errors with class and byte
offset; the parser never searches forward to guess instruction boundaries.

OOR resources are read as complete audio streams, not prefixed RIO objects.
The decoder reassembles split Vorbis packets, reconstructs identification
headers, and obtains standard libvorbis setup packets from the user's own
`Plugins/riooor.rpo`, verified by SHA-1. No original DLL code is executed.
Two retail voice extents have 31/11 bytes after logical EOS; these tails are
retained explicitly. WAV export and per-packet PCM decoding use pure-Rust
`lewton`. V0 audio has no final sample count, so encoder end padding is retained.
All 30,297 directly scene-referenced audio streams pass framing validation and decode to PCM;
the two EOS-tail cases also have dedicated retail regression tests.
`check-audio` additionally follows CPmBgm/CPmEffect source wrappers, including
inline setup headers and BGM header continuations.

The VM's retail startup regression resolves its entry through the process
ocean, executes the initial scalar assignments, and follows the jump from
scene 21 into scene 252 and binds the first logo helper's image/camera arguments.
This test deliberately acknowledges host requests;
it does not claim game/UI execution. Object-property binding and the
rendering/application host are still under development.

The first-logo runtime regression executes the original startup messages and
checks that the fully faded 1024x600 frame equals the decoded source pixels.
Desktop presentation and automatic exit have also been exercised under Xvfb.

See [THIRD_PARTY.md](THIRD_PARTY.md) for format research and attribution. Game
assets, original binaries, extracted scripts, and dialogue are not included.
