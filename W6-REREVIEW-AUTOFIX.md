# FM-HEL-14-W6 re-review auto-fix

**Tip:** `40fb542dc20a258520d34fc153cc46efb104e1eb`
**Parent:** `54ec373b1f4e727116aea0a84162ca9bedb76db5`
**Branch:** `wip/1.4-w6-vhdl`
**Author:** saksham-45
**Lock:** `crates/helion-vhdl/` only
**Hold:** soft-hold / no merge / never Mac

## Auto-fix 1 — `entity_clocks` from selected arch only

`entity_clocks` no longer unions process clocks from every architecture of an entity. The map is built from `arch_for(&e.name)` (last matching architecture, the body emit uses). Top’s last-arch fallback overwrites its entry so clocks match the emitted body. `child_clocks` reads this same map.

A non-selected sequential architecture must not force `clock→clk` on a selected architecture that treats `clock` as data.

Clock/clk rename-on-collision product policy is unchanged (ask-user pending).

## Auto-fix 2 — package consts fold, not dumped into children

`parse_arch` still clones `pkg_consts` into the fold env (widths/indexes/idents). `bind_consts` / `emit_sv` no longer dump every package entry as `localparam` into each hierarchical child.

Emit localparams:

- children: entity generics + entity consts + arch consts
- top: those, plus package params whose names do not collide with ports/signals of that unit

## Tests

```
cargo test -p helion-vhdl --offline --lib -- --test-threads=1
test result: ok. 26 passed; 0 failed
```

NARROW still green: `vhdl_sta_clock_name_is_narrow`, `vhdl_narrow_clock_keeps_data_clock_port`, `vhdl_rng_clock_is_sta_clk`.

Tiny coverage for the two auto-fixes: `vhdl_selected_arch_clocks_not_unioned`, `vhdl_package_const_not_dumped_into_child`.
