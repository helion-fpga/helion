# FM-HEL-L3-VHDL — SoftDiag mirror

**Branch:** `wip/learner-L3-vhdl`  
**Baseline / schema tip:** `d12c326a509bc48c13af48f55dedeca230c3ccd7` (`origin/wip/learner-L3`)  
**Worktree:** `/workspace/helion-learner-L3-vhdl` (Linux; never Mac)  
**Lock:** `crates/helion-vhdl/` only (did not edit `helion-ir`; schema already present)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>` (env per commit)  
**Hold:** soft-hold / no merge / no force-push  
**PR:** soft-hold #53 on helion-fpga/helion

## Tip

| | SHA |
|---|---|
| Schema (IR) | `d12c326a509bc48c13af48f55dedeca230c3ccd7` |
| VHDL SoftDiag | `de1b940d4c3fc4b3ae951c2637b6372779f23cca` |
| Autofix | _(this commit / `git rev-parse HEAD`)_ |
| Branch tip | `wip/learner-L3-vhdl` HEAD |

```
vhdl(L3): SoftDiag autofix — two-pass child softs + if-generate
```

## Autofix (adversarial review + Firstmate ASK-USER)

1. **Two-pass `child_soft_incomplete`** — Pass 1 emits every unit with an empty child_softs map (declaration-order safe). Pass 2 builds a bottom-up SoftDiag forest and appends `child_soft_incomplete` where a known child has softs. Covers parent-declared-before-child.
2. **Roots-only forest** — `MapResult.softs` holds roots only; nested softs live under `SoftDiag.children`. `soft_table_lines` flattens for display without duplicating leaves into the top-level vec.
3. **`parse_if_generate`** — no silent Inst drops; `generate_not_lowered` when Insts are truncated (mirrors for-generate).
4. **`synth_vhdl` / `synth_vhdl_path`** stay Design-only (softs dropped by design); SoftDiag API is `map_vhdl*`. Documented here + `docs/SOFT-DIAG-SCHEMA.md`.
5. TLS `PARSE_SOFTS` left as-is (not worth a large rewrite).

## What changed

`crates/helion-vhdl/src/lib.rs` only (+ brief report/schema notes).

- **`map_vhdl` / `map_vhdl_path`** return `helion_ir::MapResult { design, softs }` (roots-only forest).
- **`missing_component`**, **`child_soft_incomplete`**, **`clock_clk_clash`**, **`sim_only`**, **`vendor_wrapper` / `record_type`**, **`generate_not_lowered`** (for- and if-generate).
- **SOFT ≠ PASS.** Missing body stays `cells=0` + `NO_BODY=1`. No fake FD, no invented LUTs for `unused_*` / `fcov_`.

## SoftDiag proof

Hierarchy (roots-only; leaf nested under wrap):

```
soft name=child_soft_incomplete module=wrap detail=inner span=vhdl.vhd:… children=1
soft name=missing_component module=inner detail=inst=u_miss child=missing_child span=vhdl.vhd:…
```

## Tests

```
cargo test -p helion-vhdl --offline --lib -- --test-threads=1
test result: ok. 35 passed; 0 failed
```

| Test | Result |
|------|--------|
| `vhdl_missing_component_softdiag_fields` | ok |
| `vhdl_missing_component_not_silent_pass` | ok |
| `vhdl_missing_component_module_is_entity_being_lowered` | ok (roots-only + nested leaf) |
| `vhdl_parent_before_child_child_soft_incomplete` | ok (order-gap autofix) |
| `vhdl_if_generate_inst_not_silent_softdiag` | ok |
| `vhdl_generate_inst_not_lowered_softdiag` | ok |
| `vhdl_map_closed_cone_has_no_softs` | ok |
| `vhdl_clock_clk_clash_softdiag` | ok |

## Shared locks (untouched)

- **Gold counter WNS_PS=9640** — untouched.
- **no fake FD library** — sequential VHDL still maps `always_ff` / `Hff`.
- **SOFT ≠ PASS** — soft cones never count as PASS / closed WNS.

## Isolation

Diff for the autofix is `crates/helion-vhdl/src/lib.rs` (+ report/schema notes).  
Did not touch `crates/helion-ir` SoftDiag types or invent a parallel VHDL table.
