# rugp_rs

Native Rust game port of rUGP, targeting the 2008 **君が望む永遠
～Latest Edition～** disc. It runs the original archives and scripts directly:
startup/title, dialogue and choices, backgrounds/portraits, voice/BGM/effects,
movies, credits, original UUC loading and the original Premium menus.

All three original True End story routes have returned from new game to the
original title. Scenes 241 and 249 have continuous scene-pixel-rendered runs;
scene 246 has a resource/clock/text-active host-logic run. Four earned
first-chapter alternate routes, all three earned Premium extra stories and all
33 usable replay selectors also have complete host-logic playback/return checks.
These are specific verified paths, not exhaustive choice-combination coverage.
Detailed scopes and reproducible probes are below.

Playability takes priority over exact presentation. The software renderer uses
static portrait/planar-model approximations and simplified weather, transitions
and text styling instead of complete native 3D/GDI behavior. Original UUC loading
has the checkpoint restrictions described below. This is a game-specific playable
port, not a pixel-exact or general-purpose rUGP replacement; unsupported formats
or operation variants outside the covered flows still report explicit errors.

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

## Prebuilt releases

The [rolling release](https://github.com/asukaminato0721/rugp_rs/releases/tag/rolling)
provides Linux x86_64 (`.tar.gz`), Windows x86_64 (`.zip`),
and macOS Intel / Apple Silicon (`.tar.gz`) packages. Each includes the
`rugp_rs` executable, this README and third-party attribution; desktop and
device audio support are enabled. Linux packages are built on Ubuntu 22.04
and require glibc 2.35 or newer plus ALSA and X11/Wayland runtime libraries.
Original game files and a Japanese outline font must be supplied separately.
Extract the matching package and run:

```sh
./rugp_rs --project-dir /path/to/game play
# Windows (PowerShell):
.\rugp_rs.exe --project-dir C:\path\to\game play
```

Every push to `master` automatically tests, builds and packages all four
targets, then updates the `rolling` prerelease with the archives and
`SHA256SUMS`. The `rolling` tag moves to the built commit and the release notes
link to that commit. A manual Release workflow run on `master` also updates
the rolling release; manual runs on other refs only upload workflow artifacts.
Pushing a version tag such as `v0.1.0` publishes a separate versioned release.
Version tags containing a hyphen (such as `v0.1.0-rc.1`) create prereleases.

## Play

Keep the original game files available and install a Japanese outline font
(the tested fallback is Noto Sans CJK JP). No game assets or fonts are bundled.
The pinned Nix shell supplies the Linux audio/window build dependencies:

```sh
nix develop --command cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE play
```

Enter or a left-click reveals or advances dialogue. Arrows and Enter choose
menu items. Right-click or Escape opens the game menu. **F2** opens settings.
Settings include text speed, auto delay, master volume, BGM, voice, effects,
and fullscreen.
The runtime stores these preferences in the per-game `settings.json` file.
**Ctrl** activates skip while held. **S** toggles continuous skip.
Skip stops at choices, explicit non-text confirmations, and Load requests.
**A** toggles automatic dialogue advance. It waits for text reveal and voice
playback, then applies the configured delay. A left-click stops skip or auto.
**Space** hides the message window. Space, Enter, Escape, or a click restores it.
**Page Up** or the mouse wheel upwards opens text history for this session.
The history contains completed dialogue pages. Left/Right selects a page.
Up/Down scrolls a long page. Escape or right-click closes the history.
The mouse wheel downwards reveals or advances dialogue during normal play.
During the OP, Escape, Ctrl, Enter, Space, or a
left-click skips the movie. The script then continues to the title menu.
Audio playback is included in every build. `play` enables audio automatically.

If the installed game has no `Setup.ini`, the runtime uses the Latest Edition
disc defaults for the message window. An invalid existing file reports an error.

**F9** opens the original UUC Load chooser. The runtime detects `Vmreg` under
Documents or the Wine prefix. Use `play --save-dir /path/to/Vmreg` to select
another directory. The chooser supports arrows, Page Up/Down, mouse clicks,
Enter, and Escape. It lists original `vmrNNNNN.uuc` files, including later slots.
The title's Load command opens the same chooser. The VM and audio pause while
a menu is open. Cancel resumes the current game without confirming dialogue.

To start directly from a Windows save:

```sh
cargo run --release -- --project-dir /path/to/game play --load-uuc /path/to/Vmreg/vmr00001.uuc
```

The loader reads original files without changing them.
It supports v19 dialogue checkpoints with an empty native call stack.
It reconstructs the scene through script execution. It restores native integer
and Boolean variables, plus scalar variant values: integers, real numbers,
strings and null. Object-valued variants come from replay in their reference
context. Audio and animation clocks restart at that dialogue page.
For a branched route, repeat `--choice ID` for each selection after New Game.
The loader checks the game version, instruction identity, and dialogue text.
Unsupported checkpoints report an error. Global `gdb`/`sedb` progress is not imported.
Native UUC writing is not implemented. **F5** currently reports this limitation.
The port-specific save format and `import-save` command were removed.

## Inspection and diagnostics

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
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE play
```

Pass a game directory or its primary `.rio` file. All inputs remain untouched;
extraction refuses to overwrite an existing output. Objects are addressed by
their project-tree ID. Disassembly emits JSON with exact stream offsets,
in-memory command offsets, operation flags, and references.
Use `--no-default-features` for the CLI without desktop/window dependencies.
`play` executes the Rust game host; unsupported operations report their exact
scene/address. PCM export and decoding also work without opening an audio device.

Dialogue uses an installed Japanese outline font, preferring the face specified
in `Setup.ini`. Use `play --font /path/to/font.ttc --font-index 0` to supply a
face explicitly (`frame` accepts the same options). No fonts are bundled.
Fallback fonts and portable rasterization can change glyph metrics/appearance.
Click or press Enter/Space to reveal/advance dialogue; held-key repeats do not
advance multiple messages. Exact GDI layout and speaker styling remain incomplete.

On Linux, building the audio output requires ALSA development files.
The pinned Nix development shell supplies the audio/window dependencies:

```sh
nix develop
cargo run --release -- --project-dir /run/media/w/KIMINOZO_LE play
```

Headless runs still honor the duration of scripted waiting one-shot sounds.
Nonblocking sound-event queries (mode `0x20`) ignore their time operand, as
the native query branch does; SC457's `32/400` packet no longer fails or adds
a 400ms wait. Regression checks cover inactive, active and completed PCM,
an unread invalid time operand, and snapshot restoration of the query status
without stopping, replaying or waiting on the sound.
The static direct-source sync audit covers 1,876 packets: 280 effect waits
(`16/0`), 1,592 effect queries (`32/0`), three effect queries with time words
400/800/2200, and one BGM query (`32/0`). All use supported dispatch modes;
this audit does not resolve dynamically supplied sources or execute every packet.

The headless playability probe executes the same runtime, automatically advances
dialogue and chooses the first menu entry. It prints only cursors/counters and
uses volatile state (no save/profile writes). Optional arguments are maximum
ticks and milliseconds per tick; zero maximum ticks removes the probe budget.
Failures identify the next missing operation:

```sh
cargo run --release -p rugp_runtime --no-default-features --example autoplay -- /run/media/w/KIMINOZO_LE 10000 1000
```

For faster host-logic diagnostics, append `--no-scene-raster`. This still decodes
resources, lays out text and advances real host/media clocks and projection
caches, but omits scene pixel composition. It is not visual verification.
`--choices=0,1,0` selects enabled entries by zero-based order for successive
menus (including the title menu, where 0 starts a new game); remaining choices
default to the first entry. Like keyboard navigation, oversized indices wrap.
`--stop-on-title-return` stops when a played story returns to the initial menu
instruction, instead of automatically starting another game. This checks one
choice path only, not all endings or all branches.
For reproducible branch probes, repeat `--choice-at=SCENE:ADDRESS:INDEX` to
override a particular original selection cursor (decimal values, zero-based
enabled-entry index). `--expect-scene=ID` requires an actual sampled visit before
successful exit. Unreached directed choices fail the probe as well; returning
via a different ending cannot silently count as the requested route.
`cargo run --release -p rugp_scene_vm --example route_edges -- GAME_ROOT`
lists only static cross-scene jump and selection cursors, without dialogue.
Its possible edges do not prove that the original branch guards are satisfiable.

Three headless plans have returned to that title: default entries (24,960 text
confirmations, 73 scenes, 50 selections) and second entries after the title
(23,034 confirmations, 69 scenes, 52 selections), plus third entries after the
title (25,493 confirmations, 73 scenes, 53 selections). These execute real resources,
audio timing and host operations, but omit scene rasterization.
The default plan has also completed a continuous pixel-rendered run with the
same counters, including the ending and return to the original title.
After correcting non-text confirmations, fresh default host-logic and continuous
pixel-rendered runs both returned to that title at tick 37,201: 24,960 dialogue
confirmations, 50 selections, 1,146 separate non-text confirmations and 74 sampled
script scenes.
The fresh second-entry host-logic plan also returned to the original title at
tick 35,195: 23,034 dialogue confirmations, 52 selections, 1,076 non-text
confirmations, 8,634 sounds and 70 sampled script scenes.
The fresh third-entry plan returned at tick 37,878: 25,493 dialogue confirmations,
53 selections, 1,148 non-text confirmations, 9,372 sounds and 74 sampled scenes.
Those ordinal-only plans are three choice strategies, not themselves proof of
all three True End story routes. Separate ending-directed checks now establish
the following complete new-game/title-return paths:

| True End scene | Dialogue confirmations | Selections | Return tick | Scene rasterization |
| --- | ---: | ---: | ---: | --- |
| 241 | 23,375 | 51 | 35,366 | Continuous pixels |
| 246 | 22,883 | 47 | 35,216 | Host logic only |
| 249 | 23,034 | 52 | 35,195 | Continuous pixels |

The 241 pixel run uses default entries with four directed overrides:
`415:61248:1`, `422:38168:1`, `418:17096:2`, `418:54204:1`, and
`--expect-scene=241`. It completed 1,085 non-text confirmations and 8,948 sounds
across 63 sampled scenes. Every supplied directed cursor was actually reached.
The 249 pixel run chooses the first title entry and second entries for subsequent
menus, with `--expect-scene=249`; its counters match the headless run above,
including 1,076 non-text confirmations, 8,634 sounds and 70 sampled scenes.
Both use `--stop-on-title-return` without `--no-scene-raster`.
A separate cursor-directed full new-game run now reaches True End scene 246
and returns to the original title: tick 35,216, 22,883 dialogue confirmations,
47 selections, 1,089 non-text confirmations, 8,589 sounds and 70 sampled scenes.
It uses default entries except `--choice-at=422:38168:1`, with
`--no-scene-raster --stop-on-title-return --expect-scene=246`; no ending or
plot flags are assigned by the probe. Its host-logic scope is intentionally
distinguished from continuous scene-pixel verification.

Desktop `play` keeps CStdb configuration/progress databases under a per-game
subdirectory of the user's data directory (`$XDG_DATA_HOME/rugp_rs`, or
`~/.local/share/rugp_rs` on Linux). Use `play --user-data-dir PATH` to choose
another root. Saves are atomically replaced with a process-exclusive lock;
corrupt or oversized saved profiles return errors instead of being reset.
The original game directory is rejected as an output location. Diagnostics and
`Game::open` remain volatile. CStdb progress is shared across playthroughs and
is independent of reconstructed dialogue checkpoints.
The asset layer now provides a versioned reference-graph snapshot foundation:
inline-instance aliases (including bitmap-header references) survive a round
trip without saving process addresses or inventing archive offsets. It rejects
conflicting aliases, malformed graphs, and excessive expanded payloads. This
also supports checked rebinding to loaded script constants, verified against
all retail script inline references and initial object-variable values.
The VM snapshot layer preserves call frames/arguments, globals, shared typed
slots, synchronization status and suspended host operations, with a streaming
script compatibility fingerprint. Restoring a pending host operation does not
acknowledge or skip it. Runtime windows, audio, waits and UI are captured in the
same coordinated checkpoint described below.
Logical dialogue snapshots now retain consumed/remaining controls, displayed
text, partial glyph/pause delays, and clear-versus-continue confirmation waits;
restoring does not reveal text or confirm a page. The static audit checks
round-trip progress for all 100,923 real dialogue bodies, not their window layout.
Movie snapshots reconstruct pixels from the original archive and exact clock,
including delta-frame dependencies and fractional/end boundaries. A retail
opening regression compares restored frames and subsequent advancement directly
with uninterrupted playback.
Scene snapshots now preserve typed window/layer state, shared dynamic/offscreen
textures, world geometry/flags, cameras and their cached projection center,
character parts and placement, staged ParseScreen scenes, framebuffer-fade
endpoints, and posture waits. Postures keep their source identity, sampled delta
history and fractional clock instead of restarting motion. Texture-table IDs
preserve shared storage without merging independent equal-pixel surfaces.
Retail-host checks cover exact scene/VM round trips during startup, the opening,
title and first dialogue, plus synthetic in-progress ParseScreen/fade/posture
waits on that host. No-raster diagnostics cannot capture portable scene snapshots.
Independent actor snapshots now preserve generic wait tails, complete unsigned
timer epochs/thresholds, image-fade clocks and cancelled/completed event handles,
movie synchronization, and staff-roll progress. Event handles use the same
reference graph as VM variables; waits retain their original event even if an
operand is replaced. Completed events remain queryable without reviving their
old window. Credit rolls retain positioned glyph masks, so restoration does not
require the original font cache or reflow already-rendered text. Native packet
regressions now perform coordinated VM/scene/actor wire round trips during
image fades, timer waits, and credits. The opening movie also survives delta-frame
and final-boundary restores without overwriting the subsequently restored title.
UI snapshots now reload immutable image-menu/message-window resources while
retaining selector enable/ignore flags, hover/pressed visuals, text-menu hit
rectangles and ordinal output destinations. Retained message pages stay separate
from the current output stream; voice callbacks and forced-clock behavior are
restored without restarting output. Script navigation/save flags, variant-valued
settings and lifecycle state are preserved, but global CStdb progress is not
rewound. A non-persistent physical press latch prevents an old/load-button
release from confirming restored dialogue or menus. Retail packet/title checks
exercise these round trips, including suspended text choices and timed speech.
The full runtime checkpoint now coordinates all of these components with
logical audio in one shared reference graph. Compatibility/resource validation
finishes before any live state is swapped. A load discards stale output requests,
allocates fresh playback tickets, and reconstructs muted PCM deadlines from the
original epochs; device hosts must reset output and resume the returned plans.
Disk loading uses original Windows UUC files through the desktop chooser or
`play --load-uuc`. Internal snapshots support runtime validation and replay.
They are not exposed as another disk save format. Invalid native saves leave
the live game unchanged. Native writing and global progress import remain open.

Gallery database selectors now honor the native per-item rule: decimal-prefix
conversion must equal exactly 1 to enable an item; other values disable it.
Ignored-state flags are preserved correctly: `AllIgnoreSelecter` clears enabled
state, but a subsequent `EnableSelecter` makes navigation/back buttons usable
even though the disabled-image suppression bit remains set. The original CG
hub, twelve-page navigation and return path are exercised without changing
ending-clear flags or force-unlocking images. All 263 database-selector packets
are audited, including seven deliberately null/unavailable entries.
The music selector issues its original playback request and returns to the hub;
both fresh-profile replay pages stay locked and can navigate back. The locked
bonus explanation also returns after normal dialogue confirmations, without
writing unlock flags. This is isolated menu coverage, not an earned title unlock.
Separate original-ending regressions now exercise an earned title unlock:
credit branch 3 writes its own clear state, then enters Premium through the
previously disabled title selector and returns. The three True End terminal
scenes each expose exactly their own route plus Cancel in the gated extra-route
chooser; cancellation returns through the original hub to the title. The first
chapter's ending does not unlock second-chapter Premium. These tests run native
ending tails, not their complete story prefixes. A separate long regression
then selects each actually earned extra route through its original menu and
runs it back to the original title: scenes 202, 264 and 349 completed 4,828,
2,828 and 2,546 dialogue confirmations, respectively. These are host-logic
runs with resources and clocks active, not pixel-rendered route verification.

The post-clear New Game chooser also has a separate first-chapter alternate
mode, distinct from those three Premium extras. Four opt-in
`retail_earned_first_chapter_if_` probes have passed for the original terminal
prefixes 213/229/304/369. Each first earns permission via unchanged ending tail
249, selects the actual New Game/alternate-mode entries, and then plays the
whole first-chapter prefix from SC483 through its original ending/credits to
the title. They never assign plot, counter or unlock flags. All four observed
their own final story scene, a newly running SC238 credit roll, its completion
and the exact original title cursor. These are host-logic runs with resources,
text rasterization and media clocks active, not scene-pixel verification; the
prerequisite ending tail is not a complete second-chapter playthrough.

| Final story scene | Dialogue confirmations | Selections | Return tick |
| --- | ---: | ---: | ---: |
| 213 | 3,648 | 7 | 6,537 |
| 229 | 5,412 | 10 | 9,325 |
| 304 | 8,019 | 15 | 14,019 |
| 369 | 4,846 | 10 | 8,233 |

The Rust-port progress database uses versioned, stable stored-resource keys,
not Windows' private reference-stream text encoding. Bindings come from the
original selector instructions. Successful host image use (including cache hits)
records only bound CG resources; standalone decoding/preloading, failed host
operations and snapshot restoration do not unlock them. This progress persists
globally and is not rewound by game slots. A real new-game probe reaches its
first earned CG after 75 dialogue confirmations, then checks that earned state
against the original gallery. This is not Windows-profile interoperability or
a claim that every premium/replay entry has been played.
An additional earned-replay probe runs original scene 478 until its actual
image use unlocks the first replay (515 dialogue confirmations), preserves that
progress into an isolated Premium hub, and selects the enabled replay normally.
The unchanged replay script completes 378 dialogue and nine non-text
confirmations, returns to its replay page, then navigates back to the title.
It does not fabricate ending permission or replay flags; remaining earned
replay entries have the independent playback coverage described below.
The remaining 32 non-null replay selectors now have independent opt-in
playback tests (`retail_earned_replay_p1_` and `retail_earned_replay_p2_`), named
by original page/selector. They use actual story image use, check the original menu identity and
enabled selector, and drive the unchanged replay back to its page and title.
All 33 non-null replay selectors have now passed these checks. The seven null
selectors are deliberately unavailable, not missing replay implementations.
Original story choice
plans earn the appropriate branch's image. Later-chapter fixtures first run
the original SC427 chapter initializer and confirm its first dialogue; raw
serialized plot-variable defaults are not fresh-chapter state. One sequence also needs the native
encounter counter: its fixture actually plays two preceding SC425 sequences,
letting the original script increment it, rather than writing the counter.
Page-two selectors 9 and 10 additionally require a previous second-chapter
clear: their fixtures run the unchanged True End tail 249, then SC427 reads
that earned database state. Actual choices in SC422/418/290 (and SC300/292 for
selector 9) earn the required plot flags and image, without test assignments to
those flags. These two checks also enter through the genuinely enabled title
Premium selector. This is isolated prerequisite/ending-tail coverage, not a
claim to have played their entire pre-clear story prefix.

Literal-null `CVmSync` mode 16 now waits for an actual Enter/Space or left-click
confirmation, indefinitely when its time word is zero (also Windows INFINITE).
Positive timeouts remain cancellable and retain their remaining deadline across
saves. Clock-only mode 0 is separate and cannot be click-skipped. A load-button
release, a pre-load held mouse button, or an out-of-window release cannot confirm
the restored wait. This corrects a former one-millisecond automatic advance in
CG viewing. The autoplay diagnostic explicitly supplies these non-text
confirmations and reports their count separately from dialogue pages.

The native image title menu accepts mouse hover/press/release and arrow-key
navigation (Enter/Space activates). Disabled selectors are excluded from input.
Pointer mapping shares the presentation viewport, including letterboxing and
resizing. `play --fast-forward-ms 132000` executes the real startup on a
virtual clock before opening the window, for development testing; the default
launcher does not fast-forward. It does not synthesize confirmation input:
startup prompts can still require Enter/Space or a click.

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
subsequent white CBg2d background. Region copying/scaling and specialized-effect
fallbacks have since been added; timed offscreen blits remain unsupported.
Native Cr6Ti world-display scales are preserved independently of pixels. The
following startup screen's 0.5 bitmap scale and 1440 camera focal length now
project to the full viewport, covered by a retail pixel-equality regression.
Static CBg2d world placement now preserves the serialized Q16 scale, base point,
depth and pitch bias, independently of bitmap dimensions/display scale. The
default simplified camera mode uses the native linear roll/pitch pan and Q16
projection rounding; bank is unused in that mode. Translated camera positions,
angular bounds and synchronized modes 4/8 (plus approximate mode 12) are now
supported. Other styles and general 3D geometry remain incomplete.
The software compositor now also has a perspective-correct quadrilateral path:
reciprocal-depth texture interpolation, pixel-center sampling, object-viewport
clipping, either winding, and shared-edge alpha coverage are tested against
independent ray/plane intersections. World rendering uses this compositor;
an axis-aligned fast path preserves the same fractional-edge sampling.
`OM_SetExclusiveCamera` now selects the native Q12 camera rotation and
perspective/orthographic background projection. Queued overrides select the
next world image's projection. Camera-only transitions retain that projection;
the next image without an override restores the default. Independent-camera
references and near-plane clipping remain incomplete. A retail regression compares every
pixel against an independent inverse mapping, verifies opposite camera pans,
and checks that mode 0 is instantaneous even with a 400ms duration field.
Desktop presentation and automatic exit have also been exercised under Xvfb.
The startup movie regression checks frame-clock boundaries, actual temporal
pixels, a large clock jump, and synchronization through the end of the opening.
Native CStdb text lookup/write and scalar output binding now run after the
opening. A sentinel regression verifies that the query actually updates its
destination, and another retail regression writes, closes, reopens, and reads
an isolated user-data directory. CRbx image-menu selection now blocks the VM
until an actual choice and writes the numeric selector result back to it.
The title regression checks disabled items, hover/pressed images, cancelled
drags, keyboard order, the completed synchronous title fade, and mouse result binding.

All five directly referenced CRbx menus parse (137 selectors), and their five
indexed SIA masks and 250 state/background image references decode. The SIA
bytes are used directly as hit-test IDs, without substituting button rectangles.
These state images are indirect resources and are separate from the earlier
direct-bitmap count. `image OFFSET` also resolves menu image references.
Other menu styles and object-valued selection results remain incomplete.
Text-based story choices now have their own implemented ordinal selection path.
A desktop Xvfb/XTest smoke test also verifies hover and
clicking the first title button: the VM enters chapter one. Plain `CVmMsg`
output now renders Japanese text and blocks on its actual control-code waits.
A retail regression checks first/second-message pixels, persistent VM waits,
cancelled input, mouse/keyboard advancement, and the intervening sound wait.
Release desktop input was also exercised on an isolated Xvfb/XTest display.
Sound synchronization now distinguishes native mode 32 (nonblocking media-event
status query) from mode 16 (wait). Queries update VM status; waits observe the
active playback instance, not a replayed full duration. Muted asynchronous
one-shots have decoded-duration deadlines, and device output reports completion
for every playback, including nonblocking effects. Stop/replacement and stale
completion identities have regressions, as do real PCM timing and Kira mock
natural completion. Sound operation 4 now plays with its native initial/target
gain (4096 = unity) and millisecond transition, interpolated in amplitude rather
than decibels. Zero duration keeps the initial gain; PCM loops do not restart
the envelope and fading to silence does not itself stop playback. Kira mock
tests check actual output samples; native block buffering/quantization are not
reproduced bit-for-bit. Inline `CAcsPos` acoustic packets now retain real PCM
playback with a portable static stereo/distance approximation. Signed range and
XYZ words are checked in the compact 8-byte and full 14-byte layouts; the native
moving listener, cone and extra motion words are not reproduced. All 44 literal
compact effect packets are audited. The two inline `CMoveAcsOngen` references
are validated and sampled at time zero, using a fixed portable 1000-unit distance
scale; moving audio trajectories remain stationary approximations. Stereo gains belong to each playback instance, not
the shared PCM cache, and survive gain changes, stop handling and portable
restoration. Legacy saves default to unity channel gains. Kira mock samples and
an original spatial-play/sync packet verify channel balance and fresh restored
tickets. Restore preparation also decodes looping PCM before committing state.
Class-specific `OM_SetVarData` IDs 128/129 now set/clear distinct CParseBgBox
world-slot flags (1–7); non-integer payloads and out-of-range slots have the
native no-effect behavior. These flags suppress cached planes only in the
interleaved world-sprite pass, not the fast background pass. Their state is
preserved; full world-sprite interleaving remains unimplemented. A host
regression verifies cumulative flags, independent clearing, type behavior,
unsupported dispatch, and unchanged fast-pass pixels.
Chapter execution now passes the camera, layer-control messages and the
extended background at scene 88, PC `0x7c`, and synchronized camera setup
at scene 79, PC `0x428`. Correct synchronous ParseScreen waits now allow
later camera setup. Native default angular boundary correction now passes
the translated target at scene 79, PC `0x60`, and mode-4 camera setup
at scene 79, PC `0x3a8`. Posture motion now passes scene 64, PC `0x0`,
including object assignment and the later query/cancel through `CVmVarObj`.
Voiced dialogue now passes scene 483, PC `0xc28`: its `COptimizedObs`
callback queues the real PCM voice on a separate voice bus, without blocking
text input. Completing/skipping the message stops that voice; refreshing text
does not replay it. Sound operation 5 now fades/stops the existing instance,
without replaying it or extending its natural duration. Device output uses
Kira's current-gain stop tween rather than reproducing the packet's initial
gain/reset and native mixing curve exactly. Late PCM-duration reporting cannot
overwrite a pending fade-stop deadline.

Audio snapshot foundations now retain original source wrappers, playback epochs,
gain motions, and fade-stop deadlines after the output queue is drained.
Restored tickets are allocated above both saved and live high-water marks;
old completion callbacks cannot finish restored sounds. Muted restoration
reconstructs original PCM deadlines, including a tail that already ended before
its device callback. Device reconstruction resumes the decoded PCM frame/loop
phase and remaining gain motion rather than replaying from the beginning.
Fade-stop reconstruction preserves current attenuation and its remaining stop
time, but Kira's restarted decibel tween remains a presentation approximation.
Synthetic output samples and a real retail PCM/VM-wait round trip verify these
components, which are now included in the coordinated full-game slots above.

The chapter regression now reaches actual character dialogue at scene 483,
PC `0xea4`, instead of asserting an expected unsupported-operation error.
`OM_QueueFace2Data` retains the face parts, `OM_SetLayerData2D` retains layer
vectors, and character `OM_HumanIn` composites the real body/face textures.
Portrait placement now uses the owning world's camera, native CRmt logical
dimensions/scale, CRimp Q16 anchor and player height. It no longer derives the
portrait size from mipmap dimensions or window height. Captured ParseScreen
images use the same parent camera. Eyes and mouth remain static; layer-vector
deformation, rotated character planes and animated lips/blinks remain incomplete. Message windows render
above portraits regardless of serialized object-address ordering.
`OM_SetCharMouthAnimeSetting` retains lip-sync settings separately from the
character's placement/style, without starting mouth animation or replaying voice.
`OM_SetCharBreath` similarly retains independent breathing settings without
deforming the static portrait or changing its live placement/lip-sync settings.
Script helper functions can initialize local parameter slots not supplied by
the caller; assignment does not first read an uninitialized destination.
Region blits now crop/scale between differently sized surfaces and preserve
pixels outside the destination region. Mosaic, inversion, the right curtain,
and four-direction `CFadeCarten` with its retail 128-pixel feather use integer
pixel operators recovered from the retail DLL. Curtains retain their preceding
rendered frame and sample through internal snapshot restoration.
Stretch, StretchAnti and time-based OverStretchAnti interpolate the original source and
destination rectangles in Q13, with bilinear sampling and native opacity
endpoints. OverStretch can accumulate against the live destination. Native
antialiasing details, frame-count-based stretch, rotation, custom curtain widths, raster
and shock effects remain approximate. Their envelope layouts share one decoder
table, with prefix/tail and truncation regressions. Mode 0 retains its serialized
coordinates. Mode 0x11 uses source extents for default destination extents.
All eight retail even-nibble text/callback families (`0x0` through `0xE`)
share the portable layout while retaining their explicit text control-code
waits. Odd-nibble families remain rejected. Normal-render and raster-suppressed real-host autoplay
probes match through 1842 text confirmations and five story selections,
including the first character motion and later external posture resources.
Choosing the alternate first story option follows a different script branch
and reaches the same weather helper after 1160 confirmations. These are
playability checkpoints, not full-game completion verification.
`CMsgSelBox` story selectors now use actual labels, wrapped portable rows,
hover/pressed feedback and keyboard/mouse input. Confirmation writes the
zero-based ordinal into the script's integer destination and consumes the
selector list. A retail-resource regression covers both return values, real
framebuffer changes, blocking without input and canceled pointer clicks.
Image-menu `OM_AllIgnoreSelecter` preserves selector resources while hiding
their enabled/disabled state images and excluding them from hit testing and
keyboard navigation. Its native ignore bit remains independent from enable;
the clear-disable/enable sequence restores ordinary interaction.
Non-null object-valued selector IDs/results and native selector markup remain
unsupported. `OM_SetSpecifyCmdLighting` on background boxes retains its settings
but currently omits the lighting/pulse effect without introducing a host wait.
Rain/snow packets and the named scalar members of their independent
`CNormalCamera` objects are retained without drawing weather particles or
interpolating their intensity. Explicit script waits still run; non-null
weather completion handles remain unsupported rather than being abandoned.
Image-area `OM_BodyScroll` retains the configured origin/time but currently
shows the full source surface without independent body cropping/panning.
The ending's mode-1, attribute-`0x8000` pan on `CImgAreaBox` is also retained
as a visual approximation: native dispatch registers it asynchronously, with
no script output handle. It introduces no extra wait or changes to unrelated
camera/media clocks; the original separate script waits still execute.
Its zero-distance/time mode-1 reset stops that retained pan without clearing
the image or resetting unrelated clocks.
Mode-zero `OM_BoxScroll` uses staged absolute endpoint placement, without native
in-flight movement; the helper's separate blocking blit still retains its clock.
Background `OM_StopAutoAnim` mode zero stops an unimplemented native animation
channel, not explicit posture/camera tweens. Backing-cache enlargement hints
(`OM_SetCacheExtra`) are retained without changing the logical viewport.
Window-attached `CVmBlt` flag 1 retains its Xs-effect packet without replacing
world planes/cameras or drawing native raster distortion. Finite blocking
requests keep their duration; asynchronous duration -1 is retained without a
blocking wait. `OM_StopXsFade` modes zero and one clear only this separate effect
configuration, not ordinary framebuffer transitions or posture motion.
Timed offscreen blends whose placed source and background both equal the
existing destination retain their exact wait without rewriting live image
layers. A real ending self-surface effect verifies its 3,000 ms boundary;
non-identity timed offscreen blends remain unsupported.
The title's quit item now handles global `OM_VmmFinishByUser` exit code zero:
it stops audio, returns `RunState::Ended` and does not execute the following
title-loop jump. Nonzero exit codes remain explicit errors.

`message_stats` audits static message call sites/fields and encoded overrides
without printing retail text or sample bytes, for example:
`cargo run -p rugp_scene_vm --example message_stats -- GAME OM_BoxScroll`.

If a posture starts while a camera tween is prepared but not yet started,
portable playback first settles that camera at its bounded endpoint, then
starts the posture there. Native overlap/interpolation is approximated, while
posture identity, inclusive completion, synchronization and later render waits
are retained. A retail-host regression covers timed and instantaneous moments.
Background `OM_HumanIn2` low style `0x20` clears its staged image/captured surface
even with a null source, as the native clear branch does before resolving the
image. It preserves camera preparation and existing waits until presentation.
Inline `CDcAgesModelQsT#1` resources now use their declared base image/CBg2d
planes as a planar presentation. Material references stay in their original
object-reader context; object XYZ is retained as metadata, while mesh geometry
and its spatial placement are omitted. A retail regression checks all six
model packets in scene 474 against their actual base-image geometry/pixels.
`CVmVarObj#1` globals now load their serialized CBasicValue defaults, including
inline posture references, instead of being left as unresolved containers.
Defaults preserve their original reader context and distinct instance identity;
cached aliases share one instance, writes stay VM-local, and fresh VMs reset to
the resource defaults. Runtime synchronization can query an initialized but
unactivated posture without inventing an active motion or skipping the query.
The less-common dialogue variants use the same portable text/voice flow while
retaining their modifier words. Explicit text
pauses/confirmation still apply; native callback mixing/style differences are
not reproduced. Retail-packet regressions check actual font pixels, blocking
input and exactly one voice callback (or none for unvoiced packets). The
metadata-only `message_stats` example accepts `CVmMsg` to audit all output
modifier counts and one representative call site per family without printing
retail text.
Signed message speeds at or below `-10` use their magnitude as a forced
per-glyph delay (clamped to 3000 ms), with click-to-reveal disabled; they do
not invent input waits in automatically continuing text. Ordinary negative
speed settings inherit the configured user speed. A retail `-100` packet
regression checks the exact clock endpoint and the voice start/stop pair.
`CLayoutTextImg` ending resources are decoded with their font/style fields,
text, extent, and typed `CSbm` reference arrays. `OM_PlayTextStaffrollVideo`
scrolls the original labels in a separate window overlay with portable,
centered CSV layout and outline fonts. The requested duration runs independently;
mode-16 synchronization waits until its exact endpoint. Completion removes only
the credits overlay, preserving the background/camera. All four original text
sources and a real playback packet pass framebuffer/clock regressions. Native
font faces/style/alignment, embedded bitmaps, looping, nonzero offsets, and
non-default sequence settings remain unsupported or approximate.
The three `CParseBgBox` self-targeted, style-0 `OM_EmbedAlpherChannel` packets
are recognized as native primary-plane alpha-clear requests. Playback-first
presentation retains their settings and the existing RGBA/world planes rather
than reproducing that native alpha mutation. This has no asynchronous output
or completion wait; cameras, prepared tweens and other clocks are unaffected.
Other styles/channels/positions and cross-window embedding remain rejected.
`CWaitableTimer` supports asynchronous `OM_PlayMedia`, `OM_SetTimeInterval`,
mode-0/16 waits and mode-32 active-state queries. Threshold updates keep the
original playback epoch, including updates after an earlier threshold has
elapsed; the clock uses native unsigned wrapping. A real ending timer packet
regression verifies exact waits, nonblocking queries and no audio/frame changes.
`OM_ImageASyncFadeinout` supports the retail helper's display-0/mode-1
image-window fade with its native Q8 opacity envelope and exact duration.
The returned `CGenericUso` is a distinct in-memory object written to the real
script destination; copies retain identity and mode-0/16 synchronization waits.
Mode-32/33 queries report pre-cancellation activity, then complete/cancel the
actor; mode 34 queries without cancelling. Query time words are ignored natively,
including the ending's mode-32/time-4000 packet. Completion hides and zero-sizes the window
while preserving its image and restoring opacity. A real helper packet tests
variable writeback, framebuffer blending, query/cancellation and exact cleanup. Concurrent
fades on the same window, other display/mode combinations and timed blocking waits
remain unsupported.
Opt-in integration regressions initialize the real title and enter all eleven
branches of ending coordinator 244, plus alternate ending 249. Each returns to
the original title selection with no skipped host operations. Branches 8/9/10
and ending 249 retain scene rasterization; branches 0..7 use host-logic mode.
These isolated ending tests do not replace complete story-route runs or pixel
correctness verification.
`CVmSync` distinguishes an empty/non-object `CVmVarObj` from literal null.
The former queries false or completes a wait immediately; the latter queries
true. Neither query consumes its time word. Native scalar defaults remain
scalar values in the VM; this object-only extraction applies at synchronization,
not as a general conversion of integers into events or pointers.

Billboard queue parameters, character posture/scaling parameters and character
material property 100 and shading packets are retained without exact
geometry/deformation/tinting. The
`CLayerSimpleFlush` parameter packets are retained; playback currently completes
instantly without rendering that layer effect, including wait/query completion.
Complete-character `OM_SetRefData` identifier 20 now composites the supplied
body/expression parts from the original inline reference context. Null clears
the character core; current placement is retained rather than reproducing the
native saved pose/deformation. A retail regression checks actual part pixels,
staging, replacement and clearing. Flash-layer color and scalar packets are
retained separately even though their visual playback is approximated.
Null body/expression `OM_HumanIn` packets update placement/style without
replacing the existing texture or reviving a cleared character.
Plain `CVmImage` now loads or clears a real CImgBox surface, including subsequent
blits from that surface. Its immediate-presentation boolean refreshes the scene;
`OM_SetFitImage` makes subsequent image assignments size the box to the source
while preserving X/Y. A retail-helper regression checks fitting, staged versus
immediate pixels and null clearing. Nonzero positioning/owner-draw modes remain
unsupported.
Ordinary blits accept nonzero auxiliary words while validating blocking, flags,
regions and duration independently.
Declaration-only, never-called CRsa objects now provide persistent typed shared
slots. They are separate from active function arguments: the chapter's shared
scope in scene 208 can be accessed by helper 211 without borrowing caller 482's
arguments. VM tests cover aliases, persistence across helpers, fresh-VM reset,
and continued rejection of out-of-scope callable arguments. `control_flow`
accepts an optional project node ID to report only metadata cross-references.
Copied helpers can retain their template's parameter owner: original helper 63
declares helper 58's slot. Matching leading parameter declarations now bind the
nearest actual call frame, including nested template/copy calls. They do not
invent zero defaults or borrow arbitrary caller arguments; snapshot restoration
retains the same binding. A real earned extra-route probe passes these calls.
Sound operation 6 adjusts an existing instance's gain without replaying it or
changing its completion deadline. Device output ramps from current gain to the
target (rather than reproducing the native initial-gain reset); zero duration
uses the packet's start gain. Kira mock samples verify the live looping handle.
`OM_SetUucSaveSwitch` retains the script's save-enable state even when sent to
the global/unbound system target; internal checkpoints honor that permission.
`OM_ParseImageBuild` renders the world and visible portraits into an offscreen
surface without presenting it; `OM_UpdateAllView` explicitly refreshes the
logical scene; `OM_AgesRecalc` refreshes the portable geometry/rendered view.
The native incremental queue-clear property 41 is accepted
without discarding logical images/motions in the full-scene renderer.
Captured-frame re-entry (`OM_VramAgesImageIn`) currently stages a fixed screen
snapshot. Group visibility and all-character removal operate on the owning
window's children; unrelated UI windows retain their visibility.
Null-source sound operations 0/1 stop the effect/music group respectively,
using the same group mapping for device audio and muted scheduling.
Its inline `CPostureMoment#4` resources now retain typed numeric fields and
all five context-bound references during the original archive-reading pass.
Inner versions 0–2, conditional vectors/modes, signed vectors and finite channel
scales have synthetic truncation/alignment tests, and all retail scenes validate
their 152 typed inline posture records. Background posture execution now samples
position, Q4 angles and zoom on an independent VM clock, with per-moment bounds,
channel scaling, absolute/delta modes, inclusive endpoints and loop handling.
Time modes use sequential, equal-share or duration-weighted segments. Spatial
Bezier uses Bernstein sums; timing Bezier uses the native 40-segment crossing
approximation. Floating-point sampling uses f64 rather than x87 precision.
Synthetic tests and a host/pixel regression cover elapsed time, distinct identical
instances, cached identity, stacking, restart, natural completion and mode-33
query/cancel without snapping to the endpoint. Modes 16/17 now keep the real VM
pending until natural completion or a supplied timeout, without replaying the
motion. Mode 32 queries active status and forces the endpoint; timeout callbacks
likewise distinguish snap-to-end from cancellation. The host regression covers
inclusive endpoints, actual VM suspension, identity and a timed-out loop.
`OM_AddPostureMomentumEx` accepts character targets as well as backgrounds,
with offset, positive time scale and channel multipliers. Fractional scaled
milliseconds are retained between ticks. Portrait placement now uses motion
position/depth/zoom; native rotations remain approximate. Postures can continue
while screen transitions redraw, without an exact native backing-cache model.
Prepared-state option 1 now uses the same live presentation clock in the
portable full-scene renderer. It retains actual motion samples, delta history,
loops and script synchronization; separate native prepared pose/backing buffers
are a visual approximation, not an invented immediate completion.
Restarting a cached active posture now validates a fresh motion before removing
its previous binding. One source identity remains attached to one owner; restart
resets its motion clock/history without snapping the old sampled camera, and
transfer leaves the previous owner where it was. Native fixture tests cover
restart, transfer, failed replacement, source queries and cancellation. A long
opt-in regression has run all three earned extra routes back to the original
title, including their original non-text input waits (683, 326 and 388 actual
confirmations respectively). It does not force unlocks or bypass host calls.
Standalone posture resources now retain all five references with the RIO object
cache context; stored offsets identify their playback/wait instances independently
of inline object identities.
`CMoveAcsOngen#3` now retains sequential segment endpoints, X/Z compact vectors,
Bezier controls, timing modes, flags and embedded quadratic/Bezier time curves.
Timing instances and class reuse share the original archive cache. Tests cover
synthetic truncations, cache reuse and all 146 retail inline motion records.
Four external motion bodies also parse; two extents have an unexplained 26-byte
trailer that is retained explicitly for inspection. Playback now uses their
validated curve and ignores those opaque trailing properties. No retail
coordinates or curve tables are included in the repo.
Camera style `0x800` is supported for both screen and angular
messages: the prepared start state is stored separately without changing
the visible camera or starting a duration clock. A zero zoom can be stored
without attempting to project it. Host tests check stored zoom/Q4 angles,
unchanged pixels/current camera, and rejection of unsupported fade modes
without corrupting the stored state. Fade modes 4 and 8 interpolate staged
screen zoom and angular targets on ParseScreen's shared clock, including
camera-only transitions and synchronized opacity curves. Separate screen
and angular commands combine through the prepared target; a later capture
replaces the start without changing an already prepared delta. Native easing
uses repeated quantized Q16 squaring, with signed-floor component interpolation,
not floating-point powers. Tests cover all sixteen easing styles, intermediate
camera/pixel values, VM waits, zero zoom/duration, and capture cleanup in both
modes, including mixed-mode screen/angular commands. Native mode 4 clears
the window backing-cache flag that mode 8 sets, but both receive the same
ParseScreen progress and easing; neither starts a timer from the camera
message's duration field. The full-scene renderer rebuilds pixels instead of
using those incremental window backing caches.
Mode 12 now uses the portable synchronized absolute interpolation as a
playability fallback; its native rotation/spatial effect is not reproduced.
The original SC130 camera style `0x11` is also accepted as absolute linear
interpolation: the native sampler uses the low four easing bits, not this
extra bit. Its original style is retained and validated in snapshots. A host
regression covers the same pixel/clock/capture cases in modes 4, 8 and 12.
Additive/relative style `0x400` and active-camera replacement remain unsupported.
Plain bitmap `OM_HumanIn` placement now honors its signed global X/Y and
absolute Z (zero defaults to 720), including SC217's CRip008 Y offset. These
32-bit offsets remain separate from bounded immutable vertices and cached
image data; projection, angular bounds and scene snapshots retain them. Native
camera-relative slots still ignore global XYZ. Nonzero CBg2d-group placement
and textured-model geometry deformation remain separate unsupported cases.
Absolute XYZ camera positions now apply in instant mode and animated modes 4/8,
including signed-floor interpolation, captured starts and combined screen-zoom
commands. World planes subtract the camera position before projecting at their
relative depth. Native slot mode 1 instead ignores XYZ translation while still
applying angular pan and zoom. The chapter's far layer has that mode; its three
foreground layers do not. An extended retail pixel oracle covers the unchanged
far plane and independent foreground parallax.
Default angular boundary correction follows the native two-stage Q13 algorithm,
including first-image pixel width, quantized sine terms, asymmetric bounds,
the hardcoded reference height and the previous first-slot projected Y center.
Signed/zero relative depth is supported rather than rejecting a camera crossing a
background plane. Projection preserves signed MulDiv and its zero-divisor
sentinel; wholly nonpositive-depth planes are culled without erasing other
layers. Geometry crossing the near plane still needs explicit clipping.
The projection cache is separate from staged camera targets, survives image
staging, and follows active render snapshots. Correction runs at preparation,
prepared-target lookup and synchronized sampling, not as a generic rectangle
coverage clamp. `_ftol` low-word truncation/invalid results and signed MulDiv
rounding have unit tests; metadata-only numerical vectors and an inverse pixel
oracle cover corrected targets. The implementation uses f64 arithmetic, not
bit-identical x87 extended intermediates; alternate boundary flags and full
matrix modes remain unsupported.
The sampled draw-ratio curve is a `CMimeFlat16LimitOne#1`, not a CFade:
seventeen signed samples use a Q16 input, with arithmetic-shift interpolation
and input clamping. Its strict asset parser, inline-object framing and native
ascending/descending defaults are now implemented. Synthetic tests cover
every truncation, exact knots, negative slopes and unclamped signed output;
a retail test checks every Q16 position against an independent raw-data
oracle. CParseBgBox draw-ratio mode `0x100` now queues opacity curves until
ParseScreen starts. Default ascending/descending curves and explicit sampled
curves (including mode `0x110`) use the shared Q16 transition progress;
their own time field does not start another clock. Opacity is sampled and
clamped each frame, applies independently to each world plane, and retains
its final value when synchronized curve state is released. Render snapshots
keep subsequent image replacements staged; explicit live presentations can
refresh them without resetting the clock. A host regression checks two
overlapping objects with different curves, real curve data, intermediate
pixels, VM waits, zero duration, large time jumps, curve replacement/reset,
and staged image replacement. Independent-clock draw ratios, special default
after-image curves, non-background targets, overlapping ParseScreen clocks
and framebuffer effects during them remain explicitly unsupported.
ParseScreen now holds the actual VM message until its render clock finishes;
only trigger style `0x8000` returns asynchronously. This applies to live
camera/opacity tracks and the legacy framebuffer fallback, without resetting
the clock or blocking audio timing. A host regression verifies the pending PC,
intermediate/final pixels, repeated zero-time steps, exact completion, and
zero-duration behavior. Unknown trigger styles, non-null effect data and
asynchronous output handles still stop explicitly.
Static native inspection establishes that script camera angles use 4096 units
per turn (internally shifted left by four for a 65536-unit
matrix), and fade mode 0 takes the target state directly, not a linear 400ms
interpolation. Its low-four-bit easing flags, including style 1, have no
effect in fade mode 0; screen and angular pixel tests cover them. Default
simplified mode pans linearly rather than applying that
trigonometric matrix. Its projection wraps the Q16 numerator in signed 32-bit
arithmetic before native MulDiv rounding, including the overflow sentinel;
screen-origin addition also wraps in Q16. Large offscreen chapter pans,
pitch-bias wrapping, translation/depth, per-slot translation bypass and final
screen-origin wrapping have numerical regressions.
The metadata-only `rugp_assets`
example `background`
accepts a game directory, CBg2d offset and extent for this work.
Its optional `--extension-header` reports the native extension fields and first
scrambled layer-class record. `--layers` strictly parses the linked
`CBg2dLayerRip007Image#1` graph, sharing the enclosing class/object cache,
validates its count and end of input, and decodes every layer image. Both
options report metadata without extracting game pixels or layer names.
Cr6Ti inline records now consume their native even-aligned bitstream and
two-byte lookahead guard, keeping subsequent cached layer records aligned.
Synthetic regressions cover shared classes/images, linked ordering, every
truncation, non-finite scales, invalid pointers and count/EOF checks. The
chapter background's four layers all parse and decode. Runtime image groups
now retain each layer's texture, anchor, independent position/depth and scale;
they project and composite in native linked order (up to eight slots).
The serialized draw-flags hidden bit is honored separately from the
sprite-pass suppression mask; the chapter background's three foreground
planes are initially hidden, not eagerly displayed.
The group's scaled background bounds, rather than individual foreground
quads, govern the camera boundary check. One image replacement replaces the
whole group without clearing sprite-pass masks. A retail host regression
compares every composited pixel against an independent inverse-mapping and
alpha oracle at two camera positions, in both the native initial visibility
state and an explicitly exposed all-layer test state, and checks staging and
replacement. Native layer-activation messages are not yet implemented.
All 321 directly referenced backgrounds' legacy planes were also re-decoded.
Non-default extension color/range/properties currently leave the basic image
projection unchanged; they no longer reject otherwise valid background images.
General legacy canvas composition and sprite interleaving remain unsupported.
For X11 isolation, unset
`WAYLAND_DISPLAY` and `WAYLAND_SOCKET`; winit otherwise prefers an inherited
Wayland session.
Unknown host messages are never silently skipped. Unsupported dialogue
callbacks, substitutions and packet families still stop with explicit errors;
the documented presentation approximations do not replace story/control state.

`rugp_assets::message` reads the native CMsgSelBox resource layout and its
CFrameImage border-piece references, with truncation/version regressions and a
retail metadata test. This is separate from UI snapshot/save-state parsing;
all eight referenced CS5i border pieces now decode as bottom-up BGRA surfaces.
An opt-in retail audit parses every one of the 100,923 dialogue bodies and
checks their primary window/voice-callback types and output families without
logging original text. This is syntax coverage, not proof of execution, layout,
trailing display-modifier fidelity or completion of every branch.
The image manager and `image OFFSET` can load these indirect border resources.
A separate default-window/font audit now rasterizes 100,748 isolated pages from
all 100,923 dialogue bodies using the configured size and installed Noto Sans CJK
JP fallback, including text before explicit clear controls. Empty records and
standalone clears are valid and need not display a glyph page. This verifies
font coverage and page bounds, not VM branch execution, script-time window
overrides or retained-page combinations across separate message records.
The static text-choice audit also renders all 132 potentially reachable
Add/Ask blocks (273 original labels), checks their native-default hit rectangles,
and includes conditional labels conservatively. Forty old selector packets in
SC242's unlabelled suffix are excluded only after a control-flow overapproximation
proves them unreachable: all scene/label entries, both scalar branch sides and
call returns are retained, while unconditional jumps/returns have no fallthrough.
These include five obsolete style-2 menus, not a missing live story-menu mode.
Opt in with `retail_all_isolated_dialogue_pages_fit_the_native_default_windows`
and `retail_static_text_selector_blocks_fit_native_default_windows`; provide
`RUGP_TEST_GAME` and optionally `RUGP_TEST_FONT`.
Eight-piece border placement and the configured non-gradient backdrop blend
now compose in Rust, including the original additive/shift blend rather than
ordinary opacity. Transparent UI layers preserve straight colour and alpha.
The saved CViewBox selection state and scroll position are not colours or text
spacing. Font discovery/rasterization is portable Rust, with a bounded glyph
cache; the original GDI/5RF cache, speaker colours, kinsoku, automatic pagination
and full layout-flag semantics remain incomplete. Viewport overflow is an error,
not silently dropped text. Normal tests use synthetic text only; the retail
dialogue regression requires `RUGP_TEST_GAME` and an installed Japanese font
(or `RUGP_TEST_FONT`). The `dialogue_stats` example emits only aggregate metadata.
The `sound_stats` example similarly inventories control kinds/flags; add
`--transitions` to print distinct gain/transition tuples without sound data.
Null-source `CVmSound` dispatches only the four native global-stop kinds;
other kinds (including a real scene-345 play packet) are native no-ops, not
implicit stops or waits. Both device and muted paths preserve active channels.

See [THIRD_PARTY.md](THIRD_PARTY.md) for format research and attribution. Game
assets, original binaries, extracted scripts, and dialogue are not included.

Desktop animation clocks retain fractional milliseconds and slow-frame time.
Motion ticks during a synchronized opacity transition update the captured pose
without publishing staged replacement images. Specialized spatial fades and
native character deformation still use the approximations described above.
