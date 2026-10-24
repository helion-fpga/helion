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

### A register and the carry bit it reads have two different names

The single most expensive bug on this flow. `count := count + 1; LD0 <= count(24)`
puts the same bit 24 on the wire under **two spellings**:

```
u_ff_ccount_24  d_net = n_nx_count_24    <- the carry lane sum (next_bit_name)
u_ff0           d_net = count_24         <- the output register (bit_name)
```

`next_bit_name` prefixes `n_nx_`; `bit_name` does not. Every comparison in the
codebase was an exact string match, so nothing linked the chain's output to the
register that reads it. The `LD0` flop was never pinned to the carry lane,
`mux_for_ff` fell through to the bare slice pin `BX`, nothing routed there, and
the register sampled a constant forever.

The image looked perfect on every axis that had ever been checked: 9996 frames,
correct IDCODE, byte-perfect clock spine (0 diffs against Vivado across the
whole device), balanced `ZINI`/`ZRST` (104/104 -- the flop was configured
correctly, it was simply fed nothing), the hardware-proven
`INT_R_X73Y46.IMUX34 <- NR1END1` arm, and an IOLOGIC identical to the
reference. `done` read 1. The LED stayed dark.

`same_bit()` in `emit.rs` resolves the two spellings. It is a name-level
equivalence and sound because the `n_nx_` prefix is generated mechanically, so
a next-state name and its registered counterpart always denote one signal.

**Tell-tale for this class:** a flop that is configured, clocked and reset
correctly but never changes state. The failing question is not "is the flop
right?" -- it is "does anything drive its `D`?". `emit_fasm` now refuses to
emit such a flop, naming the flop, site and mux arm, because a plausible-
looking inert bitstream is far worse than a build error.

Two related traps, both hit for real:

- **A chain in the tile is not a chain in the slice.** A CLB holds a `SLICEM`
  and a `SLICEL`; a carry in one does not feed a flop in the other. A
  site-level "is it driven" test reported `driven` for the dead register
  because both shared `SLICE_X112Y45`.
- **A chain must also be on the flop's own lane.** `AFFMUX.XOR` makes lane A
  the adder lane; a flop on lane B is fed by routing. Four lanes share a slice,
  so matching on the slice alone is not enough.

### Bitstream import: the reverse path is landed

`crates/helion-xilinx7/src/bitimport.rs` reads a `.bit` back into frames:
envelope TLV walk, Type-1 FDRI(count=0) + Type-2 header pair with a
**27-bit** count (`& 0x07FF_FFFF` -- a 31-bit mask reads truncation on any
payload over 128M words, which is every part), then the frame walk with
202 zero pad words between `far_group`s. The Rust masks differ from the
Python reference (`work/x7decode.py`) by exactly those two constants; keep
the two decoders byte-compatible when you touch either. Goldens:
`tests/fixtures/BLINK-242.bit` (1,010,808 payload words; `` frame window
0x422380 w[89]=0x0A420800, w[90]=0x00010000`). The frames walk needs
`HELION_XILINX_DB` and mirrors the writer's rule: walk `part.json`'s
`frame_addresses()`, never the tilegrid union.

### Structure is not behaviour; decode the bitstream and run it

Every structural check on this flow -- frame count, IDCODE, feature diff
against a reference, clock reachability, pad presence, ZINI/ZRST census --
passed on an image whose output register sampled a constant and left LD0 dark
forever. A check that an image is *shaped* right is not a check that it *does*
anything, and this flow has hidden dead designs behind that gap more than once.

`crates/helion-xilinx7/tests/behaviour_counter.rs` closes it: it decodes the
emitted FASM back into a circuit and runs it. The adder model, derived from the
CARRY4 truth table and not assumed, for lane `n` holding bit `n` with the
constant 1 as the LUT's other operand and `CY_n` the carry out of lane `n-1`:

```
S  = bit_n ^ 1        the LUT's XOR output
O  = S ^ CY_n         what FFMUX.XOR drives into the flop
CO = S ? 1 : CY_n     the carry passed to lane n+1
```

Starting from all-zero with `CY_0 = 0` this yields 1, 2, 3, ... Use it as the
model of record. Three ways to get this wrong, all hit while writing the test:

- **Treating `CY` as the incoming carry into the sum** rather than the carry
  out of the lane below. Every bit then toggles independently, no carry
  propagates, and the decoder reports "never advances" for a bitstream that
  demonstrably counts. The reference is what catches it -- keep it as a case.
- **Keying the decode on `(slice, lane)`.** All seven rows' `SLICEM_X0` then
  collapse into one entry and a 26-flop chain reads as 4 lanes. The row is
  part of the key.
- **Assuming the `ZINI` polarity.** Which value a set `ZINI` encodes is an
  X-Ray convention, so try both and require that one produces a counter.

The pattern generalises: when a property matters and the existing checks are
structural, write a decoder that reconstructs the property and run it. Verify
the new check by breaking the thing it guards -- this one was confirmed
non-vacuous by dropping the chain filter (fails, finds 0) and by stopping the
carry from propagating (fails, does not count).

### An unused LUT still has an init, and Vivado picks a different one

The single remaining non-routing difference against `~/CDA4253_A1/blinky.bit` is
one LUT init on an **unused** LUT:

```
CLBLM_R_X71Y39.SLICEL_X1.ALUT
  reference 0xff00ff0000000000
  Helion    0xffffffff00000000
```

It is unused: no `ALUT` output mux selects it and no flop samples it, and both
bitstreams agree exactly on `SLICEL_X1.CARRY4.ACY0`, `NOCLKINV` and
`PRECYINIT.AX`. So this cannot affect behaviour, and it is the *only* such
difference on the whole device -- counting every `(tile, slice, LUT)` init
field across all tile types gives exactly one differing entry.

Worth keeping for two reasons. First, a "16 extra bits in `ALUT.INIT[32..39]`
and `[48..55]`" reading looks alarming and invites a hunt for a writer bug; it
is not one. `frames.rs` already rejects a word index `>= WORDS_PER_FRAME`, and
`CLBLM_R`'s geometry (`offset: 79`, `words: 2`) maps these to words 79 and 80,
comfortably in range -- the segbits frame offsets 26/27 are *not* word offsets
and must not be added to `words`.

Second, Vivado canonicalises the init of a LUT it does not use -- here to a
repeating `0xff00` pattern -- where Helion writes all-ones. If character-exact
bitstream equality is ever pursued, this is one of the differences to
canonicalise, and it is a safe one to change because nothing reads it.

### Instrument the predicate before editing it

`mux_for_ff` picks `XOR` when a carry element publishes the net on the flop's
`D`, else `O6` for a LUT-fed flop. On the registered blinky the LD0 register
emitted `BFFMUX.O6` where the reference emits `DFFMUX.XOR`, which reads like a
broken predicate. A trace printing `d_net` and whether the predicate matched
showed **25 of 26 flops taking `XOR`**, including every counter bit; the single
`O6` is the registered output, whose `D` genuinely is not a carry sum *under
that spelling*. The predicate's structure was right; what it compared was not
(see the `n_nx_` note above). Instrumenting first is what kept this from being
"fixed" in the wrong place -- the actual repair is one equivalence function.

Note also that a register may legitimately sit in a slice that is also carrying
the chain: `AFFMUX.XOR` (chain lane) and `BFFMUX.O6` (the register) are
different lanes of one slice and coexist. Do not "fix" that into a conflict.

The LED output path checks out feature-for-feature against the reference:
`RIOB33_X73Y45` and `RIOI3_X73Y45` are **identical**, including
`PULLTYPE.NONE`, `SLEW.SLOW`, `DRIVE.I12_I16` and `IN_TERM.NONE`. Do not look
for an `OLOGIC.OQUSED` bit to confirm an output pad -- the known-good reference
does not have one either, so its absence proves nothing.

### Where the counter actually lives, and what is left to differ

The 25-bit counter's carry chain runs up column `X71`, `CLBLM_R_X71Y39..Y45`,
four lanes per row, with `SLICEM_X0.PRECYINIT.AX` starting it at Y39 and
`CIN` at every row above. After the `same_bit` fix the chains are **identical
row for row** against `~/CDA4253_A1/blinky.bit`, and the column emits 113
carry/FF features on both sides.

The most recent confirmed state (round 52, after the placements were made to
match and the LED still did not blink) corrects two earlier conclusions here.

First, the placement note below proved to be exactly the thing to chase: putting
the register on the chain's lane A instead of lane D left LD0 lit and motionless
because a CARRY4 lane's `FFMUX.XOR` binds a flop to ITS OWN lane's sum, and lane
A and lane D of a chained row latch different values. Matching the reference's
lane (`CLBLM_R_X71Y44.SLICEL_X1.DFF`, commit `db33e26`) was necessary. It was
not sufficient -- the LED stayed static, so do not treat the placements as
the whole story.

Second, the alt-mux "backbone" is not uniform across the column, which text
diffs of FASM obscure because `BYP_ALT*` arms share coarse config bits with
data arms. Decoded per row against the reference:

- `INT_R_X71Y40..Y44` (middle rows, full CIN-fed CARRY4): identical 13-feature
  template on both sides -- `BYP_ALT{0..7}.GFAN*` ties plus
  `FAN_ALT1.NL1BEG_N3` / `FAN_ALT3.NL1END_S3_0` / `FAN_ALT5.LOGIC_OUTS6`.
- `INT_R_X71Y39` (chain start, `PRECYINIT.AX`): the reference ties
  `BYP_ALT2/3/4/6/7` only, arms `BYP_ALT0` from a data wire (`EL1END0`), and
  configures `FAN_ALT5.LOGIC_OUTS6` but no `FAN_ALT1`/`FAN_ALT3` arm.
- `INT_R_X71Y45` (chain end, one lane): the reference keeps `FAN_ALT1.NL1BEG_N3`
  and the `BYP_ALT1/3/4/6` ties only.

Deleting three mid-row features (`BYP_ALT1.GFAN0`, `BYP_ALT4.GFAN0`,
`FAN_ALT3.NL1END_S3_0`) from the working reference froze LD0 on hardware, so
the middle-row template is load-bearing; the Y39/Y45 differences are the
router's per-row choices on top of it. The four arms into the chain slice's
real operand pins (`IMUX4/12/35/43` -> SLICEM A6/B6/C6/D6) match the reference
since commit `28dfa68`; the four `_L_` arms into `SLICEL_X1` are per-row
routing choices not derivable from the netlist.

The invariant that must never regress: no `BYP_ALT*`/`FAN_ALT*` mux may have
two arms selected anywhere on the die -- prjxray's coarse+fine arm encoding
means two arms select NEITHER input, the output is undefined, and every
structural check still passes. Audit with `python3 work/altmux.py <ours.bit>
<ref.bit>`; both decode, never diff FASM text. Background in
`work/FINDING-round47-backbone.md`, `work/FINDING-round47-arms-reassessed.md`.


