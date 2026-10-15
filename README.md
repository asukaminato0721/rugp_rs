# rugp_rs

Native Rust reimplementation of rUGP, targeting the 2008 **君が望む永遠
～Latest Edition～** disc. This is an active port: archive and scene decoding
work, along with Cr6Ti/CRip007/CRip008, CRmt mipmaps, CBg2d image planes,
and OOR/Vorbis audio decoding. A first vertical slice runs the real startup VM
through the title menu into chapter-one dialogue and standing portraits in a
software framebuffer. Playability takes priority over exact effect fidelity:
static portrait projection and some transitions intentionally approximate rUGP.
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

Dialogue uses an installed Japanese outline font, preferring the face specified
in `Setup.ini`. Use `play --font /path/to/font.ttc --font-index 0` to supply a
face explicitly (`frame` accepts the same options). No fonts are bundled.
Fallback fonts and portable rasterization can change glyph metrics/appearance.
Click or press Enter/Space to reveal/advance dialogue; held-key repeats do not
advance multiple messages. Exact GDI layout and speaker styling remain incomplete.

For device audio, enable `--features audio`. On Linux this needs ALSA development
files. The pinned Nix development shell supplies the audio/window dependencies:

```sh
nix develop
cargo run --release --features audio -- --project-dir /run/media/w/KIMINOZO_LE play
```

Muted/headless runs still honor the duration of scripted waiting one-shot sounds.

The headless playability probe executes the same runtime, automatically advances
dialogue and chooses the first menu entry. It prints only cursors/counters and
uses volatile state (no save/profile writes). Optional arguments are maximum
ticks and milliseconds per tick; failures identify the next missing operation:

```sh
cargo run --release -p rugp_runtime --no-default-features --example autoplay -- /run/media/w/KIMINOZO_LE 10000 1000
```

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
an axis-aligned fast path preserves the same fractional-edge sampling. Full
matrix camera modes are not yet connected. A retail regression compares every
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
Other menu styles, object-valued selection results, and text-based choices
remain incomplete. A desktop Xvfb/XTest smoke test also verifies hover and
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
reproduced bit-for-bit. Native extended acoustic packets remain unsupported.
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

The chapter regression now reaches actual character dialogue at scene 483,
PC `0xea4`, instead of asserting an expected unsupported-operation error.
`OM_QueueFace2Data` retains the face parts, `OM_SetLayerData2D` retains layer
vectors, and character `OM_HumanIn` composites the real body/face textures.
Portraits currently use approximate depth/horizontal placement, static eyes
and mouth, and instant replacement; native sprite geometry, layer-vector
deformation and animated lips/blinks remain incomplete. Message windows render
above portraits regardless of serialized object-address ordering.
Script helper functions can initialize local parameter slots not supplied by
the caller; assignment does not first read an uninitialized destination.
Region blits now crop/scale between differently sized surfaces and preserve
pixels outside the destination region. Known specialized CFade#12 classes
(rotation, stretch, mosaic, raster and shock effects) currently use their common
transition curve/wait without native displacement/deformation. Their envelope
layouts share one decoder table, with prefix/tail and truncation regressions.
Default-bound mode 0 is supported as well as explicit-region mode 0x11.
Standard `0x6` text
packet variants share the portable layout while retaining their explicit text
control-code waits. The real-host first-choice autoplay probe has advanced
319 text waits, including the first character motion and later external
posture resources; full-game completion is not yet verified.
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
Plain `CVmImage` now loads or clears a real CImgBox surface, including subsequent
blits from that surface. Nonzero positioning/specialized modes remain unsupported.
Ordinary blits accept nonzero auxiliary words while validating blocking, flags,
regions and duration independently.
Declaration-only, never-called CRsa objects now provide persistent typed shared
slots. They are separate from active function arguments: the chapter's shared
scope in scene 208 can be accessed by helper 211 without borrowing caller 482's
arguments. VM tests cover aliases, persistence across helpers, fresh-VM reset,
and continued rejection of out-of-scope callable arguments. `control_flow`
accepts an optional project node ID to report only metadata cross-references.
Sound operation 6 adjusts an existing instance's gain without replaying it or
changing its completion deadline. Device output ramps from current gain to the
target (rather than reproducing the native initial-gain reset); zero duration
uses the packet's start gain. Kira mock samples verify the live looping handle.
`OM_SetUucSaveSwitch` retains the script's save-enable state even when sent to
the global/unbound system target. This is not a complete save-slot implementation.
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
Prepared-state option 1 remains unsupported.
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
Additive/relative styles and active-camera replacement remain unsupported.
Absolute XYZ camera positions now apply in instant mode and animated modes 4/8,
including signed-floor interpolation, captured starts and combined screen-zoom
commands. World planes subtract the camera position before projecting at their
relative depth. Native slot mode 1 instead ignores XYZ translation while still
applying angular pan and zoom. The chapter's far layer has that mode; its three
foreground layers do not. An extended retail pixel oracle covers the unchanged
far plane and independent foreground parallax. Nonpositive relative plane depths
remain explicitly unsupported.
Default angular boundary correction follows the native two-stage Q13 algorithm,
including first-image pixel width, quantized sine terms, asymmetric bounds,
the hardcoded reference height and the previous first-slot projected Y center.
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
General legacy canvas composition, sprite interleaving and full-matrix modes
remain unsupported.
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
The image manager and `image OFFSET` can load these indirect border resources.
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

See [THIRD_PARTY.md](THIRD_PARTY.md) for format research and attribution. Game
assets, original binaries, extracted scripts, and dialogue are not included.
