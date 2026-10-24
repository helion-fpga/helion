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

### A pin can land in a tile type that ships no segbits, and that is silent too

A real ZedBoard puts LED6 and LED7 on `U19` and `U14`, which are
`RIOB33_SING_X73Y49` and `RIOB33_SING_X73Y0`. X-Ray ships no `segbits_*` for the
`_SING` variants, so those pads currently emit nothing: our own
`examples/zedboard.xdc` loses 2 of its 8 LEDs and still exits 0 with a
full-size image. The emitter does print a `dropped_summary()`, so
`--test build_bitstream` now asserts `dropped.is_empty()` -- a pre-existing
`features_applied > 2000` bound did not catch this and was removed, because the
baseline per-tile configuration dominates that count and drifts with every
routing change. **Guard the thing that is missing, not a proxy for it.**

Borrowing is possible and `tilegrid.json`'s `alias` says how: it names the base
type and the site remap (`IOB33_Y0 -> IOB33_Y1`). A per-site split of
`segbits_riob33.db` shows why that matters -- the base `IOB_Y0` bits live in
words 2..3 and do not fit a 2-word variant, while `IOB_Y1` lives in words 0..1
and does. The "word 102 outside 0..101" that made this look impossible comes
from adding the *base* tile's word index to the *variant's* offset, which only
the remapped-away site does. So: keep features whose relative word index is
below the variant's word count, and rewrite the site per the alias.

Two traps, both hit in practice. Gate on the presence of an `alias` record, not
on the name -- `variant_base_type` also strips `_TBYTESRC`/`_TBYTETERM`, and
those **do** ship their own segbits. And `LIOB33_SING`/`LIOI3_SING` sit on the
clock edge: writing configuration into them makes the clock spine unroutable
(`net clk_bufg unroutable to 25 loads`). Blinky's image must stay
`ce510391ba5debb119d92137808d7839`.

The borrowing is **not landed**, and the reason is structural. Splitting
`segbits_riob33.db` by which words a feature needs shows the variant is not a
subset but a clean split -- `IOB_Y1`'s 38 features all fit words 0..1, while
`IOB_Y0`'s 43 all need words >=2 -- so a `RIOB33_SING` tile can only ever be
`IOB_Y1`. Selecting the site is therefore the whole problem, and `alias.sites`
is per **tile instance**, not per type:

```
RIOB33_SING_X73Y0 : start_offset=2  sites={"IOB33_Y0": "IOB33_Y0"}
RIOB33_SING_X73Y49: start_offset=0  sites={"IOB33_Y0": "IOB33_Y1"}
```

Both are keyed by the same local name while meaning different things, and
`start_offset` is 2 in a base tile with 2 sites -- it indexes the die-level site
list. `SegBitsDb` is keyed by tile **type**, so a correct borrow needs a
per-instance table or a per-instance site selection at the FASM lookup. Do not
"fix" this by aggregating the alias by type; that is silently wrong for one of
the two tiles. With the word filter in place the emitter fails loudly and
specifically (`no segbits for feature "RIOB33_SING.IOB_Y0....SLEW.SLOW"`) rather
than producing a padless image, which is the right intermediate state.

### Absence of a segbits feature does not mean a pip is unwritable

`Device::pip_is_encodable` exists to keep the router off pips the DB cannot
encode (`LIOI3.IOI_BYP3_0->>IOI_IMUX_RC0` reaches the sink in the model and then
fails at emission). But rejecting every pip with no matching feature breaks the
clock: BUFG's `CLK_BUFG_R_FBG_OUT0` and REBUF's `GCLK0_1_DN_TEST_RING_IN` are
traversed in the working reference and have no feature of their own. The check
is therefore advisory and returns `true`; tightening it needs a positive test
for "described but uncharacterised", never an absence test. A pip's FASM key is
`<TYPE>.<DST>.<SRC>` -- destination first, **with** the type prefix.

### Check the reference's SOURCE before trusting a diff against its bitstream

The ZedBoard ground truth is `~/CDA4253_A1/blinky.bit`, and its source is
**`blinky.vhd`** -- not the SystemVerilog stand-in that comparisons had been
built from. The two are not equivalent, and the difference explains the anomaly
that drove a long investigation:

```vhdl
if rising_edge(clk) then
    count := count + 1;
    LD0 <= count(24);      -- REGISTERED output: 26 carry-chain flops
end if;
```

versus the stand-in's `assign LD0 = count[24]` -- combinational, 25 flops. The
reference has one more flop **because the design has one more flop**. Everything
"missing" was the difference between the two circuits, including the
`NR1END1`-vs-`NE2END1` arm divergence: different source registers land on
different wires. Use `work/ref/blinky_reg.sv` (the faithful equivalent) as the
baseline.

A related counting trap: the same physical slice decodes through all four
`CLB*` tile-type tables. `SLICEL_X1.DFF.ZINI` is frame word 31 bit 59 in
`segbits_clblm_r.db`, `segbits_clblm_l.db`, `segbits_clbll_l.db` **and**
`segbits_clbll_r.db`, so summing over tile types multiplies one flop by four
("104 vs 100"). Deduplicate on `(tile, slice, FF letter)` before drawing any
conclusion from a count.

Before comparing two artifacts, confirm they are supposed to be the same thing.
Round 42 disproved a hand-derived routing topology by asking the node graph;
this disproved a "missing flop" by reading the actual source. Both premises had
been assumed rather than checked.

