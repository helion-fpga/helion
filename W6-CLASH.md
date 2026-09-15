# FM-HEL-14-W6 clock-clash (DIAGNOSTIC + SKIP)

**Tip (VHDL fix):** `11e5f1b742881f51b043d7dae9ea2d2840202c9b`  
**Parent:** `a227b3a0f063c0a97c724c196aa8dee1df9875b8`  
**Branch:** `wip/1.4-w6-vhdl`  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>` (env per commit)  
**Lock:** `crates/helion-vhdl/` only  
**Hold:** soft-hold / no merge / never Mac

Captain FM-AUTO-DECIDE: DIAGNOSTIC + SKIP.

## Policy

When renaming process/STA clock `clock` → `clk` would clash because a
port/signal (or generic/const) named `clk` already exists:

- SKIP the rename — keep the original process clock name (`clock`)
- Emit named diagnostic `clock_clk_clash` (construct + why + entity)
- NEVER silent merge
- NEVER hard-error / abort synth for this case alone

Unclashed NARROW still holds: process clock `clock` with no occupied `clk`
becomes STA `clk` (`vhdl_rng_clock_is_sta_clk`).

## Diagnostic

```
diagnostic clock_clk_clash module={entity} construct=process_clock clock=clock occupied=clk (rename clock→clk skipped; clk already a port/signal; nets kept distinct; not a silent merge; not a synth abort)
```

Emitted from `emit_sv` when `clock_clk_clash(sta_clocks, occupied)`.
`sta_clock_name` / `rewrite_sta_clock_ident` take occupied names and skip.

## Tiny: occupied_sv_names

Top package dump occupancy includes entity generics + entity consts + arch
consts (not only ports/signals), so package `localparam` does not collide
with those names.

## Tests

```
cargo test -p helion-vhdl --offline --lib -- --test-threads=1
test result: ok. 28 passed; 0 failed
```

Clash regression `vhdl_clock_clk_clash_skips_rename`:

- design: data port `clk` + process clock `clock`
- distinct nets: `input logic clk` and `input logic clock`
- `posedge clock` (not merged onto data `clk`)
- diagnostic `clock_clk_clash` present (`module=clash`, construct, why)
- no duplicate `clk` ports
- synth Ok (Hff maps; not `NO_BODY`)

Also: `vhdl_sta_clock_name_is_narrow` skip-on-occupied asserts;
`vhdl_occupied_includes_generics_consts` (package dump vs generic/const).

NARROW still green: `vhdl_sta_clock_name_is_narrow`,
`vhdl_narrow_clock_keeps_data_clock_port`, `vhdl_rng_clock_is_sta_clk`.

## Soft-hold

No merge. Push branch only.
