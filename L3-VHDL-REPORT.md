# FM-HEL-L3-VHDL — SoftDiag mirror

**Branch:** `wip/learner-L3-vhdl`  
**Baseline / schema tip:** `d12c326a509bc48c13af48f55dedeca230c3ccd7` (`origin/wip/learner-L3`)  
**Worktree:** `/workspace/helion-learner-L3-vhdl` (Linux; never Mac)  
**Lock:** `crates/helion-vhdl/` only (did not edit `helion-ir`; schema already present)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>` (env per commit)  
**Hold:** soft-hold / no merge / no force-push

## Tip

| | SHA |
|---|---|
| Schema (IR) | `d12c326a509bc48c13af48f55dedeca230c3ccd7` |
| VHDL SoftDiag | `de1b940d4c3fc4b3ae951c2637b6372779f23cca` |
| Branch tip | HEAD of `wip/learner-L3-vhdl` (print `git rev-parse HEAD`) |

Parent of the VHDL fix is schema `d12c326`.

```
de1b940 vhdl(L3): emit helion_ir::SoftDiag into MapResult.softs
```

## What changed

`crates/helion-vhdl/src/lib.rs` only.

- **`map_vhdl` / `map_vhdl_path`** return `helion_ir::MapResult { design, softs }`. `synth_vhdl` / `synth_vhdl_path` stay `Design`-only wrappers (GUI/CLI unchanged).
- **`missing_component`** is a first-class `SoftDiag`: `name`, `module` (entity being lowered, not design top), `detail=inst=… child=…`, best-effort `SoftSpan` (file/line/column from source).
- **`child_soft_incomplete`** on a parent that instantiates a child with softs (nested `children`).
- **`clock_clk_clash`**, **`sim_only`**, **`vendor_wrapper` / `record_type`**, **`generate_not_lowered`** (generate body dropped an instance) also land in `softs`.
- Existing named `eprintln` diagnostics kept (`diagnostic missing_component module=… inst=…`). IR row also printed via `SoftDiag::table_line()`.
- **SOFT ≠ PASS.** Missing body stays `cells=0` + `NO_BODY=1`. No silent success, no fake FD, no invented LUTs for `unused_*` / `fcov_`.

## SoftDiag proof

```
diagnostic missing_component module=wrap inst=u_miss child=missing_child (component body absent; not a LUT; not a closed WNS)
soft name=missing_component module=wrap detail=inst=u_miss child=missing_child span=vhdl.vhd:10:3
diagnostic unknown_instance module=wrap inst=u_miss child=missing_child (child body absent; not a LUT)
diagnostic no_body module=wrap cells=0 (ports only or unknown vendor instance; no gates invented)
synth_design wrap cells=0 luts=0
```

Hierarchy (entity being lowered, not top_name):

```
diagnostic missing_component module=inner inst=u_miss child=missing_child (component body absent; not a LUT; not a closed WNS)
soft name=missing_component module=inner detail=inst=u_miss child=missing_child span=vhdl.vhd:7:3
soft name=child_soft_incomplete module=wrap detail=inner span=vhdl.vhd:14:3 children=1
```

Schema: `helion_ir::SoftSpan` / `SoftDiag` / `MapResult` (see `docs/SOFT-DIAG-SCHEMA.md`). VHDL emits those types into `MapResult.softs`. Headless row: `SoftDiag::table_line()` / `MapResult::soft_table_lines()`.

## Tests

```
cargo test -p helion-vhdl --offline --lib -- --test-threads=1
test result: ok. 33 passed; 0 failed
```

| Test | Result |
|------|--------|
| `vhdl_missing_component_softdiag_fields` | ok (`name`+`module`+`detail` inst+`span` file/line; no PASS; no FD; no unused_/fcov_ LUTs) |
| `vhdl_missing_component_not_silent_pass` | ok (`has_softs`, `cells=0`, `NO_BODY=1`) |
| `vhdl_missing_component_module_is_entity_being_lowered` | ok (`module=inner`, `child_soft_incomplete` on wrap) |
| `vhdl_missing_component_names_instance` | ok (`inst=u_miss`) |
| `vhdl_map_closed_cone_has_no_softs` | ok (full_adder LUTs, empty `softs`) |
| `vhdl_clock_clk_clash_softdiag` | ok |
| `vhdl_generate_inst_not_lowered_softdiag` | ok |
| `vhdl_no_fake_fd_library` | ok |
| `vhdl_fulladder_maps_luts` | ok |
| `vhdl_fulladder_hierarchy_adder4_maps` | ok |

## Shared locks (untouched)

- **Gold counter WNS_PS=9640** — this crate does not touch STA / gold / other crates.
- **no fake FD library** — sequential VHDL still maps `always_ff` / `Hff`.
- **SOFT ≠ PASS** — missing/empty is `NO_BODY` + named miss + `MapResult.softs`; not a closed WNS.

## Isolation

Diff for the VHDL commit is one file: `crates/helion-vhdl/src/lib.rs`.  
Did not touch `crates/helion-ir/src/lib.rs` or any other crate.
