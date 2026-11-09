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

**Borrowing is landed for the half that fits, and refused for the half that
does not.** The split is bit position, not word index: within the same two
words, `IOB_Y1`'s features sit at bit positions 0..63 and `IOB_Y0`'s at 64..127.
A `RIOB33_SING` tile hosts exactly one half, and `alias.sites` says which:

```
RIOB33_SING_X73Y49  alias {"IOB33_Y0": "IOB33_Y1"}  -> low half   fits, borrowed
RIOB33_SING_X73Y0   alias {"IOB33_Y0": "IOB33_Y0"}  -> high half  cannot fit
```

On the ZedBoard that is LED6 (U19) recovered and LED7 (U14) still unprogrammable
from upstream segbits. `build_bitstream` pins the split exactly, rather than
asserting an empty drop list -- requiring empty would forbid the working borrow
as well as the impossible one.

Three spellings of one site meet here and **none of them match**:

```
FASM / segbits    IOB_Y0
tilegrid `sites`  IOB_X1Y49
alias remap       IOB33_Y0 -> IOB33_Y1   (die-level names)
```

The alias VALUE names the site the base type knows it as, so the lookup must
rewrite the local name wholesale (`IOB_Y0` -> `IOB_Y1`) instead of looking up
or comparing the FASM name. Getting this wrong reads as "does not fit" and
silently drops a pad that would have worked -- it cost several wrong turns here,
including a fit test that rejected the good instance.

The drop decision is per **instance**, never per type: the two `_SING` pads are
one type and land on opposite sides of the split. `TileBitLayout::tile_alias`
resolves it from the tilegrid, and `borrowed_bits_fit` in `build.rs` decides
before the frame write so the writer never aborts mid-image.

Work in **words from the block**, not in bit positions:

```
word = block.offset + start_offset + bitpos / 32
```

`start_offset` was parsed by nobody until recently, and a row-pair of IO tiles
shares one 4-word budget: `RIOB33_SING_X73Y0` has `offset 0` but
`start_offset 2`, so it owns words 2..3 of the pair, not 0..1. `IOB_Y1`'s bits
(pos 0..63, i.e. words 0..1) land on 2..3 and fit; `IOB_Y0`'s (pos 64..127,
words 2..3) land on 4..5 and do not. The fit test is therefore
`bitpos / 32 < 2` **without** re-adding the skew -- adding it in both places
double-counts and gives the opposite answer.

Two wrong turns are worth knowing. First, `bitpos < 64` was used as if bit
positions were word indices; it happened to give the right verdict for the
right pad by accident. Second, on noticing `start_offset` was ignored, the fit
test was "corrected" to `bitpos / 32 < 2` on top of an offset that already
included the skew, which briefly reported LED7 as borrowable. Re-derive the
arithmetic from the two formulas above before changing either.

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

### Block-by-block diffing: the residual is SIX tiles, not 70 bits

`work/blockdiff.py` applies the X-Ray method here (they characterise a region of
interest at a time -- `work/prjxray-db/artix7/harness/README.md`). No Zynq
harness ships, but none is needed: tilegrid + segbits are complete, so a block
is just a tile rectangle.

```
CLEAN BLOCKS: 17 of 19        differing tiles: 6 of 15,000+
```

Everything outside Y32..Y47 is BIT-IDENTICAL to Vivado. The six are
`CLBLM_R_X71Y39`, `INT_L_X70Y39`, `INT_R_X69Y39`, `INT_R_X71Y39`,
`INT_R_X71Y41`, `INT_R_X71Y42` -- and the clock backbone
`BYP_ALT{0..7} <- GFAN0/GFAN1` is IDENTICAL on Y40..Y45. All six reduce to:
(i) the inert unused-`ALUT` init already documented above; (ii) Y39, where
Vivado arms `BYP_ALT0` from DATA (`EL1END0`) and we tie `GFAN0` -- emitted
config, since bottom-row shaping skips `ALT1` but still writes `ALT0`;
(iii) Y41, one contested `FAN_ALT1` (Vivado goes via `NL1BEG_N3`, we burn an
arc into `NR1BEG0` and land `IMUX4` on the cross-row `FAN_BOUNCE_S3_0` --
both sides still reach `FAN_BOUNCE1`); (iv) Y42, one extra `FAN_ALT0.NR1END0`.

"Dead" for the Y39 `ALUT` was verified at the pin level, not assumed: across
that tile's `SLICEL_X1`, **zero** mux segbits are set on EITHER side -- no
`AFFMUX`, `AOUTMUX`, `ACLKINV` or `ALUT.DI1MUX` -- so nothing selects that LUT
and no flop samples it. It is inert in both images. (An earlier note here
claimed `Slicel_X1` has no `AOUTMUX` in the DB; that is wrong -- 12 exist. The
reason it is inert is that none is set, not that the mux is missing.)

**Trap: "no intra-tile pips" does NOT mean dead.** `NR1BEG0` shares the
fed-but-no-outgoing-pips set with `IMUX*`, `FAN*`, `CLK*` -- normal, because
those exits are tileconns/LUT inputs. `NR1BEG0`'s tileconn is `[0,-1] ->
NR1END0`, so it is dead only because **Y40 configures no `NR1END*`**. Follow the
tileconn and check the far tile before calling a wire dead.

**The routing residue is a 4-unit cost tie**, not an unranked rule:
`CLBLM_M_DQ -> IMUX34` is `FAN_BOUNCE1` 140 vs `SR1BEG_S0` 136 (Vivado's pick)
against `DEFAULT_HOP_PS` 45. Adding `SR1BEG_S0` to `arm_penalty` made the image
WORSE (70 -> 84 bits) because that list applies fabric-wide. Rounds 33, 34 and
37 agree from different angles; do not re-open it by ranking.

### `bitsim.py` only reads CARRY-CHAIN flops, so it reports live designs as frozen

`work/bitsim.py`'s `bits()` returns FFs whose mux is `XOR`, i.e. the carry
chain's own lanes. For a design with no registered adder output -- a toggle, or
a counter whose bits are not chained -- `bits()` is **empty**. `state_int()`
then returns `None` (which prints as `0`), the simulator never updates any
flop, and every `D` stays `False` forever. A live design reads as dead.

Confirmed on `examples/hier.sv` (one `q <= ~q` flop) and
`examples/complex.sv` (16 4-bit counters): both reported
`distinct_states=1 transitions=0` from `run_bit`, while `work/sim3.py` on the
same sources shows `hier`'s `led` alternating every cycle and `complex`'s
`u00_cnt` counting 0..7 and wrapping with `tog` alternating. `memsim.py` has the
same shape of flaw, and its `reg` probe is hardcoded to the blinky's
`('L', 44, 'D')`, so it reports nothing meaningful for any other design.

**Use `sim3.py` (HNF, any design) unless the question is specifically about the
blinky's carry chain.** Verify a simulator against a known answer before
believing a "frozen" verdict.

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

### Carry-sum register routing

When an output FF consumes a carry-chain sum and is assigned to the paired slice, its source net must reach both the raw FF D route and the matching lane LUT input (`A6`/`B6`/`C6`/`D6`). The emitter selects `FFMUX.XOR` for this case, so a route only to raw `D` does not feed the carry sum. The regression is `carry_operand_routed::every_carry_lane_operand_is_routed`; the ZedBoard example is `count_24` to `LD0` at `INT_R_X71Y44.IMUX42` / `CLBLM_L_D6`.

#### Route retries must not repeat themselves

A rip-up retry re-runs the identical search against an identical occupancy
set, so it reproduces the same choice and loses the same mux again. On the
25-bit blinky that cost hours: `count_24` and the four Y44 operand nets all
need arms in `INT_R_X71Y44`, and every sweep ripped the others up to take the
same arm back -- 12+ re-entries of the same four nets, no progress, no error.
`route()` now keeps `avoid: net -> wires it lost a contest over`, skips them in
both expansion paths, and clears the set when the net routes. Durable lesson:
**any loop that retries a deterministic search needs per-attempt state**, or it
cannot converge.

Two related traps found the same way. Instrument with explicit file I/O
(`/tmp/NETTRACE.log`), not `eprintln`: in this harness the test binary's stderr
trace never appeared even with an unconditional print, while the file trace was
immediate. And the "same-tile sink floods the A* heuristic" diagnosis is wrong
for this router: `bfs_route_all` already routes all sinks in one multi-sink
A*, and no single search exceeded 10M pops. Measure before accepting it.

#### The reference's operand arms are uniform, and ours are not yet

Decoded per row from `~/CDA4253_A1/blinky.bit`, the reference arms the same
four operand muxes identically on every counter row:

```
IMUX4 <- FAN_BOUNCE1    IMUX12 <- WR1END2
IMUX5 <- FAN_BOUNCE3    IMUX13 <- LOGIC_OUTS6
IMUX42 <- ER1END1       IMUX43 <- WL1END1
```

Y45 (the chain's last row) arms only `IMUX4` and `IMUX43`. We match `IMUX4`,
`IMUX12` and `IMUX43` but still pick `NL1BEG_N3` for `IMUX5`, `FAN_BOUNCE3`
for `IMUX13` and `SR1BEG_S0` for `IMUX42`. Ranking those families in
`arm_penalty` did NOT change any arm, so the choice is decided earlier than hop
cost -- look at sink threading, not the cost model.

The observable consequence is in `work/bitsim.py`: with an alternating seed the
reference's `L`-half lane pins read `1,0,1,0` while ours read `0,1,0,1` -- the
`L` (DI/generate) half is bound to the complement of the bits the `M` half
sees, on every row. Both halves' LUT inits and pin-6 coverage are identical, so
this is an arm/lane-binding difference, not a missing route.

#### SOLVED: the paired half reads the operand one lane EARLIER

That complement was a one-lane offset in the mate endpoints, now fixed by
`mate_lane_target` in `route.rs`. Seed exactly one counter bit into
`~/CDA4253_A1/blinky.bit` and read back every lane pin; the reference is
perfectly regular:

```
bit18 -> MY43C + LY43B     bit21 -> MY44B + LY44A
bit19 -> MY43D + LY43C     bit22 -> MY44C + LY44B
bit20 -> MY44A + LY43D     bit23 -> MY44D + LY44C
                           bit24 -> MY45A + LY44D
```

The L lane is always the M lane MINUS ONE, and lane `A` rolls back into the
**previous** chain element's lane `D` -- not `+1`, and not the same lane. The
old code bound the mate to the same lane, which put every L-half lane one bit
behind the reference and left `count_24` off `LY44D` entirely.

Two wrong guesses were made before measuring: first `+1` (lanes two ahead of
the reference), then reading the earlier `0,1,0,1` pattern as "the complement".
Both looked plausible; only per-bit seeding settled it. **Seed one bit at a
time** for this class of question -- an all-ones or all-zeros seed cannot
distinguish a tie-off from a live value, and alternating seeds hide an offset
that happens to be even.

After the fix our `L`-half map is identical to the reference for bits 18..24,
`carry_operand_routed` passes on its own merits (`count_24` really does reach
`CLBLM_L_D6`), and `work/bitsim.py` traces match the reference digit for digit
from zero-start, mid-count and across the 2^24 wrap, with zero X's.

#### SOLVED (round 29): the counter was frozen because the entry lane read the wrong pin

The blinky's counter never advanced in simulation while every structural check
passed. **Root cause: the carry chain's entry lane wired its operand to the
wrong slice pin.**

The entry lane's `S` LUT is `~A3` (INIT `0x0f0f_0f0f_0f0f_0f0f`); the body lanes
are a plain `A6` pass-through (INIT `0xffff_ffff_0000_0000`). `emit_carry4_map`
put every lane's operand on `I5`, which reaches slice pin `A6` -- correct for
the body lanes, wrong for the entry lane, whose mask reads `A3`. Nothing drove
`A3`, it fell back to the INT tile's VCC default, and `~A3` was 0 for every
state, so **bit 0 could never carry and the chain sat at 0 forever**.

Both images had the identical INIT and the identical `FFMUX.XOR`, which is why
configuration-level checks could not see it. Only the pins differed:

```
GT-blinky.bit   entry lane  pin3 = 0  pin6 = 1   <- consumes A3, A6 floats high
ours (before)   entry lane  pin3 = 1  pin6 = 0   <- reads A6, A3 floats high
```

The fix is `entry_lane = bit == 0` in `emit_carry4_map`: the entry lane's `a`
goes on `I2` (`A3`, `IMUX1`) and `b` on `I3`. The router then drives
`CLBLM_M_A3 <- CLBLM_IMUX1` exactly as the reference does.
`work/bitsim.py` now matches the reference on every seed (zero, wrap, one-hot)
with zero X's, and all 32 `(idx,o5,zini,co,cy0m)` conventions agree.

**Why the existing test missed it.** `behaviour_counter` sweeps BOTH `ZINI`
polarities and passes if either counts. A frozen chain decodes identically
under each polarity, so the sweep cannot distinguish it from a working one. The
new guard `the_entry_lane_reads_its_operand_on_a3_not_a6` asserts the actual
routing (`CLBLM_M_A3.CLBLM_IMUX1` present, `CLBLM_M_A6.CLBLM_IMUX4` absent) and
was verified non-vacuous by reverting the fix and watching it fail.

**Two traps hit while diagnosing this, both worth keeping:**

- **`active_features` adds the tile's `offset` to the word index**
  (`word = geom["CLB_IO_CLK"]["offset"] + bp // 32`). A hand-rolled bit patcher
  that indexes the frame directly writes to the wrong word chain-silently, and
  every "the patch changed nothing" conclusion drawn from it is void. This is
  the same "the word index is already absolute" trap recorded above.
- **Routing hypotheses must be tested one variable at a time, and the
  configuration must be proven identical first.** Two wrong theories (the
  `IMUX4`/`IMUX5` arms, then the `CLK0/CLK1` B6-vs-B11 taps) were each
  disproven by patching and re-simulating. What finally located the defect was
  diffing per-lane pin values rather than per-tile feature lists: the feature
  diff showed six plausible arm changes, while the pin diff showed exactly one
  wrong input on exactly one lane.

#### Rounds 30-34: the leaf, the backbone, and the cost tie

Four leads, all measured, all now closed or accepted. Kept in one place because
they share one lesson: **a ranking rule only works on the cost path that
reaches the edge.** `arm_penalty` is skipped for every clock net
(`use_delay == false`), so clock-edge preferences must live in `leaf_penalty`.

- **R30/31, the clock leaf.** The route took `TOPL0` while only `TOPL5` was
  enabled -- the clock landed on a trunk nothing drove. `leaf_penalty` now pins
  it; every counter row arms `CLK1.GCLK_B11` like the reference. Diff 120 -> 88.
  Guard: `behaviour_counter::the_clock_leaf_matches_the_enabled_trunk` inspects
  the ROUTE, not the FASM, because `vivado_leaf_spelling` rewrites leaf names
  to `TOPL5` on the way out. Re-deriving `TOPL0` is a regression with a measured
  signature (+2 word-50 bits).
- **R32, the backbone is not uniform.** Only interior rows carry all eight
  `BYP_ALT` ties; endpoints are thinner, in different places. `LowEnd::{Bottom,Top}`
  off `clock_row_endpoints` shapes both. Diff 88 -> 70. Do not "simplify" back
  to a uniform loop. Guard: `INT_R_X71Y40..Y44` must keep `BYP_ALT1.GFAN0`,
  `BYP_ALT4.GFAN0`, `FAN_ALT3.NL1END_S3_0` -- deleting exactly those three is the
  hardware-verified way to freeze the LED.
- **R33/34, the residual is a cost-order gap.** At `INT_R_X71Y41.FAN_ALT1` the
  competing sources sit at `g` = 14 (`GFAN1`, the backbone), 91 (`LOGIC_OUTS7`,
  which wins) and 230 (`NL1BEG_N3`, what the reference uses) -- a gap of 139
  against `DEFAULT_HOP_PS` 45. No sane penalty closes that, and forcing it is a
  genuine quality loss traded for a bit-diff. Reverted. Prefer
  `sim3.py`/`blockdiff.py` over re-deriving this.

#### Where the objective stands after round 29

| requirement | status |
| --- | --- |
| functionally identical to Vivado | **MET in simulation** (every seed, 32/32 conventions) |
| bit-for-bit equivalent | **NOT MET** -- 120 differing bits |
| validated on real hardware | **NOT DONE** -- never flashed |

The 120 bits are the classes already attributed above (spine one-hot encoding,
per-row alt-mux arms, one unused-LUT init). None of them affects behaviour:
that is now demonstrated rather than assumed, because the counter increments
identically to the hardware-verified reference on every seed.

Do not treat "simulation matches" as hardware validation. `bitsim.py` is a
model of the fabric written from the X-Ray DB, not the fabric itself, and this
file records two prior occasions where a structurally perfect image was dead on
the board. The only evidence that closes the objective is a ZedBoard flashing
`blinky_reg.bit` and the LED blinking.

#### `bitsim.py --gate` grades the REFERENCE, not your image

`--gate` sweeps conventions against its built-in reference only (it names
`BLINK-242.bit`, which is NOT Vivado -- see the round-19 note; use
`work/oracle.py` to check what any file actually is). It never
opens the bitstream you pass it, so `GATE-LOCKED` is a statement about the
simulator's settings, not a verdict on any image -- and it stays locked while
the image is already correct. The graded path is the positional argument with
NO flag. Note that path's own defaults (`idx=rev`) report the reference as
`not-blinking` too, so it is not a usable oracle as written; compare trace rows
from `run_bit` directly instead.


### A flop's inputs arrive from the INT tile, not the CLB tile

`Routed7::ff_data_is_driven` is the safety net that stops an unrouted flop
being emitted. Its third case ("routing reaches this slice") compared the
pip's tile against the flop's CLB tile name:

```
flop site   CLBLM_R_X71Y51
pip tile    INT_R_X71Y51     <- never equal
```

7-series keeps the slice and its input muxes in **separate tiles**, so that
comparison could never match and the whole branch was dead. Any design whose
flop `D` came from routing rather than from its own slice was rejected with
`takes AFFMUX.AX ... but no routed net ends at that slice` while the net was
demonstrably routed to `INT_R_X71Y51.IMUX43`. `internal_clock`'s
`internal_clock_reaches_a_bitstream` had been failing on this for a long time
and looked like a clock or mux-selection bug; the mux was right and the routing
was right.

`int_tile_for_clb` maps `CLBLM_<L|R>_X<n>Y<m>` / `CLBLL_<L|R>_...` to
`INT_<L|R>_X<n>Y<m>` by suffix. Verify any change here against
`undriven_ff_guard`, which still has to fire on a genuinely dead flop -- a
permissive `ff_data_is_driven` turns the one guard against silent dead designs
into a no-op.

### A register file's COMBINATIONAL read port was silently missing

The write side of a small unpacked array lowers to one flop per bit
(`<mem>_w<word>_<bit>`), and the clocked read (`dout <= mem[addr]`) worked. The
COMBINATIONAL read did not:

```systemverilog
always_ff @(posedge clk) if (we) mem[addr] <= din;
assign dout = mem[addr];          // <- no read logic at all
```

The assign loop bails on anything `rhs_unpacked_index` reports, on the grounds
that walking a wide cone into a >96 AND term is not a LUT. True for a huge
array, wrong for a register file: at the caps the write side already accepts
(depth<=16, width<=32, depth*width<=128) the read is a mux over one-hot selects,
which is exactly what LUT6s are for. So a perfectly ordinary 16x8 register file
could not be built.

**The failure mode was misattributed, which is what let it live.** The array
itself lowered fine -- flops existed, writes landed -- so the design looked
almost built and failed at PACKING with

```
run: 8 output pad(s) have no driver after packing and cannot be placed: dout_0 .. dout_7
```

Nothing in that message mentions the memory. The real diagnostic
(`variable_index_read module=regmem signal=dout`) was on stderr and read like a
timing note.

Fixed by `unpacked_read_mux_expr` in `helion-sv`: for an array whose write side
is in `lowered_unpacked`, each `dout` bit becomes
`(addr==0)?w0:(addr==1)?w1:...:w<depth-1>`. Depth 1 qualifies too -- a one-word
array is still storage and its read is a pass-through; requiring depth > 1 left
a 1x4 memory undriven with the same misleading message.

**Two traps when writing the regression test** (`tests/unpacked_variable_read.rs`),
both of which made an early version vacuous:

- `endpoints.is_empty()` is ALWAYS false for a top-level output, because port
  binding attaches an endpoint even with nothing driving it. The probe showed
  `dout_0 ep=[u_iob:I, u_iob:PAD]` -- two endpoints, no driver. That is
  precisely how the defect hid.
- The net carrying `dout_i` is not always NAMED `dout_i`: a constant index
  ALIASES the output onto `<mem>_w3_0`, so `dout_0` is then only the pad side.

So follow the wire instead: find the IOB whose `PAD` endpoint is on the port,
then check whether the net on that IOB's `I` pin has a LUT6/HFF endpoint. This
mirrors the packer, whose test is membership in the packed LUT/FF Q set. Verify
any such guard by disabling the fix -- done here: with the new block forced off,
exactly the two guard tests fail and the three controls still pass.

### SOLVED: a register file read back the TOP word for every address

`unpacked_read_mux_expr` builds `mem[addr]` as
`OR over words w of (addr == w) ? mem_w : 0`, descending from the top word.
The top word was emitted **bare**, with no `addr == depth-1` guard, so it
became an unconditional OR term of every other word:

```
depth 2:  mem1 | (mem0 & ~addr)      instead of      addr ? mem1 : mem0
```

Every word needs its guard; `acc` now starts at `Const(false)` and every word
goes through the same `Or(And(sel, word), acc)`. The fixed-xor against the
symptom: `addr=0` returns `0xFF` whenever the top word is all ones.

**Two harness bugs nearly hid this**, and both are the same class -- a
simulator that agrees with itself.

- The evaluator matched a gate's output net by its CELL name, but the output
  net is whatever the `O` pin drives (`u_wshare_l41` drives `u_wshare_n41`).
  It read nothing and reported every word wrong for the wrong reason.
  Resolve outputs through the `O` pin endpoint.
- The truth table never hit the failing row. The corruption is "top word leaks
  in", so it shows only when the TOP word holds a 1 that a lower-addressed
  word must mask off. My first table set the top word to `0x00`, which is
  exactly the value that cannot expose it, and every depth passed against
  known-broken code. **Pick the table from the failure condition, not from a
  hash of the index.**

Validate any such harness on a case whose answer is known first: a plain
`d = d[s]` 4:1 mux was enough to prove the evaluator sound before trusting it.

The regression is `tests/unpacked_variable_read.rs::read_returns_the_right_word_*`
(depth 2/4/8/16). It was verified non-vacuous by running it against the
pre-fix source: all four fail there and all pass after.

### Known gap: no BRAM inference in `helion-sv`

`pico_acceptance::picorv32_with_bram_end_to_end` fails at `synth_sv`, before any
back-end work: a depth=32/width=8 memory exceeds the register-file limits
(depth<=16, width<=32, depth*width<=128) and there is no block-RAM lowering to
fall back on. Refusing loudly is correct -- a half-lowered memory would carry
read data with no driver -- so this is a missing feature, not a regression.
Do not "fix" it by raising the register-file limits: that turns one BRAM into
thousands of flops.

### `BLINK-242.bit` is NOT Vivado, and two one-hot muxes were double-armed

Two findings from round 19, both found only because the oracle was finally
checksummed. Run `python3 work/oracle.py` before trusting any comparison.

**The oracle was misidentified.** `work/ref/BLINK-242.bit` was used as the
golden for several rounds; it is a *Helion* build (md5 `3cdb9a46`, 4045681
bytes, BD payload 3,369,061 -- identical to `OURS-r25.bit`). Vivado's output is
`work/ref/GT-blinky.bit` (md5 `3f6d7cfa`, **4045666** bytes, payload 2,646,636),
and a second independent Vivado build (`GT2-blinky.bit`) has the same size. So
size alone identifies the producer, and a "diff against BLINK-242" was
Helion-vs-Helion -- the same circular-result class as `/tmp/ref/blinky.bit`.
`OURS-r25.bit` is also *dead* (`[0,0,0,0,0,0]` from zero start), so nothing
should be calibrated against it either. `work/oracle.py` now refuses any file
that is not Vivado's, by md5, and exits non-zero.

**`FAN_ALT` arms were double-selected.** `emit.rs` wrote the `FAN_ALT{1,3,5}`
backbone unconditionally while the router could *also* arm `FAN_ALT1` from a
data wire (the clock net on Y39/Y41). prjxray encodes these as one-hot
coarse+fine, so two arms select **neither** input and the output is undefined
-- while every structural check passes. `work/altmux.py` reported 2
double-selected muxes against Vivado's 0. The `BYP_ALT` loop directly above
already guarded with `routed_mux_in_use`; the `FAN_ALT` copies did not. Fixed
by skipping only the mux the router legitimately took. Now 0 on both sides.

**`arm_penalty` was one hop short.** The LED pad's tile (`INT_R_X73Y46`) must
take `IMUX34 <- NR1END1`; we took `FAN_BOUNCE1`. Instrumenting the two
candidates showed why:

```
FAN_BOUNCE1  cur_g 298 + pen 45 = 343
NR1END1      cur_g 343 + pen  0 = 343     <- exact tie
```

The preferred vertical arm arrives exactly `DEFAULT_HOP_PS` later, so a
one-hop penalty only tied and the search broke at whichever reached the sink
first. The unranked penalty is now **two** hops. Alternatives stay reachable
(they merely have to be the only way in), `INT_R_X73Y46` became IDENTICAL to
Vivado, `INT_R_X73Y45` gained its `NR1BEG1.ER1END1` stitch, and `clkdiff.py`
went from 1 REAL diff to **0**.

Lesson: when a preference is expressed as a cost margin, measure the actual
gap before choosing the margin. "One hop" sounded like enough and was
arbitrary; the trace gave the exact number in one run.

### Word 50 of clock tiles: Vivado sets it, no segbit names it

The largest remaining gap to bit-for-bit equality, and it was invisible to
every tool in this repo until round 20.

`work/ref/GT-blinky.bit` sets word 50 of **52 frames** across the clock
network to a per-tile value, most of which no segbit names.

These are NOT all clock tiles. An earlier enumeration used `break` inside a
loop over block types and attributed one tile's value to several, which made it
look as if every clock tile were nonzero. Re-derived properly against the
reference, word 50 is nonzero on only these two under `HCLK*` types:

```
HCLK_L_X174Y26  0x49ae
HCLK_R_X175Y26  0x103e
```

The 350 bits are spread over **52 frames** (the spine spans many frames per
tile, including `CLK_HROW_BOT_R` and `CFG_CENTER_MID`), and Helion writes 0x0
to most of them. We do already set 5 of the 52, each a single bit that is a
subset of Vivado's value -- so we emit part of this field, just not most of it.

**One wrong alternate is FIXED (round 22).** On `HCLK_L_X174Y26` Vivado sets
all eight `HCLK_LEAF_CLK_B_TOPL5.*` features for its buffer and none of
`TOPL0`; Helion set the mirror image, because the router reaches `TOPL0`.
`TOPL0..TOPL5` are six sibling leaf wires with no outgoing pips -- every
buffer/spine source reaches all six (`tile_type_HCLK_L.json`), and they
terminate in the tile, so naming one only decides which segbits get written.
That does **not** make the spellings free: the leaves collide with the
positional word-50 field (`TOPL0` sets word50 bit29 of frame `+1` and bit31 of
`+5`, `TOPL5` sets bit25 of `+3` and bit23 of `+5`), so emitting the router's
`TOPL0` on top of `CLOCK_WORD50_POSITIONAL` ORs two extra bits into frames the
reference leaves at zero. Normalising to `TOPL5` is therefore correct, not
cosmetic -- see the round-30 note below.
Normalised by `vivado_leaf_spelling` in `emit.rs`. That tile is now
feature-identical to Vivado and the image went 474 -> 470 bits at exactly
Vivado's file size.

Two traps on the way there, both real:

- **Fix it at the pip line, not in `clock_companions`.** The leaf arrives as a
  NON-pseudo pip whose FASM line is written from `dst_wire`. A companion-side
  change ADDS the `TOPL5` feature and leaves `TOPL0` in place -- verified: three
  features on the tile instead of two, diff stuck at 474.
- **`format!("{PREFIX}5{}", &wire[PREFIX.len()..])` yields `TOPL50`.** The
  slice after `strip_prefix` still holds the leaf digit. That panics at frame
  assembly with `no segbits for feature ..._TOPL50...`, and because the library
  had not relinked it kept reporting the OLD component's error while the source
  read correctly -- force a `cargo build -p helion-xilinx7` before believing
  that a string-level fix had no effect.

**What is left in word 50 is NOT ours (verified round 23).** Attribute the 52
remaining frames by EXACT block extent (`base + offset .. + words`), not a
`base .. base+200` sweep -- the sweep mis-attributes badly and is what made an
earlier round think our own tile was still wrong. Doing it exactly:

```
tiles our design actually uses       differing frames
  HCLK_L_X174Y26                          NONE
  HCLK_R_X175Y26                          NONE
  CLK_HROW_BOT_R_X87Y26                   NONE
  HCLK_CMT_L_X178Y26                      NONE
```

**SOLVED (round 24): word 50 is a positional table, and it now matches.**
Diff went **470 -> 120 bits**, unnamed bits 350 -> **0**, `clkdiff` unnamed
136 -> **0**, at the same 4,045,666-byte file size. `apply_clock_word_50` in
`frames.rs` writes it after `apply_fasm`, from `CLOCK_WORD50_POSITIONAL`.

**The key MUST include the far-group.** This is the trap that cost a round. An
offset-only key looks right because 50 of the offsets carry one value across
groups -- but an offset repeats ~5x per row, so an offset-keyed table wrote
**256** frames where Vivado writes **52**, and the diff went the wrong way
(470 -> 2352). Each far-group has its own set of programmed offsets and only
`0x9b` appears in several. Key is `(far_group, offset)`; `far_group` is
`addr >> 12` for this grouping (`frames::far_group` is a different split --
do not swap them).

It is NOT a constant and NOT "unused-tile defaults": 42 distinct patterns, and
group 0x422 programs 42 frames while 0x401 programs 3. Our own word-50 bits were
always a strict subset of Vivado's, which is why leaving the rest at zero still
produced a working image -- the table is for bit-identity, not for function.

**Round 27: the 120 bits decompose into three classes, and none is a defect.**

Bit-level split of the residual diff (decode, then classify per cluster):

```
320 bits  CLK0/CLK1 spine-encoding bits   the B6-vs-B11 one-hot group:
                                          choosing B6 sets a different coarse
                                          bit, so ONE selector choice shows up
                                          as many mismatched bits
1032 bits FAN_ALT*/IMUX* arms             per-row router choices on Y39/Y45
 16 bits CLBLM_R_X71Y39.SLICEL_X1.ALUT   unused-LUT canonicalisation
```

Three things worth recording, each measured rather than assumed:

- **`IMUX10` vs `IMUX5` on Y39 is not a mis-route.** Vivado arms
  `IMUX10.LOGIC_OUTS5`, Helion arms `IMUX5.FAN_BOUNCE3`. `INT_R_X71Y39` has
  **no `OUTMUX` feature at all**, so nothing drives `LOGIC_OUTS5` -- Vivado's
  arm is a dangling stub, and `LOGIC_OUTS5`'s only driver would be a slice
  output mux in the same tile, which is not configured. Do not chase this to
  "our router picked the wrong pin": both muxes are unused for this design.
- **`CLK1.GCLK_B6` vs `GCLK_B11` produces a 320-BIT diff from ONE choice.**
  The arm is one-hot coarse+fine, so `B6` and `B11` share no coarse bit --
  every shared multiplexer bit differs. This is why the residual looks like
  "hundreds of routed-arm errors" when it is a single selector decision per
  tile. Both reach `CLK1` with identical 0.058 ns delay.
- **The Y39 `ALUT` init is NOT a DI function and NOT the `0xff00` canonical
  form.** Solved from the reference bits: `0xff0fff0f0f0f0f0f`, which depends
  only on `{A3, A4, A6}` = `A4 AND (A6 OR !A3)`. It is **unused** -- there is
  no `OUTMUX` feature anywhere in that tile and all four lanes take
  `AFFMUX.XOR` from the CARRY4 -- so Vivado is canonicalising the init of a
  LUT nothing reads. Do NOT hardcode this value: it is Vivado's choice for an
  unread LUT, and the earlier `0xff00` note in this file is a *different*
  case, so the canonical form is not even constant.

**Round 26: the last 120 bits are row-specific alt-mux choices, not a bug.**
After word 50 the diff is 120 named bits, all on the X71 counter column's INT
tiles. Decoded per row, `BYP_ALT{0..7}` is **identical on both sides for
Y40..Y44** (all eight arms, route and reference). The only divergences are the
chain's ENDPOINTS:

```
Y39  ref: ALT0->EL1END0, ALT1 absent, ALT2/3/4/5/6/7 tied
     our: ALT0->GFAN0,   ALT1->GFAN0,  ALT2..7 tied
Y45  ref: only ALT1, ALT3, ALT4, ALT6
     our: all eight
```

`altmux` reports 0 double-selected on BOTH sides, so this is not the one-hot
hazard coming back -- it is that Vivado arms `BYP_ALT<n>` from a DATA wire
(`EL1END0`) where the router happens to reach it, and leaves the arm blank when
nothing reaches it. That is real per-row routing, not fixed configuration, so
it cannot be derived from a topology rule; matching it means matching the
router's choice of data net on those two rows, and the same is true of the
`CLK1.GCLK_B6`-vs-`B11` tap (`GCLK_B6` and `GCLK_B11` both reach `CLK1` with
identical 0.058 ns delay -- a free selector choice in a 12-way mux, same class
as the `TOPL0`/`TOPL5` leaf). **Do not "fix" either by hardcoding an index**:
the mapping is per-design, and a hardcoded `B11` breaks any design whose clock
lands on a different spine.

Three traps, all hit:

- **`clkdiff.py` cannot see it.** Its `scan()` compares only NAMED segbits
  features, and `segbits_hclk_r.db` / `segbits_clk_hrow_bot_r.db` have no
  entry for word 50 at all. So it printed "identical tiles: 180 of 181, REAL
  diffs: 0" while every one of those tiles differed here. A clean clkdiff is
  NOT evidence of byte-identity. It now reports
  `unnamed clock words differing: N` alongside the verdict.
- **The word index is already absolute.** `CLB_IO_CLK.offset` is 50 with
  `words: 1` for `HCLK_R_X171Y26`, and the frame is `baseaddr + frame_offset`
  (rawdiff's `0x42228b`), NOT `baseaddr`. Indexing the frame at `offset` after
  adding `offset` to the address double-counts and reports a false 0.
- **`baseaddr` is a hex STRING** in the tilegrid, not an int.

Vivado is deterministic here: `GT-blinky.bit` and `GT2-blinky.bit` (two
separate builds of this source) agree on word 50 in every tile, and the two
bitstreams differ ONLY in a 4-byte timestamp in the header. So a fixed map
does exist and is matchable — this is tractable work, not an unknowable.

### The residual is FIVE tiles, and 46 of 70 bits are ONE coupled decision

Block-by-block on the registered blinky (`8bb37a4c`, 4,045,666 B) against the
Vivado oracle gives **70 bits in 5 tiles**, not six:

```
46  CLBLM_R_X71Y39     16  CLBLM_R_X71Y41      4  CLBLM_R_X71Y42
 2  INT_R_X69Y39        2  INT_L_X70Y39
```

**One decision explains 40 of the 70.** At `INT_R_X71Y39` Vivado arms
`BYP_ALT0` from a DATA wire (`EL1END0` -> `BYP0` -> the slice's A4 path); we
tie it to `GFAN0` from the clock backbone. `INT_R.EL1END0->>BYP_ALT0` is a real
pip in `tile_type_INT_R.json`, so both paths exist and both work. Everything
else in that tile follows from it: the unused `ALUT` INIT, `IMUX34`, `FAN_ALT1`
and `FAN_ALT3` all differ *because* the source of A4 differs.

That is why the `ALUT.INIT` cluster cannot be fixed on its own. Our init is
`0xffffffff00000000` (depends on `I5`/A6 only); Vivado's is
`0xff00ff0000000000` (depends on `I3`/A4 **and** `I5`/A6) = `A4 AND A6`. The
extra `A4` term is not a canonicalisation choice -- it is Vivado's LUT seeing a
different input because `BYP_ALT0` feeds A4 from `EL1END0`. Two attempts to
"fix" the init in isolation both failed and are recorded here so they are not
retried:

- `canonical_unread_lut_init` keyed on `PackedLut` never fired: the ALUT init
  is not emitted from the packed-LUT loop at all, it comes from the carry
  element's `lane_di_init[0]` in the DI half.
- `canonical_unread_carry_di_init` ANDing the DI init's own dependencies was a
  no-op (the init depends on one input, so ANDing it returns it unchanged) and
  the variant that used `lane_operand_pins` made the diff WORSE (70 -> 86)
  because those pins come from the S LUT, not the DI LUT.

So: **route `BYP_ALT0` from the data wire first, then the init follows.** Do not
touch the init before the arm.

Also settled this round, so it is not chased again: Vivado is **deterministic**.
Six identical parallel builds produced byte-identical fabric payloads; the only
differing byte was offset 93, the header timestamp. And a fresh Vivado build of
the same design puts the counter in the SAME slices as ours
(`CLBLM_R_X71Y39..Y45`) -- our 70-bit diff is **100% routing, 0 placement**.
The 1,500-bit gap between two *Vivado* builds (old oracle vs fresh) is synth
directive variation, not router nondeterminism, so "bit-identical to Vivado"
must always name which Vivado invocation it means.

### The Y39 `BYP_ALT0` arm is a DANGLING STUB in the oracle, not a route we missed

The previous entry said "Vivado arms `BYP_ALT0` from a data wire (`EL1END0`),
so route it and the ALUT init follows". That is wrong, and the DB says so
plainly — this is worth having measured rather than chased.

`EL1END0` **has no feeder at all**, in the tile or across any boundary:

```
pips with dst_wire == EL1END0        []          (in tile_type_INT_R.json)
tileconns mentioning EL1END0         none        (all INT_L/INT_R types)
pips with src_wire == EL1END0        20          (it only ever fans OUT)
```

A wire with fanout and no driver is permanently floating, so
`BYP_ALT0 <- EL1END0` drives the bypass mux from nothing. This is the same
class as the `IMUX10.LOGIC_OUTS5` stub already recorded above: Vivado routes
to a source that is not connected, and every structural check still passes
because the *bit* is set.

Our `BYP_ALT0.GFAN0` tie is therefore not a defect — it is strictly better than
the oracle at that bit, and the 46 bits in `CLBLM_R_X71Y39` are us being more
correct, not less. That reframes the "40 of 70 explained by one decision"
finding: the cluster is real and coupled, but the coupling runs the other way
from what was assumed. Do NOT implement `EL1END0` routing to close it — there
is nothing to connect to.

Consequence for the objective: the residual is no longer 70 bits of "work to
do". Split it honestly:

```
46  CLBLM_R_X71Y39   oracle arms a floating wire; we tie GFAN0    (we are right)
20  Y41/Y42/Y69/L70  per-row router choices on a 4-unit cost tie  (A-vs-B)
 4  Y39 leftover     inert ALUT init, unreachable without the above
```

**Only the 20 are a genuine A-vs-B routing choice.** Reaching literal 0
requires reproducing Vivado's stub, which would make the image *less* correct.
Report the number with that qualification instead of treating it as a defect
count — and if a future round is asked to "get to 0", the honest answer is that
20 of the bits are free to match and the rest are a deliberate divergence that
should be documented, not removed.

### Full classification of the 70-bit residual (measured, by fanout)

Every remaining cluster has now been traced to the wire in
`tile_type_INT_{L,R}.json`, so the residual is classified rather than
speculated about. **56 of the 70 bits are on wires with no fanout at all.**

```
tile / wire            ours  ref   fanout                       class
CLBLM_R_X71Y39 BYP_ALT0  GFAN0 EL1END0  EL1END0 : 0 pips, 0 tcs   dead wire (ref)
INT_R_X69Y39  ER1BEG1    none  SR1BEG_S0 ER1BEG1 : 0 pips, 0 tcs  dead wire (ref)
INT_L_X70Y39  EL1BEG0    none  ER1END1   EL1BEG0 : 0 pips, 0 tcs  dead wire (ref)
CLBLM_R_X71Y42 FAN_ALT0  extra arm     FAN_ALT0: 2 pips, 0 tcs    live, we over-arm
```

"0 pips, 0 tileconns" from the SOURCE side means the wire drives nothing
anywhere on the die. Selecting it is configuration with no functional effect,
exactly like a `VCC_WIRE` tie. So:

- **46 (Y39)** — oracle arms a floating wire, we tie the real clock backbone.
  We are *more* correct; do not "fix".
- **2 (Y69) + 2 (L70)** — oracle arms a wire that drives nothing; we arm
  nothing. Functionally identical images, encoding differs only. Matching it
  is cosmetically safe but buys nothing: it would set a mux bit for a stub.
- **4 (Y42 `FAN_ALT0`)** — the opposite direction: we arm an extra
  `FAN_ALT0` that has a real 2-pip fanout. This is the one place we might be
  wrong, and it is worth checking whether that arm perturbs `FAN0`/
  `FAN_BOUNCE0` for the design.
- **16 (Y41 + Y39 leftover)** — genuine A-vs-B router choices on the 4-unit
  cost tie already documented; free to match, no correctness impact.

**VERIFIED by patch (round 2):** `work/bitpatch.py` on three dead-wire bits
(`0x42228b:79`, `0x42230d:79`, `0x422308:79`) takes the diff **70 -> 67**,
`altmux.py` still reports 0 double-selected on both sides, file length
unchanged and readback matching. So the dead-wire bits are independent of
everything else and setting them is side-effect-free -- the theory holds
mechanically, it is only the *wisdom* of emitting them that is in question.
**ANSWERED (round 3, small-design campaign): it is systematic, and we already
do it too.** Four fresh Vivado builds of different circuits on the cluster
(`t01_ff` one toggle, `t02_cnt4` 4-bit counter, `t03_and` LUT-only, `t04_cnt8`
8-bit counter) were decoded for "dead-end mux arms" -- arms of `ER1BEG1`/
`EL1BEG0`, which are muxes with 20 input arms and **zero outgoing pips and zero
outgoing tileconns**, i.e. they drive nothing:

```
t01_ff    1 dead arm        t04_cnt8  2 dead arms
t02_cnt4  1 dead arm        t03_and   0 dead arms (almost no routing)
oracle    9 dead arms       OURS      7 dead arms
```

So this is **routine behaviour, not a blinky quirk** -- and Helion already does
it 7 times on blinky without any special case, purely as a side effect of the
router's choice. The 2 arms we miss (`ER1BEG1` on X69Y39, `EL1BEG0` on X70Y39)
are per-design noise, not a rule we are failing to implement. **Do not build an
emit rule for them**: there is no general rule to find, and a per-tile table
would not transfer to another design.

Corollary worth keeping: a wire being a "dead end" is about OUTGOING edges
only. `ER1BEG1`/`EL1BEG0` have 20 incoming arms and 0 outgoing; `EL1END0` is
the mirror (0 incoming, 20 outgoing). Checking "is it in `dst_wire`" alone
answers the wrong question in both directions -- count both sides and say which
you mean.

Also worth knowing before re-deriving it: `FAN_ALT0` on Y42 is the ONE cluster
where the ROUTER, not the backbone, is responsible -- `emit.rs` only ever
writes `FAN_ALT{1,3,5}`, so `BYP_BOUNCE_N3_2 -> FAN_ALT0` is a real hop the
router took and Vivado avoided. The pip's delay is `[0,0,0,0]` and `FAN_ALT0`
has real fanout (`FAN0`, `FAN_BOUNCE0`), so this is a routing-efficiency
difference, not a bug, and it is the closest thing in the residual to a
genuine QoR question.

Two traps in checking "is this wire dead", both hit this round and both give
the WRONG answer if used alone:

- Counting a wire as live because it appears as a `src_wire` in `pips` is
  backwards -- that is what a dead wire does (it has fanout and no driver).
  Liveness needs an incoming edge: a pip with `dst_wire == W`, or a tileconn
  with `dst == W`.
- Searching all `tile_type_*` files for the bare wire name crosses tile
  instances. `INT_L.EL1END0` being driven says nothing about
  `INT_R.EL1END0`; they are different wires on different tiles. Scope the
  check to the tile type being questioned.

**Practical consequence:** chasing the literal 0 down from 70 mostly means
copying Vivado's arbitrary selection of dead-end wires. That is achievable —
`emit` already special-cases the Y39 bottom row — but it is *encoding
matching*, not a correctness or QoR gain, and each such rule is one more
per-design special case (`EL1END0`, `ER1BEG1`, `EL1BEG0` are three different
wires in three different tiles). The honest framing for a "0 diff" objective
is: **the 20 live bits are the real target; the 50 dead-wire bits are a
documented, deliberate divergence.**

Verify fanout before matching any arm: `src_wire` in `pips` plus `wire_pairs`
in `tileconn` — a wire absent from both drives nothing.
