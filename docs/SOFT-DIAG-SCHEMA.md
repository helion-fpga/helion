# SoftDiag schema (FM-HEL-L3)

Canonical types live in `crates/helion-ir/src/lib.rs`:

- `SoftSpan` — `file` / `line` / `column` / `end_line` (`Option`s)
- `SoftDiag` — `name` + `module` + `detail: Option<String>` + `span` + `children: Vec<SoftDiag>`
- `MapResult` — `design: Design` + `softs: Vec<SoftDiag>`

`name` uses existing diagnostic ids (`assign_not_lowered`, `generate_not_lowered`, `child_soft_incomplete`, …).
Do **not** invent a parallel VHDL table — emit `helion_ir::SoftDiag` into `MapResult.softs`.
PASS never includes soft cones. Never invent LUTs for `unused_*` / `fcov_`.

Headless row: `SoftDiag::table_line()` / `MapResult::soft_table_lines()`.

`MapResult.softs` is a **roots-only forest** (nested softs live under `SoftDiag.children`; `soft_table_lines` flattens for display). SoftDiag API is `map_vhdl*` — `synth_vhdl` / `synth_vhdl_path` stay Design-only and drop softs by design.

