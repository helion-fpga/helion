# xczu3eg timing sources audit

Date: 2026-11-24 (first lane) and 2026-11-25 (follow-up).
Lane: xczu3eg timing (fm/ultra-bitgen-xtiming-impl),
base f6a606c8 (the N58 lane tip). Workers: W-IMPL, W-IMPL-FOLLOWUP.

## Verdict: (A-partial)

Exactly one documented xczu3eg value now exists: BRAM CLK to DOUT,
1020 ps, from DS925 v1.30 Table 80 TRCKO_DO at -2 / 0.85V, read
directly from the DS925 document by the follow-up lane (see the
Block RAM section below). It is recorded in
`documented_xczu3eg()` in `crates/helion-ultrascale/src/timing_db.rs`.
Every other xczu3eg DB field stays Ballpark or UNKNOWN, the
preliminary xczu3eg DB stays an honestly labeled estimate, and no
fabric delay (LUT, FF, CARRY8, interconnect, DSP, URAM, IOB) has a
documented public source. The fabric-delay unblocker is unchanged:
Vivado `report_timing` plus `write_sdf` on a real design.

## Prior verdict (2026-11-24): BLOCKED

No public source retrievable by this lane carries a documented xczu3eg
fabric delay (LUT delay, INT hop delay, FF setup/hold, FF clock-to-out)
that can go into the timing DB, and the one value that should exist in
public documentation (DS925 Block RAM TRCKO_DO) could not be read from
DS925 itself with the tools available to this lane. No number was
invented, no value was transplanted from a sibling datasheet, and the
DB code is unchanged except for a doc-comment pointer to this file.
The preliminary xczu3eg DB stays an honestly labeled estimate.

Two unblockers, in order of cost:

1. Cheap, for the single BRAM value only: read the DS925 Block RAM and
   FIFO Switching Characteristics table from the live DS925 document
   (PDF or docs.amd.com page) with a working browser and record the
   TRCKO_DO value at -2 / 0.85V plus the PDF table number. That would
   support a `documented_xczu3eg()` with exactly one value, parallel to
   `documented_xcku3p()` and `documented_xcvu3p()`. It would not touch
   any fabric delay. DONE 2026-11-25 by the follow-up lane: see the
   Block RAM section below.
2. Real, for the fabric delays: Vivado `report_timing` plus `write_sdf`
   on a real design, per GOAL.md. Vivado is not available on this VM
   and Tier 2 waits for it. There is no public document that can
   substitute for this step. STILL OPEN.

## How the sources were checked

Method: `browser.search` text queries against the public AMD
documentation site (docs.amd.com) and public PDF mirrors, plus a
local filesystem check for any vendored DS925 copy. Every quoted
table value below comes from a search-result snippet of the named
public page, crawled within the last 80 days at check time.

Limits of the first lane, stated plainly (superseded in part by
the follow-up read, see below):

- One `browser.open` attempt on the DS925 landing page
  (https://docs.amd.com/r/en-US/ds925-zynq-ultrascale-plus) failed
  with a local worker error (no source HTTP status) and was not
  retried, per the runtime instruction on that failure. No DS925 page
  text was fetched directly in that lane.
- `browser.search` surfaced the full table text of the sibling
  datasheets DS922, DS923, DS926, and DS930, and of several DS925
  pages (DC, AC, Configuration, Clock Buffers, Pin-to-Pin,
  Input/Output Delay), but never the DS925 Block RAM and FIFO table
  itself across more than ten targeted queries. The DS925 BRAM page
  exists (the landing page lists the section) but its table text was
  not retrievable through search in that lane.
- No local DS925 copy exists on this VM: a filesystem search for
  `*ds925*` under `~/workspace` and `~/memory` returned nothing.
- Web fetching through the shell (curl and similar) was not used: web
  reading goes through the browser tools only.

Follow-up read (2026-11-25): the follow-up lane fetched the DS925
BRAM table directly, twice. First the current v1.30 docs.amd.com
page (Block RAM and FIFO Switching Characteristics), then the
v1.23 DigiKey PDF mirror, full text, Table 80 on page 65. Both
versions agree value-for-value; see the Block RAM section. The
limits above therefore no longer apply to the BRAM table, and
only to the fabric-delay sections, which were checked by search
snippets only.

DS925 versions seen in public sources during this lane: current
revision 1.30 English, release date 2026-07-09 (docs.amd.com landing
page); v1.23, February 28, 2023 (DigiKey PDF mirror); v1.19, June 23,
2021 (S3 PDF mirror); v1.14, November 15, 2018 (allicdata PDF mirror);
v1.12, June 18, 2018 (Farnell PDF mirror); v1.9, November 28, 2017
(PDF mirror). The current 1.30 revision is the reference revision for
this audit.

## DS925, section by section

### AC Switching Characteristics (DS925 landing page section)

States that all values are based on the speed specifications in the
AMD Vivado Design Suite, with a speed-spec version table: XCZU3EG
(including XCZU3EG) is under speed specification version 1.30. This
is the closest DS925 comes to admitting where fabric timing lives:
in Vivado speed files, not in the data sheet. No delay values.

### DC Characteristics

Absolute maximum ratings and recommended operating conditions:
supply voltages only. No timing values.

### Configuration Switching Characteristics

TPL, TPOR, TPS_PROG_B, FICAPCK, FDNACK, FCFGMCLK and related
configuration timing (milliseconds, and nanoseconds for the program
pulse). Configuration timing only, not fabric delays.

### Clock Buffers and Networks

Table 1, Clock Buffers Switching Characteristics. FMAX only, no
delays. BUFG, BUFGCE_DIV, BUFGCE, BUFCE_LEAF: 775 MHz at -2 /
0.85V, 667 MHz at -1 / 0.85V, 725 MHz at -2 / 0.72V. A frequency
is a period bound, not a pin-to-pin delay, so nothing here can fill
a DB field. Same shape as DS922/DS923/DS930.

### Block RAM and FIFO Switching Characteristics

RETRIEVED 2026-11-25 by the follow-up lane, read directly from the
DS925 document in two versions that agree value-for-value:

- Current revision: DS925 v1.30, release 2026-07-09, docs.amd.com
  page "Block RAM and FIFO Switching Characteristics"
  (https://docs.amd.com/r/en-US/ds925-zynq-ultrascale-plus/Block-RAM-and-FIFO-Switching-Characteristics).
  The web rendering numbers the table Table 1 (per-page numbering).
- Older revision: DS925 v1.23, February 28, 2023, DigiKey PDF mirror
  (https://mm.digikey.com/Volume0/opasdata/d220001/medias/docus/7155/XCZU3EG-1SFVC784I.pdf),
  Table 80: Block RAM and FIFO Switching Characteristics, page 65.

Column headers in both versions: speed grades -3 at 0.90V, -2 at
0.85V, -1 at 0.85V, -2 at 0.72V, -1 at 0.72V, in that order.

- TRCKO_DO, Clock CLK to DOUT output (without output register), Max:
  0.91, 1.02, 1.11, 1.46, 1.53 ns. The -2 at 0.85V column (second
  data column) is 1.02 ns = 1020 ps. This is the value stored in
  `documented_xczu3eg()` as `bram.clk_to_do`: TRCKO_DO without
  output register is the unregistered clock-to-DOUT path, which is
  what the DB field means.
- TRCKO_DO_REG, Clock CLK to DOUT output (with output register),
  Max: 0.27, 0.29, 0.30, 0.42, 0.44 ns. Not stored: the DB has no
  registered-output field.
- The v1.30 revision history lists changes to Tables 3, 4, 7, 8, 9
  and PS sections only; the BRAM table values are identical in v1.23
  and v1.30. The code citation "DS925 v1.30 Table 80" uses the PDF
  table number from v1.23, carried to v1.30 because the table was
  not renumbered or changed between the two revisions; the v1.30
  web page shows the same table as Table 1 under its per-page
  numbering.

The sibling datasheets show the same table shape: FMAX_WF_NC,
FMAX_RF, FMAX_FIFO, FMAX_ECC rows (frequencies), TPW (minimum pulse
width, ps), TRCKO_DO (ns, Max), TRCKO_DO_REG (ns, Max). Only
TRCKO_DO maps onto a DB field (`bram.clk_to_do`).

Sibling values observed but NOT used as a source: DS922, DS923,
DS926, and DS930 all show TRCKO_DO 1.02 ns at -2 / 0.85V and
1.46 ns at -2 / 0.72V (DS926: 1.02 ns for -2E/-2I/-2LE at 0.85V;
DS930: 1.02 ns for -2 at 0.85V). That convergence was a hypothesis
to check when the DS925 table was read, not a citation. The
follow-up lane has now read the DS925 table itself and confirms
the same values in DS925, so the stored value is cited to DS925,
not to any sibling. The GAP-F5 lane already refused to transplant
from a sibling datasheet, and this lane does the same: a DS925
value must be read from DS925.

### UltraRAM Switching Characteristics

Not surfaced for DS925 itself; the DS922/DS923/DS926/DS930 tables
are uniform: FMAX, FMAX_ECC_NOPIPELINE, FMAX_NOPIPELINE (MHz),
TPW (ps), TRSTPW (clock cycles). No clock-to-out delay exists in
the UltraRAM table, so `uram.clk_to_do` has no documented source
in any UltraScale+ datasheet. Frequencies are not delays.

### DSP48 Slice Switching Characteristics

FMAX only: FMAX, FMAX_PATDET, FMAX_MULT_NOMREG,
FMAX_MULT_NOMREG_PATDET, FMAX_PREADD_NOADREG,
FMAX_NOPIPELINEREG, FMAX_NOPIPELINEREG_PATDET (MHz). DS922 at -2
/ 0.85V: FMAX 775 MHz. No A/B-to-P or clock-to-P delay is
published, so `dsp.mult_a_b_to_p` and `dsp.clk_to_p` have no
documented source. Same for DS925, whose DSP table has the same
FMAX-only shape (confirmed via the Kria DS987/DS985 tables, which
defer to DS925 and list FMAX rows only).

### IOB High Performance (HP) and High Density (HD) Switching Characteristics

Per-standard, per-slew buffer delays: TINBUF_DELAY_PAD_I (pad
through input buffer to the I pin), TOUTBUF_DELAY_O_PAD (O pin
through output buffer to the pad), TOUTBUF_DELAY_TD_PAD (T pin
through output buffer to the pad, 3-state disabled). Example from
DS922 at -2 / 0.85V: DIFF_HSTL_I_12_F gives TINBUF 0.394 ns and
TOUTBUF 0.423 ns. These are buffer delays that depend on the I/O
standard and slew rate; the DB fields `iob.pad_to_fabric` and
`iob.fabric_to_pad` are generic with no standard or slew qualifier,
so no single value from these tables can honestly fill them. The
flow's designs do not fix one I/O standard. Nothing stored.

### Input/Output Delay Switching Characteristics

IDELAY/ODELAY control parameters only: FREFCLK ranges (300 to 800
MHz component mode), TMINPER_CLK 3.195 ns, TMINPER_RST 52.00 ns,
IDELAY/ODELAY chain resolution 2.1 to 12 ps. A delay-chain tap
resolution is not a fabric path delay. Nothing stored.

### Device Pin-to-Pin Output Parameter Guidelines

Composite pin-to-pin values, not decomposable into DB fields.
TICKOF (global clock input to output flip-flop without MMCM, near
clock region, SSTL15, output flip-flop, fast slew rate): XCZU3 is
5.28 ns at -2 / 0.85V (columns -3, -2, -1 at 0.85V then -2, -1 at
0.72V; XCZU3 row N/A, 4.90, 5.28, 6.08, 6.51 ns). TICKOF_FAR and
TICKOFMMCMCC are the same kind of composite. Each bundles the
clock input buffer, the clock network, the flip-flop clock-to-out,
and the output buffer, and the tables state that the values are
representative (clock root at device center) and direct the reader
to the Vivado timing report for actual pin-to-pin values. No
LUT, FF, or INT component can be extracted from them.

### What DS925 does not contain at all

There is no CLB, LUT, flip-flop, CARRY8, or interconnect delay
table in DS925, and none in DS922, DS923, DS926, or DS930 either.
Fabric cell delays and routing delays are published nowhere in the
public AMD documentation set; they exist only in the Vivado speed
files. This is the structural reason the fabric-delay gap cannot
be closed from documents.

## UG572 / UG574 note

The lane brief named UG574 (UltraScale Architecture Clocking
Resources). That title belongs to UG572; UG574 is the UltraScale
Architecture Configuration user guide. Neither document carries
fabric delay values: UG572 describes clocking architecture
(BUFG, BUFCE_LEAF, MMCM, PLL, attributes, equations), and UG949
describes the clock path topology and programmable row tap delays
only qualitatively (tap values decrease with distance from the
clock root; no picosecond values). No delay number from either
guide can fill a DB field.

## Field-by-field map (20 DB fields)

- cells.lut6: no public source. No LUT delay table exists.
- cells.ff_ckq: no public source. No FF clock-to-out table exists.
- cells.ff_setup: no public source.
- cells.ff_hold: no public source.
- carry8.ci_to_co_bit, carry8.s_to_o, carry8.cyinit_to_co: no
  public source. No CARRY8 delay table exists.
- dsp.mult_a_b_to_p, dsp.clk_to_p: DS925 DSP table is FMAX only.
- bram.clk_to_do: DOCUMENTED 2026-11-25 by the follow-up lane:
  DS925 v1.30 Table 80 TRCKO_DO, 1.02 ns (1020 ps) at -2 / 0.85V,
  Max, without output register. Stored in `documented_xczu3eg()`.
  This is the only documented xczu3eg value in the DB.
- bram.addr_setup: no public source. The BRAM table has no address
  setup row (only FMAX, TPW, TRCKO_DO, TRCKO_DO_REG).
- uram.clk_to_do, uram.addr_setup: UltraRAM table is FMAX and
  pulse width only; no clock-to-out delay is published.
- interconnect.single, interconnect.double, interconnect.quad,
  interconnect.long, interconnect.pip: no public source. No INT
  hop or PIP delay table exists.
- iob.pad_to_fabric, iob.fabric_to_pad: IOB tables are
  per-standard, per-slew buffer delays; no generic value applies.

## Nit N64 (found 2026-11-24, confirmed 2026-11-25, NOT fixed here)

The existing `documented_xcku3p()` and `documented_xcvu3p()` pin
BRAM CLK to DOUT at 1460 ps, cited as "DS922 Table 33 TRCKO_DO,
-2 0.85V" and "DS923 Table 33 TRCKO_DO, -2 0.85V". The current
public DS922 and DS923 Block RAM tables (docs.amd.com, crawled 5
days before this lane) show TRCKO_DO 0.91, 1.02, 1.11, 1.46, 1.53
ns across the columns -3 at 0.90V, -2 at 0.85V, -1 at 0.85V, -2
at 0.72V, -1 at 0.72V. So 1.46 ns is the -2 grade at 0.72V, and
the -2 at 0.85V value is 1.02 ns (1020 ps). The 1460 ps value is
real, but its cited conditions ("-2 0.85V") do not match the
column it comes from.

Confirmation by the follow-up lane (2026-11-25, own re-read): the
DS925 v1.23 PDF Table 80 and the DS925 v1.30 docs.amd.com page
show the identical column layout and the identical TRCKO_DO row
(0.91, 1.02, 1.11, 1.46, 1.53 ns), with the headers 0.90V / -3,
0.85V / -2, -1, 0.72V / -2, -1. In DS925 the 1.46 ns value is
unambiguously under the -2 at 0.72V column, and the -2 at 0.85V
value is 1.02 ns. The DS922 web table read in the same search
shows the same layout and values, so the column mapping in N64
holds for DS922/DS923 as well: 1460 ps is the -2 / 0.72V value,
not -2 / 0.85V. This lane did not change the value or the pinned
tests: correcting a pinned documented value for the other two
parts needs a parent ruling. The new `documented_xczu3eg()` value
applies the correct column: 1020 ps from the -2 at 0.85V column.

## What this lane changed

First lane (2026-11-24, outcome BLOCKED):

- This audit file, `crates/helion-ultrascale/docs/xczu3eg_timing_sources.md`.
- A doc-comment pointer to this file in
  `crates/helion-ultrascale/src/timing_db.rs`. No delay value
  changed; `preliminary_xczu3eg()` is byte-identical in behavior.
- Two tests in `crates/helion-ultrascale/tests/timing_db.rs`:
  the audit file must exist and name DS925, TRCKO_DO, and the
  Vivado `report_timing` unblocker (fail-before shown by stashing
  this file), and `preliminary_xczu3eg()` must carry no
  Documented delay value (regression guard).

Follow-up lane (2026-11-25, outcome A-partial):

- This audit file: BRAM table RETRIEVED with version, table
  number, values, and URLs; verdict now A-partial with exactly
  one documented value; N64 confirmed by an own re-read of DS925
  Table 80; field-by-field map updated for `bram.clk_to_do`.
- `crates/helion-ultrascale/src/timing_db.rs`: new
  `documented_xczu3eg()` with exactly one value, BRAM CLK to
  DOUT 1020 ps from DS925 v1.30 Table 80 TRCKO_DO, -2 / 0.85V.
  `documented_db()` now takes the BRAM picosecond value as a
  parameter; the xcku3p/xcvu3p calls still pass 1460 and their
  citations and pinned tests are unchanged. `preliminary_xczu3eg()`
  is byte-identical in behavior; only its doc comment now points
  at the documented value. The module doc comment records the
  A-partial outcome.
- `crates/helion-ultrascale/src/lib.rs`: `documented_xczu3eg`
  added to the re-export, parallel to the other two documented
  constructors.
- One test in `crates/helion-ultrascale/tests/timing_db.rs`:
  `documented_xczu3eg_has_only_bram_clk_to_do` pins 1020 ps, its
  exact citation, all other fields UNKNOWN, and no ballpark value.
  Fail-before: on the stashed base code the test fails to compile
  (E0432, unresolved import `documented_xczu3eg`); after restore
  it passes. The two earlier new tests still pass unchanged.
