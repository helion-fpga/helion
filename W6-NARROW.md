# FM-HEL-14-W6 NARROW + autofix

**Tip:** `3609cfae37376f880d12b1f75a6c3c47262706f8`
**Parent chain:** 89be271 → … → 3609cfa
**Branch:** `wip/1.4-w6-vhdl`
**Author:** saksham-45
**Lock:** crates/helion-vhdl/ only

## Captain NARROW
Only the process/STA clock net becomes `clk`. Other ports/signals named `clock` stay.
Tests: `vhdl_sta_clock_name_is_narrow`, `vhdl_narrow_clock_keeps_data_clock_port`, `vhdl_rng_clock_is_sta_clk`.

## Autofixes
1. `missing_component` diagnostic uses `ent.name` (entity being lowered) — `vhdl_missing_component_module_is_entity_being_lowered`
2. Entity generics+consts threaded into `parse_arch` const env
3. Instance name still in diagnostic (`inst=`)

## Tests
```
cargo test -p helion-vhdl --offline --lib -- --test-threads=1
test result: ok. 24 passed; 0 failed
```

## Soft-hold
No merge. Push branch only.
