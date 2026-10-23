# Reproducers: unlowered SV constructs (2026-10-23)

Minimal self-contained snippets for the root causes in
`../unlowered_constructs_2026-10-23.md`. Each reproduces one distinct cause.

Run:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
export HELION_XILINX_DB=/Users/saksham/Documents/zedboard-helion-debug/work/prjxray-db
./target/release/helion synth <file>.sv --part HL10T-C32-1
```

| File | Root cause | Fatal? | Status |
|---|---|---|---|
| `a_arraystruct_min.sv` | A — word copy between *different* unpacked arrays | yes | fixed |
| `a2_struct_min.sv` | A — same, in the Ibex array-of-struct + generate shape | yes | fixed |
| `b_reset_mux_word.sv` | B — synchronous-clear mux around a word write | yes | fixed |
| `c2_gen_real.sv` | D — bare `assign` as a for-generate item | no | diagnosed |
| `b_generate_assign.sv` | D — `if`/`assign` in a for-generate | no | diagnosed |
| `e_wide_cone.sv` | E — `wide_cone` cap (deliberate hang guard) | no | by design |
| `h2_varindex_small.sv` | F — variable-index read | no | diagnosed |

Root cause G (the CLI syntax gate rejecting `??` in wildcard-case literals and
`@` in comments) is reproduced by the checked-in examples directly:

```sh
./target/release/helion project examples/pico.prj
./target/release/helion project examples/serv.prj
```

It lives in `crates/helion-cli/src/main.rs` (`check_stray_tokens`), which is out
of scope for this task.
