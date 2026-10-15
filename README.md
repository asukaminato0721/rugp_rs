# rugp_rs

Native Rust reimplementation of rUGP, targeting the 2008 **君が望む永遠
～Latest Edition～** disc. This is an active port: archive and scene decoding
work, along with Cr6Ti/CRip007/CRip008, CRmt mipmaps, CBg2d image planes,
and OOR/Vorbis audio decoding. A first vertical slice runs the real startup VM
through logo loading, timed waits, and crossfades into a software framebuffer.
The full game is **not yet playable**: 3D rendering, effects, interactive UI,
and save/load remain incomplete. An optional Kira audio-device adapter
handles ordinary sample playback; specialized sound operations remain incomplete.

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
  RGBA composition, archive-backed movie playback, and startup-message handling, independent of a
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
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE check-menus --decode
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE audio 0xf4800c9 voice.wav
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE check-audio
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE check-audio --decode
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE check-video --decode
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE video-frame 0xd0ba5f 200 movie.png
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

For device audio, enable `--features audio`. On Linux this needs ALSA development
files. The pinned Nix development shell supplies the audio/window dependencies:

```sh
nix develop
cargo run --release --features audio -- --project-dir /run/media/w/KIMINOZO_LE play
```

Muted/headless runs still honor the duration of scripted waiting one-shot sounds.

Desktop `play` keeps CStdb configuration/progress databases under a per-game
subdirectory of the user's data directory (`$XDG_DATA_HOME/rugp_rs`, or
`~/.local/share/rugp_rs` on Linux). Use `play --user-data-dir PATH` to choose
another root. Saves are atomically replaced with a process-exclusive lock;
corrupt or oversized saved profiles return errors instead of being reset.
The original game directory is rejected as an output location. Diagnostics and
`Game::open` remain volatile. This currently persists CStdb key/value writes,
not complete VM/game save slots or imports of original Windows save files.

The native image title menu accepts mouse hover/press/release and arrow-key
navigation (Enter/Space activates). Disabled selectors are excluded from input.
Pointer mapping shares the presentation viewport, including letterboxing and
resizing. `play --mute --fast-forward-ms 132000` executes the real startup on a
virtual clock before opening the window, for development testing; the default
launcher does not fast-forward. Later UI and story operations remain incomplete.

## Validation

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
nix develop --command cargo test --workspace --all-features
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
All 30,297 directly scene-referenced audio streams and 605 additional sources
inside CPmBgm/CPmEffect wrappers decode to PCM (30,902 total). The two EOS-tail
cases also have dedicated retail regression tests. `check-audio` follows these
wrappers and handles inline/fragmented setup payloads. Use
`check-audio --decode --additional-only` to validate just the additional sources.
The Kira adapter has a mock-backend regression that checks real nonzero output
samples and stop commands without requiring or playing audio through a device.

Both scene-referenced CRsi movie indexes are range-checked and read directly
from the split archive, one compressed frame at a time. Their CRip008 paired-row
YUV profile has a scalar Rust decoder, including adaptive luma interpolation,
run coding, and temporal RGB residuals. All 7,386 frames (3,152 independent and
4,234 temporal) decode with checked row boundaries and zero-padding validation.
The 17-byte resource trailers are retained. `video-frame` starts at the nearest
preceding keyframe; neither inspection nor extraction loads a whole movie.
The runtime plays full-screen non-looping movies on a fractional frame clock,
honors the script's CVideoSeq synchronization, and holds the final frame.
Catch-up decoding starts at a preceding keyframe without dropping a temporal
frame's dependencies. Movie sound remains a separate scripted sound source.
Positioned/scaled output, looping, and other playback styles remain unsupported.

The VM's retail startup regression resolves its entry through the process
ocean, executes the initial scalar assignments, and follows the jump from
scene 21 into scene 252 and binds the first logo helper's image/camera arguments.
This test deliberately acknowledges host requests;
it does not claim game/UI execution. Object-property binding and the
rendering/application host are still under development.

The first-logo runtime regression executes the original startup messages and
checks that the fully faded 1024x600 frame equals the decoded source pixels.
The next startup regression exercises native framebuffer snapshots, CFadeNormal
copying, the serialized CFadeMergeWhite curve, its blocking 800ms wait, and the
subsequent white CBg2d background. Only full-screen blit regions are implemented;
other effects, transformed regions, and timed offscreen blits remain unsupported.
Native Cr6Ti world-display scales are preserved independently of pixels. The
following startup screen's 0.5 bitmap scale and 1440 camera focal length now
project to the full viewport, covered by a retail pixel-equality regression.
Camera rotation, translated view targets, and general 3D geometry remain incomplete.
Desktop presentation and automatic exit have also been exercised under Xvfb.
The startup movie regression checks frame-clock boundaries, actual temporal
pixels, a large clock jump, and synchronization through the end of the opening.
Native CStdb text lookup/write and scalar output binding now run after the
opening. A sentinel regression verifies that the query actually updates its
destination, and another retail regression writes, closes, reopens, and reads
an isolated user-data directory. CRbx image-menu selection now blocks the VM
until an actual choice and writes the numeric selector result back to it.
The title regression checks disabled items, hover/pressed images, cancelled
drags, keyboard order, the asynchronous title fade, and mouse result binding.

All five directly referenced CRbx menus parse (137 selectors), and their five
indexed SIA masks and 250 state/background image references decode. The SIA
bytes are used directly as hit-test IDs, without substituting button rectangles.
These state images are indirect resources and are separate from the earlier
direct-bitmap count. `image OFFSET` also resolves menu image references.
Other menu styles, object-valued selection results, and text-based choices
remain incomplete. A desktop Xvfb/XTest smoke test also verifies hover and
clicking the first title button: the VM enters chapter one and reaches the
current boundary, `CVmMsg` at scene 483, PC `0x118`. Dialogue rendering and
advancement are not yet implemented. Unsupported host messages are never skipped.

`rugp_assets::message` reads the native CMsgSelBox resource layout and its
CFrameImage border-piece references, with truncation/version regressions and a
retail metadata test. This is separate from UI snapshot/save-state parsing;
all eight referenced CS5i border pieces now decode as bottom-up BGRA surfaces.
The image manager and `image OFFSET` can load these indirect border resources.
Border composition, font selection, and dialogue presentation remain pending.

See [THIRD_PARTY.md](THIRD_PARTY.md) for format research and attribution. Game
assets, original binaries, extracted scripts, and dialogue are not included.
