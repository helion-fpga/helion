# Interfaces: UltraScale+ bitgen crew

Contracts between workstreams. Changes need orchestrator sign-off and a
same-day update here. Day-1 contracts from ~/workspace/ultrascale-crew/
INTERFACES.md still apply for pack-place (Placed), router (RoutableNet,
UltraRouted, WireDb), and timing (TimingDb, TimingReport).

## WireDb (router owns, routing-graph implements, integration consumes)

```rust
pub type WireId = u32;

pub struct WireInfo<'a> {
    pub name: &'a str,  // wire name exactly as in the segbits DB, e.g. BOUNCE_E_0_FT1
    pub tile: &'a str,  // Vivado tile name, e.g. INT_X12Y45
}

pub trait WireDb: Send + Sync {
    fn wire_count(&self) -> usize;
    fn fanout(&self, wire: WireId) -> &[WireId];
    fn wire_xy(&self, wire: WireId) -> (i32, i32);
    fn wire_for_bel_pin(&self, site: UltraSite, bel: &str, pin: &str) -> Option<WireId>;
    fn wire_info(&self, wire: WireId) -> Option<WireInfo<'_>>;
}
```

`wire_info` is the wire-ID-to-wire-name seam (CTO ruling 2026-11-07,
option A, on the int-pip naming seam). The router emits wire-ID paths
(`UltraRouted { routes: Vec<Vec<WireId>> }`); the int-pip emitter
consumes named PIPs (`IntPip { tile, src, dst }`). The router-to-PIP
translator (integration-owned) maps each consecutive wire pair in a
route to an `IntPip` via `wire_info` and fails loud on any unnamed
wire. The routing-graph WireDb returns `Some` from its DB-derived node
table; `EmptyWireDb` and the mock return `None`. Mock-routed designs
can never feed int-pip emission: the mock has grid positions only, no
names, and that is expected, not a bug.

Status note (CTO ruling 2026-11-07): the seam does NOT retroactively
block the INT_PIP tier-1 green. That green was for emission (zero diff
against the public segbits DB plus critic re-run) and it stands. The
seam gates the router-to-PIP translator as its own integration item,
tracked separately. The diff harness measures emitted bits, not the
router seam, so INT_PIP is not marked blocked on the seam.

## FASM text (route-to-bitgen handoff)

One line per configured feature, from the day-1 report:

```
# comment lines start with #
<tile>.<feature> [= <value>]
```

`<tile>` is the Vivado tile name (e.g. `INT_X12Y45`, `SLICE_X23Y45`).
`<feature>` is the segbits feature string exactly as it appears in the
segbits DBs (e.g. `PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1`,
`A6LUT.INIT[63:0]`, `AFF.INIT.V0` / `AFF.INIT.V1`). INIT values expand
MSB-first across the listed bits (bit 0 = LSB of the value).

FF INIT canonical spelling is the DB vocabulary `.V0`/`.V1`: exactly
one of the two features per FF, selected by value (e.g.
`CLEL_L.AFF.INIT.V0 12_02`, `CLEL_L.AFF.INIT.V1 !12_02` in
segbits_clel_l.db; SRVAL the same way). The valued spelling
(`AFF.INIT = 1'b0/1`) and the bare spelling (`AFF.INIT`) are
non-canonical and must not be emitted. FLAG (interfaces session 2
item 5): the value-to-feature mapping (which of V0/V1 is INIT=1) is
unverified by any on-disk source; FF INIT stays BLOCKED on a polarity
experiment (a Vivado US+ bitstream with known INIT, or a BitFiltrator
demo.py run reading the bit value at the located FAR and frame_ofst).
Until the mapping lands, FF INIT bits must not be emitted on the
assumption bit=value. The V0/V1 suffix does not uniformly encode the
attribute value: for INIT and SRVAL, V0 sets the bit and V1 clears
it; for CEUSED, CLKINV, and SRINV, V0 clears it and V1 sets it.

## Segbits DB API (segbits-vendor owns, everyone consumes)

```rust
pub struct BitRef { pub frame: u32, pub bit: u16, pub must_be_zero: bool }
pub trait SegbitsDb {
    fn lookup(&self, tile_type: &str, feature: &str) -> Option<&[BitRef]>;
    fn features(&self, tile_type: &str) -> &[String];
    fn tile_types(&self) -> &[String];
}
```

`frame` is a frame offset relative to the tile base FAR (prjxray
segbits convention), not an absolute FAR: the DB is keyed by tile type
only, so one line applies to every tile of that type while absolute
FARs differ per tile. Absolute FAR = `tile_base_far + frame` as a u32
sum, where `tile_base_far` is the FAR of the tile's frame offset 0.
The segbits-vendor loader already produces BitRef lists in this
relative form (DB numbers pass straight through). The lut-ff-bram
bitloc.rs output is (minor, frame_ofst) pairs, not BitRefs, so the
integration step must convert them: `frame = minor - base_minor`,
`bit = frame_ofst`, where `base_minor` is the minor field of
`tile_base_far`.

`features()` exists because `parse_bitstream` must invert the mapping
and `lookup` alone cannot enumerate. Merge requirement (interfaces
session 2 item 3): FileSegbitsDb stores per-tile-type sorted feature
lists built at load, returns an empty slice for an unknown tile type,
and ships a test asserting `features()` enumerates exactly the
features `lookup()` resolves (no missing, no extras).

`bit` is a bit address within the frame (0..2976 for a 93-word
frame). It is u16, not u8: the vendored HPIO/HDIO DBs carry bits up
to 1390, and the u8 loader failed the HPIO load at line 241
(`!02_259`). Ruling 2026-11-07 (orchestrator, on segbits-vendor
98e282a): the u8 bound was factually wrong for the vendored data,
so the contract changes to u16. The widening is backward
compatible: all INT PIP bits (0..47) fit in u16.

Data lives under `crates/helion-ultrascale/data/` in the repo (vendored CC0
prjuray-db segbits plus BitFiltrator arch JSON pointers). The loader never
reads /tmp: reference clones are ephemeral and get wiped by deployments.

## Frame addressing (device-file owns, bitgen-core consumes)

FAR layout, UltraScale+ (UG570 table 9-21, via BitFiltrator arch_spec.py):

- Bits 31:27 reserved, 26:24 block type (0 = CLB_IO_CLK, 1 = BRAM_CONTENT),
  23:18 row, 17:8 column, 7:0 minor.
- Frame size: 93 32-bit words (not 101 as in 7-series).

Device file schema follows BitFiltrator resources/devices/xcku3p.json:
`clb_colMajors`, `clb_tileTypes`, `bram_content_colMajors`,
`bram_reg_colMajors`, `max_far_row_idx`, per-SLR IDCODE, config order.
Every provisional value in the xcvu3p file is flagged `provisional: true`
with its source, until Vivado confirms it.

## Bitgen API (bitgen-core owns)

```rust
pub trait BitgenDevice {
    fn part(&self) -> &str;
    fn idcode(&self) -> Option<u32>;
    fn tile_type(&self, tile: &str) -> Option<&str>;
    fn tile_base_far(&self, tile: &str) -> Option<u32>;
    fn tile_names(&self) -> &[String];
    fn config_order_fars(&self) -> &[u32];
}
pub fn emit_bitstream(fasm: &str, device: &dyn BitgenDevice,
                      segbits: &dyn SegbitsDb) -> Result<Vec<u8>, BitgenError>
pub fn parse_bitstream(bytes: &[u8], device: &dyn BitgenDevice,
                       segbits: &dyn SegbitsDb) -> Result<String, BitgenError>
// parse_bitstream(emit_bitstream(f)) == f, exactly, for every FASM input.
```

`UltraScaleDevice` is a struct (day-1 device model used by pack,
place, and the router), so `&dyn UltraScaleDevice` cannot compile;
`BitgenDevice` is the minimal trait the emitter consumes (part,
idcode, tile_type, tile_base_far, tile_names, config_order_fars),
and it does not duplicate `UltraScaleFabric` (bels_at, wires,
timing, no FAR mapping methods). `tile_base_far` is the FAR of the
tile's frame offset 0; BitRef frame offsets add to it.

FLAG (interfaces session 2 item 4): no real implementation exists
yet. DeviceFile holds the raw data but has no `BitgenDevice` impl
and no `tile_base_far` method; the only implementation is the
provisional StubDevice in bitgen.rs tests. The emitter cannot run
on a real device file until DeviceFile implements `BitgenDevice`.
This blocks FRAMING green on a real device, not the bitgen-core
merge.

`BitgenError` names the tile, feature, and reason. Unknown feature or
unknown encoding fails loud, never emits guessed bits. Bitstream framing:
sync word, FAR sequencing per config order, per-frame CRC, exactly as UG570
specifies. A wrong CRC or a missing sync word is a FAIL, not a warning.

## Diff report (diff-harness owns)

JSONL, one record per block per run:

```json
{"block": "LUT_INIT", "design": "blinky", "part": "xcku3p",
 "diff_bits": 0, "total_bits": 512, "status": "green"}
```

Status is `green` (diff 0), `red` (diff > 0), or `blocked` (missing data,
with a `missing` field naming the exact data). The harness prints the blocks
ordered by diff_bits descending: that ordering IS the greedy work queue.
Ordering rule (harness critic session 5 FLAG 1): order by effective size,
diff_bits when the comparison ran, else total_bits (the size of the work
still standing); ties red before blocked before green, then block name.
A blocked block with 64 unconstrained bits outranks a red block with a
3-bit diff.
Placement and bitstream options are held constant across runs; the harness
records the seed and the placement hash with every run.

## Block registry (diff-harness owns, all workers use these names)

LUT_INIT, FF_INIT, BRAM_CONTENT, BRAM_PARITY, INT_PIP, IO_CONFIG, CLK,
DSP48E2, URAM288, FRAMING. A fix commit names its block. A block goes
green only on zero diff plus a critic re-run.

LUTRAM is a separate block, BLOCKED: LUTRAM emission fails loud
(LutramBlocked) until a LUTRAM table or Vivado logic-location data
exists. It is not covered by LUT_INIT. CTO ruling 2026-11-07: G4
REJECTED, the paper section V-B does not state LUTRAM shares LUT INIT
bits and the arch JSON has no LUTRAM table.
