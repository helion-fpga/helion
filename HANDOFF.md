# Helion Correctness Campaign: Comprehensive Handoff Document

**Date:** 2026-10-18  
**Branch:** `wip/ide-integration` (HEAD: `61f510d`)  
**Target:** 400-case corpus gate, 0 regressions from the 5 commits, 237/237 passing unit tests  
**Audience:** Incoming AI Agent / Compiler Engineer — Read this document carefully before modifying code.

---

## 1. Executive Summary & The 5 Gate Commits

The `wip/ide-integration` branch incorporates 5 commits that must be verified against the 400-case gate without regressions:

| Commit | Summary | Intended Purpose | Real-World Impact & Regressions |
|---|---|---|---|
| **`33ecc9f`** | sv: Skip port fixup for submodules to prevent F2 multi-driven | `synth_rtl_inner(rtl, is_top)` skips fixup when `!is_top` | **Incomplete fix**: 32 F2 multi-driven regressions persist because the default IOB fallback at lines 18628-18638 was NOT guarded by `is_top`. |
| **`6a0570a`** | sv: Support unary reduction operators in const elaboration | Added `\|`, `&`, `^`, `!`, `~` to `const_atom` | **Latent bugs**: `&` compares with `u128::MAX` (wrong for narrow operands; e.g. `&1` fails); `~` does not mask to operand width. |
| **`c68df0c`** | sv: Support 3-input adders with carry-in (`a+b+cin`) | Handled `Add(Add(a,b),cin)` in `packed_add_pair` | **Exposed 96 F6 Failures**: Restricted tie-off fixup to 6 force-list ports, exposing real silent driver drops (e.g. `mem_la_wstrb` in `picorv32.v`). |
| **`7b33e24`** | Fix string parameter parsing in `const_atom` | Added `Tok::Str` arm via `str_param_hash()` | **PASS**: Solves `serv_ctrl`/`serv_state` string parameter comparisons cleanly. Zero regressions. |
| **`8acdb12`** | Fix whole-wire copy assigns (`pc = r[0]`) being silently dropped | Expanded whole-wire RHS into per-bit assigns; removed skip | **QoR Hit & 11 Test Failures**: Removed skip caused wire assigns to fall into LUT emitter, creating identity LUT6 buffers (`0xaaaaaaaaaaaaaaaa`). Breaks 11 unit tests. |

---

## 2. Test Suite Status: `cargo test -p helion-sv --lib`

- **Total Tests:** 237
- **Passed:** 224
- **Failed:** 11 (all due to commit `8acdb12`'s identity buffer LUTs)

### Root Cause of the 11 Failures:
Commit `8acdb12` removed `RExpr::Ident(_) | RExpr::Bit(_, _) | RExpr::Range(_, _, _) => continue` in `synth_rtl_inner`. This caused wire assignments like `assign led = cnt[3];` to fall into the boolean logic synthesizer instead of being aliased as wires. The synthesizer emits a physical LUT6 with truth table `f(I0) = I0` (`INIT = 0xaaaaaaaaaaaaaaaa` = `12297829382473034410`).

This extra buffer LUT breaks:
1. `tests::incrementer_matches_gold_init` (expected 4 LUTs, got 5 with `0xaaaaaaaaaaaaaaaa`)
2. `tests::incrementer_plus_one_b1` (expected 4 LUTs, got 5)
3. `tests::generate_for_unrolls_four_inverters` (expected 4 LUTs, got 5)
4. `tests::generate_if_selects_real_branch` (expected 4 LUTs, got 5)
5. `tests::reset_if_else_is_not_a_noop` (expected 4 LUTs, got 5)
6. `tests::module_parameter_propagates_to_width` (expected 1 LUT, got 2)
7. `tests::multi_file_cross_instantiation_is_real_cells` (expected 4 LUTs, got 5)
8. `tests::preprocess_skips_package_and_ifdef_macros` (expected 1 LUT, got 2)
9. `tests::function_lifted_dead_cone_swept` (expected 0 dead LUTs, got 2)
10. `tests::const_cycles_2_unrolls_word_shift_not_a_second_add` (expected direct net alias `res_w1_0` to `u_iob/I`, but got an intermediate buffer LUT)
11. `tests::const_index_word_add_is_hffs_not_mac_or_leftover`

**Fix:** Wire assigns must be resolved by net aliasing in `Design` (or a dedicated peephole pass that removes single-input `0xaaaaaaaaaaaaaaaa` LUTs), NEVER by emitting buffer LUTs.

---

## 3. Deep-Dive Root Causes & Concrete Technical Solutions

### Issue 1: `(* full_case *)` & `(* parallel_case *)` Attribute Dropping (CRITICAL)
**Files:** `crates/helion-sv/src/lib.rs` (lines ~7052 and ~8027), `third_party/picorv32/picorv32.v` (lines 402, 1251, etc.)  
**Impact:** 96 F6 failures across the corpus, including `picorv32`.

#### Why Scratch/Fixes Failed:
1. In `parse_seq_item` (line ~8027), there is **zero attribute handling**. When `(* full_case *)` appears before `case (mem_wordsize)`:
   - The token stream presents `Tok::Sym('(')` followed by `Tok::Sym('*')`.
   - `parse_seq_item` checks `if p.eat_kw("case")`, `if p.eat_kw("if")`, etc. None match because the token is `Sym('(')`.
   - It falls through to `parse_assign_nbas`, which expects an assignment and fails with `Err(...)`.
   - In `parse_seq_block` (line 6671), any `Err` triggers `let _ = skip_item_or_block(p);`, which consumes tokens until `;` or `end`. **The entire `case` block is dropped!**
2. In `parse_attr` (line 7052):
   ```rust
   let _ = p.eat_sym('=');
   let v = match p.bump() { ... }; // UNCONDITIONAL BUMP even if '=' is missing!
   let _ = p.eat_sym('*');
   let _ = p.eat_sym(')');
   ```
   When there is no `=` (e.g. `(* full_case *)`), `p.bump()` unconditionally consumes the `*` symbol! The subsequent `p.eat_sym('*')` sees `)` and fails.
3. In `picorv32.v`, there are 9+ occurrences of comma-separated attributes:
   ```verilog
   (* parallel_case, full_case *)
   ```
   `parse_attr` only handled a single identifier before expecting `*` and `)`. It crashed completely on commas.

#### The Exact Code Fix:
Replace `parse_attr` and add `peek_attr` in `crates/helion-sv/src/lib.rs`:

```rust
fn peek_attr<'a>(p: &P<'a>) -> bool {
    p.peek() == Some(&Tok::Sym('(')) && p.t.get(p.i + 1) == Some(&Tok::Sym('*'))
}

fn parse_attrs<'a>(p: &mut P<'a>) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    while peek_attr(p) {
        p.bump(); // '('
        p.bump(); // '*'
        while p.peek().is_some() {
            if p.peek() == Some(&Tok::Sym('*')) && p.t.get(p.i + 1) == Some(&Tok::Sym(')')) {
                p.bump(); // '*'
                p.bump(); // ')'
                break;
            }
            let key = match p.bump() {
                Some(Tok::Ident(s) | Tok::Kw(s)) => s.to_string(),
                Some(Tok::Sym('*')) if p.peek() == Some(&Tok::Sym(')')) => {
                    p.bump();
                    break;
                }
                _ => continue,
            };
            let val = if p.eat_sym('=') {
                match p.bump() {
                    Some(Tok::Str(s) | Tok::Ident(s) | Tok::Kw(s)) => s.to_string(),
                    Some(Tok::Number(n, _)) => n.to_string(),
                    _ => "1".to_string(),
                }
            } else {
                "1".to_string()
            };
            attrs.push((key, val));
            let _ = p.eat_sym(',');
        }
    }
    attrs
}
```

In `parse_seq_item` (line 8027):
```rust
fn parse_seq_item<'a>(p: &mut P<'a>) -> Result<(SeqItemOut<'a>, Vec<ImmAssertAst<'a>>), String> {
    // Consume attributes preceding any statement (* full_case *), (* parallel_case *), etc.
    let attrs = parse_attrs(p);
    let _is_full_case = attrs.iter().any(|(k, _)| k.eq_ignore_ascii_case("full_case"));
    let _is_parallel_case = attrs.iter().any(|(k, _)| k.eq_ignore_ascii_case("parallel_case"));

    if peek_assert_stmt(p) { ... }
    ...
```

---

### Issue 2: Instance Parameter Override Miscompilation (CRITICAL)
**Files:** `crates/helion-sv/src/lib.rs` (lines ~8047–8127, ~9867)  
**Problem:** `#(.N(2))` overrides are ignored during procedural `if` folding. `#(.N(1))` and `#(.N(2))` both produce 65 cells.

#### Root Cause:
Modules are parsed once into `Rtl` using default parameter values in `p.params`. In `parse_seq_item`, `const_cond` folds procedural `if` branches at parse time using these defaults. When an instance is elaborated later with different parameter overrides, the branches were already pruned.

#### Solution:
- **Interim (Option C):** In `parse_seq_item`, do NOT fold procedural `if` conditions that depend on module `parameter`s (only fold conditions that depend strictly on `localparam`s or integer literals). Emit them as `RExpr::Mux` so instance elaboration can bind them properly.
- **Architectural (Option B):** Specialize the module AST during instance elaboration by performing parameter substitution on `Rtl` before synthesizing cells.

---

### Issue 3: Identity LUTs (`0xaaaaaaaaaaaaaaaa`) from Commit `8acdb12` (QoR)
**Files:** `crates/helion-sv/src/lib.rs` (lines ~17736–17825)  
**Problem:** `assign pc = a` or `assign led = cnt[3]` creates a 6-input LUT with `init = 0xaaaaaaaaaaaaaaaa` (identity buffer) instead of a direct wire. Costs +196 cells on `axi_adapter` and breaks 11 unit tests.

#### Solution:
1. When processing assignments in `synth_rtl_inner`:
   If the RHS is an `Ident`, `Bit`, or `Range`, do NOT emit a LUT. Instead, perform direct net aliasing:
   ```rust
   d.alias_net(lhs_net, rhs_net);
   ```
2. Implement a post-synthesis peephole pass:
   Detect any `CellKind::Lut6 { init: 0xaaaaaaaaaaaaaaaa }` that has 1 input and 1 output. If the output is not a preserved net or external port, rewire the fanout directly to the input net and delete the LUT cell.

---

### Issue 4: F2 Multi-Driven Regressions from Commit `33ecc9f`
**Files:** `crates/helion-sv/src/lib.rs` (lines ~18628–18658)  
**Problem:** 32 F2 multi-driven errors across corpus (`serv_decode`, `serv_aligner`, `serv_ctrl`).

#### Root Cause:
Commit `33ecc9f` added `if !is_top { continue; }` to the port fixup loop at line 18656, but missed the fallback at lines 18628–18638:
```rust
if iob_n == 0 {
    // default: last register bit to first output
    if let (Some((n, _, _)), Some((qnet, _))) = (
        rtl.ports.iter().find(|(_, dir, _)| *dir == PortDir::Out),
        reg_bits.last(),
    ) {
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect(qnet, "u_iob", "I");
        d.connect(*n, "u_iob", "PAD");
    }
}
```
1. In submodules like `serv_decode`, `iob_n` is 0.
2. This code runs on submodules and grabs the first output port (`o_sh_right`).
3. It attaches `u_iob/PAD` to `o_sh_right`.
4. `o_sh_right` is already driven by combinational LUT `u_clut59_s76/O`. Result: **two simultaneous drivers on the same net (F2)**.

#### Solution:
1. Submodules must never emit IOBs:
   ```rust
   if !is_top {
       return Ok(d); // No IOBs and no tie-offs for submodules
   }
   ```
2. For top-level modules, only attach an output fallback if `*n` has 0 drivers in `d.nets`.

---

### Issue 5: `axi_interconnect` `no_body` Failure
**Problem:** Single 988-line module produces 0 cells with diagnostic `no_body`.  
**Root Cause:** The module uses multidimensional generate-for loops:
```verilog
generate
    for (i = 0; i < M; i = i + 1) begin
        for (j = 0; j < S; j = j + 1) begin
            ...
        end
    end
endgenerate
```
`parse_for_unroll_module` fails on 2D array variable indexing like `req[i][j]` or complex parameter bounds, falling through to `note_generate_not_lowered` which sets `NO_BODY`.  
**Solution:** Expand identifier subst in `parse_for_unroll_module` to evaluate multi-index expressions with unrolled loop bounds.

---

### Issue 6: `const_atom` Width and Masking Bugs (Commit `6a0570a`)
**Files:** `crates/helion-sv/src/lib.rs` (lines 5089–5109)  
**Problem:**
- `&` (AND-reduction) compares `n == u128::MAX`. For a 4-bit operand, `&4'b1111` is `1`, but `0xF != u128::MAX`, returning `0`.
- `~` (NOT) evaluates `!n`, inverting all 128 bits without masking to operand width.  
**Solution:** Pass the target bit width into `const_atom` and mask reductions:
```rust
let mask = if width >= 128 { u128::MAX } else { (1u128 << width) - 1 };
// AND-reduce:
return Ok(if (n & mask) == mask { 1 } else { 0 });
// NOT:
return Ok((!n) & mask);
```

---

## 4. Architectural Roadmap: Surpassing AMD/Xilinx Vivado

| Dimension | Vivado Pain Point | Helion Technical Superiority |
|---|---|---|
| **Compile Speed** | 15–45 minutes for medium designs (Java GUI + C++ monolithic passes). | **1–10 seconds** full compile via native Rust, `bumpalo` arena allocations, and Rayon multithreading. |
| **Incremental ECO** | Changing 1 LUT or 1 wire requires a 10+ minute full rerun. | **< 200 ms** instant bitstream update by modifying `.hbits` frame buffers and localized rip-up/reroute. |
| **Timing Closure** | Seed roulette: designers run 20 seeds hoping one closes timing by 20 ps. | **Deterministic analytical placement** (electrostatic density model) and exact Pathfinder timing-driven routing. |
| **Formal Safety** | Requires external $50k/seat licenses (Formality/Conformal) to check synthesis. | **Built-in formal verification**: integrated `varisat` SAT solver verifies combinatorial cones during synthesis. |
| **Resource Usage** | 100+ GB disk installation, 15+ GB idle RAM. | **< 50 MB** single binary, < 500 MB RAM, runs in standard CI pipelines in seconds. |

---

## 5. Step-by-Step Instructions for the Next AI / Engineer

Follow this exact order of execution:

1. **Step 1: Fix Attribute Parsing in `crates/helion-sv/src/lib.rs`**
   - Implement `peek_attr` and `parse_attrs` as specified in Section 3, Issue 1.
   - Insert `let attrs = parse_attrs(p);` at the top of `parse_seq_item` (line ~8027).
   - Test on `third_party/picorv32/picorv32.v`: confirm `mem_la_wstrb` has drivers and cell count is restored to ~2543.
2. **Step 2: Guard Submodule IOBs**
   - In `synth_rtl_inner` (line ~18628), ensure lines 18628–18705 are completely skipped if `!is_top`.
   - Verify `serv_decode.v`: confirm `o_sh_right` no longer has a duplicate `u_iob/PAD` driver.
3. **Step 3: Replace Identity LUTs with Net Aliasing**
   - In `synth_rtl_inner`, alias nets for wire assigns instead of compiling them to LUT6 cells.
   - Run `cargo test -p helion-sv --lib`: **all 11 unit tests must pass (237/237 PASS)**.
4. **Step 4: Fix `const_atom` Widths**
   - Mask `&` and `~` in `const_atom` (lines 5089–5109).
5. **Step 5: Run Gate Sweeps**
   - Execute corpus sweep: verify the 96 F6 failures drop to 0 and F2 regressions are eliminated.
