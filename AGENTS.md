# Project agent memory

This file is the project's committed home for project-intrinsic agent knowledge: build, test, release, architecture, and sharp-edge notes that should travel with the code.

- Add durable project-specific notes here as they are discovered through real work.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.

## Helion IDE chrome

`helion-ide` is three canvases (Editor / Device / Timing) plus a 6-item activity rail and More ⋯ for the other `WorkspaceTab`s. Routing lives in `crates/helion-gui/src/chrome.rs` (`pane_for_workspace`) and `crates/helion-gui/src/surface.rs` (`central_pane` / `ChromeDriver`). The binary paint path must follow `central_pane`. More destinations never fall through to Timing. Open HDL uses `open_hdl_dialog` (rfd). Linux rfd/xdg-portal is pinned to zbus 5.13.2 / zvariant 5.9.2 so rustc 1.85 CI can compile (zbus ≥5.14 needs 1.87). Implement/synth/place/route/sim_run run on a `helion-engine` thread (`spawn_job`); do not call them inside an egui frame. Debug: `HELION_UI_LOG`, `HELION_DEBUG_OVERLAY=1`. `egui::Context::set_debug_on_hover` is `#[cfg(debug_assertions)]` only — never call it on a release paint path. CI `release helion-ide` is `cargo check --release -p helion-gui --bin helion-ide`. Launch shots: `HELION_OPEN`, `HELION_FLOW=implement|synth|sim`, `HELION_WORKSPACE=<tab label>`.

## Launch shot harness

`HELION_SHOTDIR` + `HELION_SHOT` dump the framebuffer as a PPM (`<name>.ppm`); `HELION_SHOT_QUIT=1` exits after writing. Two sharp edges, both hit in practice:

- **Set `HELION_SHOT_SURFACE=1` for any `HELION_WORKSPACE` shot.** Without it the Welcome page owns the central panel (no source is open), so *every* tab captures the same image — only the Recent list perturbs pixels. Qor/Drc/Reports were byte-identical; with the flag all 32 tabs differ (min pairwise separation 0.17%, the two Clock surfaces legitimately sharing the "No clocks yet." empty state on an empty project).
- **`HELION_SHOT_FRAME=<n>` (default 4) is a frame count, and egui only repaints while something is happening.** A large `n` on an idle UI never arrives and the shot times out. Synchronous hooks (`HELION_FLOW=implement` runs `model.implement()` before frame 0) are already settled by the default; async engine jobs are not.

`tip(name, shortcut, tcl)` paints a Tcl command into a tooltip, so the name must resolve: the `helion_gui::tcl` `HOST_COMMANDS` table *or* an `IdeModel::exec` verb. `impl_design` / `create_project` / `open_source` are Rust method names and error with `invalid command name`; the real ones are `run_implementation`, `save_project_as`, `read_sv` (which also opens a `.prj`). `tooltip_tests` scans the binary's own source so a new tooltip cannot reintroduce this.

## Toasts

`bin/helion-ide/toasts.rs`: small auto-dismissing notices, bottom-right above the status bar. Wall-clock expiry (5s) via `Instant` + `request_repaint_after` — never frame counts (the UI is reactive; frame-based timers linger while idle). Max 360px wide with wrapping, max 4 stacked, deduped. Job *progress* lives in the status bar; toasts are completions/errors only. No "Started" toasts.

## In-app updater

`crates/helion-update` (pure logic) + `bin/helion-ide/update_ui.rs` (UI).
Check runs once on fresh session startup, on a background thread; no periodic
checks while running. `HELION_NO_UPDATE_CHECK=1` skips it. GitHub Releases via
`self_update` 0.42 (pinned: 1.x needs Rust 1.88, workspace MSRV is 1.85);
SHA256SUMS verification is manual with `sha2` (0.42 has no checksums feature).
In-place install only when the install dir is user-writable (probed with a
tempfile, not permission bits); otherwise download-only to `~/Downloads`.
macOS `.app` installs swap the whole bundle (replacing one binary would break
the ad-hoc signature); Developer ID-signed bundles refuse in-place and fall
back to download-only. Restart saves the editor, spawns the new binary with
the same args, then closes. Menu: Help -> "Check for updates…".

## macOS release signing

The `.app` is ad-hoc signed (`codesign --deep --force -s -`, in `scripts/build-macos-app.sh` + verified in `release.yml`), not Apple-notarized. Without the ad-hoc signature, Sequoia-era Gatekeeper reports the download as "damaged". First launch still needs the quarantine cleared once: `xattr -cr /Applications/Helion.app` (documented in README Quickstart). Full notarization needs a paid Apple Developer ID — not set up.

## 7-series bitstream assembly

`FrameStore` must cover **every valid frame from `part.json`** (`FrameStore::for_part`, built on `PartInfo::frame_addresses`), never the union of `tilegrid.json` bit ranges. The FDRI stream has no per-frame address, so a missing frame shifts every later frame and the bitstream still reports `done=1`. The tilegrid union is 434 frames short on xc7a35t and 2,194 on xc7z020; the old symptom was a BUFG written into an unrelated CLB frame.

Clock nets are real pip routes, not free: route the spine (IOI/HCLK_CMT → BUFG_REBUF → HROW → INT CLK) and emit the per-stage active/enable companion bits (`clock_companions` in `emit.rs`). **Every stage has a gating bit and a mux bit; the mux alone leaves the branch muted.** Missing so far, all found only by diffing against a working Vivado bitstream: `HCLK_L.ENABLE_BUFFER` (the HCLK_L arm did not exist, so left-edge clocks -- i.e. the ZedBoard GCLK -- died at HROW), `CLK_HROW.BUFHCE.BUFHCE_X1Y<n>.IN_USE` (index `n` pairs with `CLK_HROW_CK_MUX_OUT_{L,R}<n>`), and input-pad `PULLTYPE.NONE`/`IN_TERM.NONE` (inputs default to a pulldown; the clock pin is an input).

`CLK_BUFG_REBUF` tiles are **in-line amplifiers**, not taps: the clock reaches
logic past a repeater only once that repeater is enabled. Programme the whole
chain between the injection stage and the placed logic, not just the stage the
route named -- the emitter cannot see the other repeaters from the netlist, so
ask the device (`EmitDevice::tiles_of_type`). The reference blinky hides this
because its flops sit beside the injection point; a design further down the
column loses its edge past the last enabled repeater. Take the neighbour on
each side unconditionally (Vivado does), then walk outward only while logic
lies beyond.

`clock_companions` is a hand-written table and has been wrong twice; two of the four stages we model were missing bits. Prefer deriving companions from the DB the way pips are derived. Check with `python3 work/clkdiff.py <ours.bit>` -- it scans **every clock tile on the device** against `~/CDA4253_A1/blinky.bit` and separates REAL diffs from benign routing choices. A single-path version is blind by construction: it can only confirm the route someone already thought of, and that is exactly how the missing REBUF enables survived. Note prjxray silently drops unmatched FASM lines, so a clean assemble proves self-consistency only.

### FF configuration: two bits per flop, and both are mandatory

A slice FF is configured by exactly two segbits: `AFF.ZINI` (the power-up INIT
value) and `AFF.ZRST` (SRVAL, the value taken on set/reset). Emit **both** for
every FF, unconditionally. Emitting `ZRST` only when the FF has an `SR` net is
wrong: with `ZRST` clear SRVAL defaults to **1**, so a register that inits to 0
powers up as a constant 1 -- the LED lights and never blinks, which looks
exactly like a dead clock and sends you hunting the clock network instead. This
cost a full investigation; the census that catches it is ZRST count == ZINI
count across CLB tiles.

`AFFMUX` selects what drives the flop and the choices are not interchangeable:
`O6` is the LUT6 output (the normal LUT->FF path), `O5` the LUT5 output, `XOR`
LUT6^LUT5 for carry/adder logic, and `AX`/`BX`/`CX`/`DX` the **raw slice input
pins**, which bypass the LUT. A LUT-fed flop must use `O6`; selecting `AX`
leaves D undriven because nothing connects to that pin.

### Instrumentation: decode, do not diff FASM text

`scripts/bitdiff.py` compares FASM *lines* and reports phantom misses (it
claimed `CLK_BUFG_REBUF.GCLK0_ENABLE_ABOVE/BELOW` were absent when decoding the
tile showed both present). Use `work/vbits.py`, which decodes both bitstreams
through the segbits DB and compares feature sets per tile; `work/spine_tiles.json`
lists the spine tiles. For the netlist itself, `work/sim3.py` simulates HNF and
verifies synthesis independently of the bitstream -- settle combinational logic
to a **fixpoint** before sampling or it reports false "STATIC" verdicts.

### Output pads come only from `assign`

The IOB pass walks `rtl.assigns`, so an output port driven **directly** by an
always block gets no pad unless it is handled explicitly (registered-output
ports are now emitted before the single-output fallback). The fallback itself
only fires for a design with exactly one output port, so multi-output designs
silently lost every pad and reported `IO=0` while `done=1`.

IOB relative site names are top-down: the upper IOB in a tile is `IOB_Y0` (see `XDevice::site_rel`). An output IOB's fabric entry is NOT `site.O` -- that is the pad side of the OBUF, and routing to it lands on a neighbouring tile's IMUX. Use `RouteQuery::iob_output_sink`, which resolves the OLOGIC's `D1` input. For LED0 (T22) the entry is `INT_R_X73Y46.IMUX34`, one tile north of the pad's own tile.

### An XDC bit constraint has two spellings, and a dropped one is silent

`led[0]` is what an XDC writes; `led_0` is what `bit_name` gives a vector bit's
net. **Both must resolve**, and there is exactly one mapper that does it:
`helion_xilinx7::build::PinConstraints::apply`. Do not hand-roll the mapping --
the CLI used to carry two extra bracket-only copies, which is how the flat
spelling got dropped on the bitstream path after another path had been fixed.

`helion_sta::Constraints::apply` is a **different** function and wrong here: it
records a whole-port `LOC` through `Design::set_loc`, which cannot express a
per-bit pin (`led[0]` and `led[7]` are different package pins of one port),
while packing reads `LOC[<bit>]` on the vector port to give each bit its own IOB.

The failure mode is the worst kind, so do not trust exit code 0: an
unrecognised key records no `LOC`, packing treats the bit as unconstrained and
drops its IOB, and the build **succeeds** with a full-size bitstream, the full
9996 frames and no output pads at all (1742 FASM, 0 `OLOGIC.OQUSED`). The
existing assertions -- frame count, IDCODE, header package name,
`features_applied > 2000` -- all pass on that broken image, because the baseline
per-tile configuration dominates the count.

The check that catches it is to count placed output sites against the number of
constrained output bits; `crates/helion-xilinx7/tests/output_pads.rs` pins that
for both spellings, that the two agree, that a partly-constrained vector keeps
exactly its constrained bits, and that near-misses (`ledx_2`, `led_foo`) are
still rejected. Verify a new guard fails on the broken code before trusting it.

An IOB whose port bit has genuinely no `PACKAGE_PIN` is **dropped at pack time,
not left unplaced** (Vivado warns and completes). Dropping it later does not
work: the IOB keeps a load endpoint with no driver and routing then dies with
`no wire for load .O`.
