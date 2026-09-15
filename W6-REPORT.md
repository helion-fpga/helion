# FM-HEL-14-W6 — VHDL worker report

**Branch:** `wip/1.4-w6-vhdl`  
**Baseline:** `236b314bf0f64a1d75f353c4ec461cb11ed87f78`  
**Worktree:** `/workspace/helion-w6-vhdl` (Linux; never Mac)  
**Lock:** `crates/helion-vhdl/` only  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>` (env per commit)  
**Hold:** soft-hold / no merge / no force-push

## Tip

| | SHA |
|---|---|
| VHDL fix | `33f8d31835dc2d58088b2b2ee4a9051692ce3920` |
| Branch tip | see `git rev-parse HEAD` after this report lands |

Parent of the VHDL fix is baseline `236b314`.

```
33f8d31 vhdl(1.4-w6): emit real entity bodies, drop emit_stub, clock≡clk
```

## What changed

`crates/helion-vhdl/src/lib.rs` only.

- **emit_stub gone.** Component declarations no longer emit empty `module …; endmodule` shells. Those shells used to satisfy hierarchy stitch with 0 cells and silence `unknown_instance`.
- **full_adder / adder4.** Every entity that has an architecture is emitted as a real SV module (child first, last entity is top). `adder4` positional port maps bind named bit-blasted nets (`a[0]` → `.a(a_0)`) so `helion-sv` stitch keeps the bit, not the bus.
- **entity consts.** Package + entity generics + entity declarative constants + architecture constants bind as `localparam` and fold in the integer/index path (`parse_constant` / `parse_generic_clause` / `parse_target_sv` now see the const env).
- **clock ≡ clk.** VHDL `clock` ports, process clocks, and idents lower to STA `clk`.
- **missing component.** Named diagnostic includes **instance name**: `diagnostic missing_component module=… inst=… child=…`. The instance stays in SV so `helion-sv` also prints `unknown_instance … inst=…`.
- **no fake FD.** Sequential VHDL still maps `always_ff` / `Hff`, not UNISIM `FDCE`/`FDRE`.

## Tests

```
cargo test -p helion-vhdl --offline --lib -- --test-threads=1
test result: ok. 21 passed; 0 failed
```

Required filters (all pass):

| Test | Result |
|------|--------|
| `vhdl_entity_constant_localparam_maps` | ok |
| `vhdl_entity_constant_generic_width_maps` | ok |
| `vhdl_entity_constant_in_entity_decl_maps` | ok |
| `vhdl_fulladder_maps_luts` | ok (≥2 LUTs) |
| `vhdl_fulladder_hierarchy_adder4_maps` | ok (≥4 LUTs, real `module full_adder` body) |
| `vhdl_blinky_is_inverter_ff` | ok (LUT init `0x5555…` **and Hff**) |
| `vhdl_blinky_file_path_and_incrementer_e2e` | ok (`examples/blinky.vhd`) |
| `vhdl_rng_clock_is_sta_clk` | ok (`input logic clk`, `posedge clk`, Hff) |
| `vhdl_missing_component_names_instance` | ok (`inst=u_miss`) |
| `vhdl_no_fake_fd_library` | ok |

`examples/blinky.vhd` synth: `synth_design blinky cells=3 luts=1` with `reg_bits=1` (inverter LUT + FF + IOB).

## Shared locks (untouched crates; re-checked)

- **Empty-XDC counter → WNS_PS=9640**
  ```
  ./target/debug/helion report_timing examples/counter.sv
  report_timing counter WNS_PS=9640 TNS_PS=0 endpoints=4 r2r_ps=360 iob_ps=220
  ```
- **missing component = named diagnostic with instance name (not silent cells=0)**
  ```
  diagnostic missing_component module=wrap inst=u_miss child=missing_child …
  diagnostic unknown_instance module=wrap inst=u_miss child=missing_child …
  diagnostic missing_component module=top inst=ghost_u child=ghost_ent …
  ```
  Design: `cells=0`, `NO_BODY=1`. No invent-gates.
- **clock ≡ clk for STA** — `vhdl_rng_clock_is_sta_clk`
- **no fake FD library** — `vhdl_no_fake_fd_library` (no UNISIM/FDCE/FDRE)
- **SOFT ≠ PASS** — missing/empty is `NO_BODY` + named miss; not a closed WNS and not scored PASS in this crate

## Isolation

Diff for the VHDL commit is one file: `crates/helion-vhdl/src/lib.rs`.  
Did not touch `crates/helion-sv/src/lib.rs` or any other crate.
