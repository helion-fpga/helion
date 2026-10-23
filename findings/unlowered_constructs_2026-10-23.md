# Unlowered constructs in the SystemVerilog frontend

Date: 2026-10-23
Branch: `fm/zedboard-fix-bufg`
Scope: diagnosis + documentation, with two category-(a) fixes implemented.

Four real designs were reported as failing to compile. This document records,
per root cause: the minimal self-contained reproducer, the exact command, the
current error text, the precise guard in `crates/helion-sv/src/lib.rs`, a
difficulty classification, and what was actually done.

Reproducers live in `/tmp/helion_repro/final/` and are reproduced inline below.
All commands were run as:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
export HELION_XILINX_DB=/Users/saksham/Documents/zedboard-helion-debug/work/prjxray-db
cd /Users/saksham/Documents/zedboard-helion-debug/helion
./target/release/helion synth <file>.sv --part HL10T-C32-1
# or, for a project file:
./target/release/helion project examples/<name>.prj
```

## Summary: does each design compile?

Measured on branch `fm/zedboard-fix-bufg` (head `1181ba9`) **with** the two
fixes in this document:

| Design | Command | Compiles? | Blocking root cause |
|---|---|---|---|
| `examples/pico.prj` | `helion project examples/pico.prj` | **No** (exit 1) | **Lexer false positive** (root cause G): `??` inside a wildcard-case literal `32'b 0000000_?????_...`. Not a lowering issue. |
| `examples/serv.prj` | `helion project examples/serv.prj` | **No** (exit 1) | **Lexer false positive** (root cause G): `@` inside an email address in a header comment. Not a lowering issue. |
| `examples/ibex_pin_wrap.prj` | `helion project examples/ibex_pin_wrap.prj` | **No** (exit 1) | Nested-mux next-word cone (root cause C), plus 2 over-width (`64-bit`) memories. `pending_dside_accesses_q` **is fixed**. |
| `examples/ysyx_ibex.sv` | `helion synth examples/ysyx_ibex.sv --part HL10T-C32-1` | **No** (exit 1) | Same as `ibex_pin_wrap.prj` (it is the same source). |

> **Important correction to the reported blocker labels.** None of the four
> failures is actually caused by an "array-of-structs in a for-loop". A plain
> `logic [1:0]` array fails identically (see root cause A). The struct type is
> an incidental property of the Ibex source, not the trigger. Likewise
> `wide_cone`, `generate_not_lowered` and `assign_not_lowered` all still fire as
> *soft* diagnostics on `picorv32`, but they were already non-fatal at this
> branch head: `pico.prj` and `serv.prj` reached the parse stage fine before the
> two commits below landed, and their current failure is a hard parse error.

> `pico.prj` / `serv.prj` are rejected by a **newly-added CLI syntax gate**,
> `check_stray_tokens`, introduced in commit `1181ba9` ("cli: fail loudly on bad
> input instead of reporting success"). Before that commit the front end
> recovered and both projects compiled with `exit=0`. The gate is not wrong to
> exist, but it runs over a token stream that does not strip comments and does
> not understand wildcard-case literals, so it rejects two legal constructs.
> See root cause G.

---

## Root cause A — word copy between *different* unpacked arrays

**Classification: (a) small localized fix. FIXED.**

### Reproducer

`/tmp/helion_repro/final/a_arraystruct_min.sv`:

```systemverilog
module a_arraystruct_min (
    input  logic clk,
    input  logic [1:0] w,
    output logic o
);
    logic [1:0] q   [2];
    logic [1:0] dly [2];

    always_ff @(posedge clk) q[0]   <= dly[0];   // <-- Bit(other_array, 0)
    always_ff @(posedge clk) dly[0] <= w;

    assign o = q[0][0];
endmodule
```

The Ibex-shaped variant (array-of-struct inside a generate-for) is
`/tmp/helion_repro/final/a2_struct_min.sv`:

```systemverilog
module a2_struct_min (
    input  logic clk,
    input  logic d,
    output logic o
);
    localparam int unsigned N = 2;
    typedef struct packed { logic valid; logic is_read; } pending_access_t;

    pending_access_t q[N];
    pending_access_t dly[N];

    for (genvar i = 0; i < N; i++) begin : g_track
      always_ff @(posedge clk) q[i] <= dly[i];
    end

    always_comb dly[0] = {d, 1'b0};
    always_comb dly[1] = '0;

    assign o = q[0].valid & q[1].is_read;
endmodule
```

### Command and error (before the fix)

```sh
./target/release/helion synth /tmp/helion_repro/final/a_arraystruct_min.sv --part HL10T-C32-1
```

```
diagnostic bram_not_lowered module=a_arraystruct_min mem=q depth=2 (memory refused by the logic-lowering path: depth>MAX_DEPTH, width>MAX_WIDTH, bits>MAX_BITS, or ram_style/force_bram; no read or write logic was emitted)
synth: synth_sv: 1 memory/memories were not lowered, so none of their read or write logic exists in the netlist and the design would be missing that storage entirely. `q`: its access pattern could not be lowered (depth=2, width=2); a readable form needs a clocked write and a matching read of the same word
```

Note the reported `depth=2, width=2` is far *under* every cap
(`MAX_DEPTH=16`, `MAX_WIDTH=32`, `MAX_BITS=128` at `lib.rs:13848-13850`), so
the message's list of size reasons is actively misleading here.

### Precise cause

`crates/helion-sv/src/lib.rs`, `classify_word_src` (was line 13817):

```rust
fn classify_word_src(rtl: &Rtl, mem: &str, rhs: &RExpr<'_>) -> Option<WordShiftSrc> {
    match rhs {
        RExpr::Ident(s) if *s != mem && sig_depth(rtl, s) == 0 => {
            Some(WordShiftSrc::Packed(s.to_string()))
        }
        RExpr::Bit(s, w) if *s == mem && sig_depth(rtl, s) > 0 => Some(WordShiftSrc::Word(*w)),
        _ => None,
    }
}
```

The `RExpr::Bit` arm required `*s == mem`. `dly[0]` parses as
`RExpr::Bit("dly", 0)` — the *indexed array name* is `dly`, not `q` — so the
arm did not match, the function returned `None`, and the caller set `ok = false`:

```rust
// lib.rs:13899 (was)
let Some(src) = classify_word_src(rtl, mem, src_r) else {
    ok = false;
    break;
};
```

With no word planned, `lower_unpacked_clocked_words` never inserted `q` into
`lowered`, so `mem_names` still contained it at `lib.rs:16535`, and the hard
error at `lib.rs:16601-16630` fired.

Instrumentation confirmed the exact branch (`DBG_CLASSIFY mem=q rhs=Bit("dly", 0)`).
The decisive isolation: a plain `logic [1:0]` array reproduces it, so the struct
type is irrelevant.

### What was done

Fixed. `WordShiftSrc::Word(usize)` became `Word(String, usize)` so a word select
can name *any* unpacked array, and `classify_word_src` now accepts a
same-element-width word of another array. Same-width only: a narrower source
would need a real extend/truncate mux and keeps the old refusal.

The `lower_word_pipeline` caller (`lib.rs:15778`) is a same-array shift register
(it requires `from + 1 == w`), so it explicitly rejects a cross-array source
rather than misinterpreting a transfer as a shift.

Test: `crates/helion-sv/tests/unpacked_word_copy.rs`
- `cross_array_word_copy_lowers_to_flops`
- `struct_array_word_copy_in_generate_lowers`
- `same_array_word_shift_still_lowers` (regression guard)
- `narrower_array_source_is_still_refused` (negative control)

All four fail/refuse correctly on the unpatched tree and pass with the fix.

---

## Root cause B — synchronous-clear mux around a word write

**Classification: (a) small localized fix. FIXED.**

### Reproducer

`/tmp/helion_repro/final/b_reset_mux_word.sv`:

```systemverilog
module b_reset_mux_word (
    input  logic       clk,
    input  logic       rst_ni,
    input  logic [1:0] d,
    output logic       o
);
    logic [1:0] q [2];
    always_ff @(posedge clk) begin
      if (!rst_ni) q[0] <= '0;
      else         q[0] <= d;
    end
    assign o = q[0][0];
endmodule
```

This is exactly the shape in `examples/ysyx_ibex.sv:23019-23021`:

```systemverilog
if (!rst_ni) begin
  pending_dside_accesses_q[i] <= '0;
end else begin
  pending_dside_accesses_q[i] <= pending_dside_accesses_d[i];
end
```

### Command and error (before the fix)

```sh
./target/release/helion synth /tmp/helion_repro/final/b_reset_mux_word.sv --part HL10T-C32-1
```

```
diagnostic bram_not_lowered module=b_reset_mux_word mem=q depth=2 (memory refused by the logic-lowering path: ...)
synth: synth_sv: 1 memory/memories were not lowered, ... `q`: its access pattern could not be lowered (depth=2, width=2)
```

### Precise cause

`crates/helion-sv/src/lib.rs`, `peel_word_enable` (was line 13794):

```rust
fn peel_word_enable<'a>(rhs: &'a RExpr<'a>, mem: &str) -> Option<(Option<RExpr<'a>>, &'a RExpr<'a>)> {
    match rhs {
        RExpr::Mux(c, t, f) => {
            let hold = match *f {
                RExpr::Ident(s) if *s == mem => true,
                RExpr::Bit(s, _) if *s == mem => true,
                _ => false,
            };
            if !hold {
                return None;
            }
            Some((Some((**c).clone()), *t))
        }
        other => Some((None, other)),
    }
}
```

Only a *hold* mux is recognized: the FALSE arm must be the array itself
(`q <= en ? next : q`). A reset mux puts the constant in the TRUE arm and the
next value in the FALSE arm — the mirror image — so `hold` was `false`, the
function returned `None`, and the array was refused.

Instrumentation confirmed:
`DBG_PEEL mem=q word=Some(0) rhs=Mux(Not(Ident("rst_ni")), Const { val: 0, width: 1, care: 1 }, Ident("d"))`.

### What was done

Fixed. `peel_word_enable` now first checks for an all-zero constant in the TRUE
arm and returns `Not(cond)` as the enable, so the flop's D cone carries
`rst ? 0 : next`. Only an all-zero constant is claimed; `q <= rst ? 1 : ...`
keeps the old refusal rather than inventing semantics.

The emitted logic was verified functionally, not merely structurally. For
`rst_mux`, the netlist is a LUT6 with `I0=rst, I1=d_0` feeding an Hff, init
`0x4444444444444444`. Decoding all 8 address points:

```
rst=0 d=0 q=0 -> out=0 expect=0 OK
rst=1 d=0 q=0 -> out=0 expect=0 OK
rst=0 d=1 q=0 -> out=1 expect=1 OK
rst=1 d=1 q=0 -> out=0 expect=0 OK
rst=0 d=0 q=1 -> out=0 expect=0 OK
rst=1 d=0 q=1 -> out=0 expect=0 OK
rst=0 d=1 q=1 -> out=1 expect=1 OK
rst=1 d=1 q=1 -> out=0 expect=0 OK
```

That is exactly `rst ? 0 : d`. The reset is modelled as a synchronous clear,
which matches the RTL: this testbench uses `@(posedge clk)` with no second edge
in the sensitivity list.

Test: `crates/helion-sv/tests/unpacked_word_copy.rs::reset_mux_word_write_lowers`.

---

## Root cause C — nested-mux next-word cone (the remaining Ibex blocker)

**Classification: (b) substantial feature. DIAGNOSED, NOT FIXED.**

### Reproducer

Not yet reduced to a fully self-contained snippet; the smallest observed shape
is a two-level mux feeding an array word. From `examples/ysyx_ibex.sv:9361`
(module `ibex_core`), instrumentation printed:

```
DBG_CLASSIFY mem=rvfi_stage_rd_addr rhs=Mux(Ident("rvfi_id_done"), Ident("rvfi_rd_addr_d"), Ident("rvfi_stage_rd_addr"))
DBG_CLASSIFY mem=rvfi_ext_stage_mip rhs=Mux(And(...), Mux(Or(Ident("instr_valid_id"), Not(Ident("captured_valid"))), Ident("cs_registers_i_mip"), Ident("captured_mip")), Ident("rvfi_ext_stage_mip"))
```

The outer mux *is* a valid hold mux whose FALSE arm is the array itself, and
`peel_word_enable` peels it correctly. The failure is the peeled **source**: it
is itself a mux over captured/next values, and `classify_word_src` only knows
three atomic source shapes (`Packed`, same-array `Word`, cross-array `Word`).

### Command and error

```sh
./target/release/helion project examples/ibex_pin_wrap.prj
# exit=1
project: synth_sv: 31 memory/memories were not lowered, ...
  `rvfi_stage_rd_addr`: its access pattern could not be lowered (depth=1, width=5)
  `rvfi_ext_stage_mip`: its access pattern could not be lowered (depth=2, width=18)
  `rvfi_ext_stage_mcycle`: too large for the register-file path (depth=1, width=64; limits are depth<=16, width<=32, depth*width<=128)
  ... (31 total)
```

### Precise cause

`crates/helion-sv/src/lib.rs:13977` `classify_word_src` returns `None` for any
rhs that is a mux/expression rather than an atomic word or packed signal; the
caller at `lib.rs:14062` then aborts that memory. `peel_word_enable`
(`lib.rs:13942`) can only peel one mux layer.

Note two distinct sub-classes inside the 31:
1. **Nested-mux source** (29 of 31) — needs a general next-word cone.
2. **`width=64` memories** (`rvfi_ext_stage_mcycle`, `rvfi_stage_order`) — these
   exceed `MAX_WIDTH=32` at `lib.rs:13849` and *correctly* report a size reason.
   Fixing these means either a wider register-file path or BRAM inference.

### Why not fixed

This is a substantial feature, not a localized guard. Doing it properly means
replacing `WordShiftSrc` with a per-word next-state `Expr` cone built through
`rexpr_to_bit`, with the existing cone-size guards (`cone_pi_exceeds`,
`rexpr_bit_budget`) applied so the 32-word × 32-bit RVFI arrays cannot explode
the bit-blast. That is a design change to the memory lowering path, and it
collides with another agent's active work in the same function. Left for a
follow-up with an explicit budget policy.

### What was done

Diagnosed and documented. The previously reported `pending_dside_accesses_q`
failure is **fixed** (root cause A + B); `ibex_pin_wrap.prj` now gets 31
memories past those guards and fails later, on this class.

---

## Root cause D — `generate_not_lowered` / bare `assign` in a for-generate

**Classification: (a) small localized fix, but NOT fatal. NOT FIXED (out of scope).**

### Reproducer

`/tmp/helion_repro/final/c2_gen_real.sv`:

```systemverilog
module c2_gen_real (
    input  logic [3:0] b,
    output logic [3:0] y
);
    genvar i;
    generate
      for (i = 0; i < 4; i = i + 1) begin : g_bits
        assign y[i] = b[i];
      end
    endgenerate
endmodule
```

### Command and output

```sh
./target/release/helion synth /tmp/helion_repro/final/c2_gen_real.sv --part HL10T-C32-1
```

```
synth_rtl after assigns comb_bits=0
diagnostic no_logic module=c2_gen_real cells=4 (behavioral body present; no LUT/FF mapped under hang guards)
synth c2_gen_real cells=4 luts=0 inits=[] part=HL10T-C32-1
```

### Precise cause

`parse_module_items` handles `assign` at `lib.rs:9472`, but a bare `assign`
inside a for-generate body is not reached by the two unroll paths. The
fallback at `lib.rs:9560` records it:

```rust
match (parse_lhs(&mut p), p.eat_sym('='), parse_rexpr(&mut p)) {
    (Ok(kind), true, Ok(rhs)) => { ... }
    _ => {
        note_generate_not_lowered(&cur_mod());
        skip_to_semi(p);
    }
}
```

The loop bound `i < 4` with step `i = i + 1` *is* supported by
`parse_for_step_signed` (`lib.rs:5863`), and a genvar used in arithmetic works
(`c4_gen_if.sv` in `/tmp/helion_repro/` lowers 2 LUTs). What is dropped is the
`assign` item form specifically: the body lowers to **0 LUTs and 0 FFs** for a
4-bit passthrough, i.e. the outputs are left undriven.

### Why not fixed

Two reasons. First, it is **not fatal** on these designs: it produces a soft
`no_logic` diagnostic, and on `picorv32` the analogous
`generate_not_lowered module=picorv32_pcpi_fast_mul` fires on a module that is
not on the critical path. Second, the observed *symptom* here — silent zero-LUT
lowering rather than a named refusal — is arguably a worse bug than the missing
feature, and belongs with whoever owns the generate-unroll path. Flagged, not
changed, to keep this change surgical.

---

## Root cause E — `wide_cone` on a wide single-assign cone

**Classification: (c) architecturally blocked (by design). DIAGNOSED, NOT FIXED.**

### Reproducer

`/tmp/helion_repro/final/e_wide_cone.sv`:

```systemverilog
module e_wide_cone (
    input  logic [63:0] a,
    input  logic [63:0] b,
    input  logic [63:0] c,
    output logic        y
);
    wire [63:0] p = a & b;
    assign y = ^(p ^ c) ^ (a[0] & b[1]);
endmodule
```

### Command and output

```sh
./target/release/helion synth /tmp/helion_repro/final/e_wide_cone.sv --part HL10T-C32-1
```

```
diagnostic wide_cone module=e_wide_cone signal=y (wide_cone_cap pis>32 ands>96 luts>4096; cone not mapped; not a LUT; not a closed WNS)
synth e_wide_cone cells=1 luts=0 inits=[] part=HL10T-C32-1
```

### Precise cause

`crates/helion-sv/src/lib.rs:12401` `emit_wide_cone` / the cap check, fed by
`map_wide_cones` (`lib.rs:13598`) and `wide_aig_over_cap`. The cap
`pis>32 ands>96 luts>4096` is a **deliberate hang guard**: this cone is
mapped by the AIG path, and lifting the cap would reintroduce the
`map_wide_cone` hang documented at `lib.rs:12367-12370`.

### What was done

Diagnosed, left alone. This is architecturally blocked rather than a missing
lowering: the guard exists to bound work, and the correct fix is an AIG-quality
improvement (better cut enumeration) so fewer cones exceed the cap, not a larger
cap. On `picorv32` it fires once as
`wide_cone module=picorv32 signal=mem_la_addr_17`, a soft diagnostic.

---

## Root cause F — variable-index read (`variable_index_read`)

**Classification: (b) substantial feature. DIAGNOSED, NOT FIXED.**

### Reproducer

`/tmp/helion_repro/final/h2_varindex_small.sv`:

```systemverilog
module h2_varindex_small (
    input  logic       clk,
    input  logic [7:0] wdata,
    input  logic [2:0] waddr,
    input  logic       we,
    input  logic [2:0] raddr,
    output logic [7:0] rdata
);
    reg [7:0] m [0:7];
    always @(posedge clk) if (we) m[waddr] <= wdata;
    assign rdata = m[raddr];
endmodule
```

### Command and output

```sh
./target/release/helion synth /tmp/helion_repro/final/h2_varindex_small.sv --part HL10T-C32-1
```

```
diagnostic variable_index_read module=h2_varindex_small signal=rdata (variable-index read not mapped; not a LUT; write-side Hff still timed)
synth h2_varindex_small cells=144 luts=72 inits=[...]
```

The write side *does* lower (144 cells / 72 LUTs); only the read mux is missing,
so `rdata` is undriven.

### Precise cause

`note_variable_index_read` (note emitted near `lib.rs:1575`), driven from the
read-expansion path; `lower_var_index_words` (`lib.rs:16439`) handles the write
side only. A variable-index read needs a real depth-way mux.

### What was done

Diagnosed, left alone. This is the same missing capability that makes
`picorv32`'s register file incomplete: at `picorv32.v:1305-1390`,
`cpuregs_rs1 = decoded_rs1 ? cpuregs[decoded_rs1] : 0` is a variable-index read
of a 32-deep array, which additionally exceeds `MAX_DEPTH=16`. `picorv32`
reports `assign_not_lowered module=picorv32 signal=cpuregs_rs1` for it. It is a
substantial feature (a depth-32 × width-32 mux), not a localized guard.

---

## Root cause G — CLI syntax gate rejects comments and wildcard literals

**Classification: (a) small localized fix, but OUT OF SCOPE (owned file). DIAGNOSED, NOT FIXED.**

This is the actual reason `pico.prj` and `serv.prj` fail. It is **not** a
lowering problem at all.

### Reproducers

`pico.prj` — the `??` is a legal SystemVerilog wildcard-case literal
(`third_party/picorv32/picorv32.v:2032`):

```systemverilog
32'b 0000000_?????_000??_???_?????_0001011: begin // getq
```

`serv.prj` — the `@` is an email address inside a header comment
(`examples/serv/serv_decode.v:4`):

```systemverilog
 * SPDX-FileCopyrightText: 2018 Olof Kindgren <olof@award-winning.me>
```

### Command and error

```sh
./target/release/helion project examples/pico.prj
# project: third_party/picorv32/picorv32.v: stray `??` (token 13483) is not valid syntax

./target/release/helion project examples/serv.prj
# project: examples/serv/serv_decode.v: stray `@` (token 1362); expected an event control such as `@(posedge clk)`
```

### Precise cause

`crates/helion-cli/src/main.rs:395` `check_stray_tokens`:

```rust
fn check_stray_tokens(toks: &[String], origin: &str) -> Result<(), String> {
    for (idx, t) in toks.iter().enumerate() {
        let next = idx.checked_add(1).and_then(|i| toks.get(i)).map(|s| s.as_str());
        match t.as_str() {
            "@" => {
                if !matches!(next, Some("(") | Some("*")) {
                    return Err(format!(
                        "{origin}: stray `@` (token {idx}); expected an event control such as `@(posedge clk)`"
                    ));
                }
            }
            "?" if next == Some("?") => {
                return Err(format!("{origin}: stray `??` (token {idx}) is not valid syntax"));
            }
            _ => {}
        }
    }
    Ok(())
}
```

Its input comes from `sv_tokens` (`crates/helion-cli/src/main.rs:527`), whose
caller comments claim it is "a small scanner that ignores comments and strings".
It does handle strings, but it **has no comment handling at all** — there is no
`//` or `/* */` skip anywhere in the function:

```rust
fn sv_tokens(source: &str) -> Vec<String> {
    ...
        if c == '"' { /* string skip */ }
        if c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '`' { /* ident/number */ }
        if c == '(' && b.get(i + 1) == Some(&'*') { /* "(*" attribute */ }
        if c == '*' && b.get(i + 1) == Some(&')') { /* "*)" attribute */ }
        // Single-character punctuation token.
        out.push(c.to_string());
    ...
}
```

So comment bodies are tokenized as ordinary punctuation: the email `@` and the
`??` pair both survive into the token list and trip the gate. The gate was added
in `1181ba9`; before it, both projects compiled with `exit=0`.

### Why not fixed

`crates/helion-cli/src/main.rs` is **explicitly out of scope for this task**
(other agents own it), and the fix belongs there rather than in `helion-sv`:
either strip comments in `sv_tokens`, or exempt `?` runs that are part of a
based literal and `@` that is not preceded by `always`/`always_comb`/etc.

Recorded here because it is the *actual* blocker for two of the four reported
designs, and any agent working on the CLI gate needs this.

## Root cause H — comments not stripped before the syntax gate

Covered by root cause G above; listed separately only because the two symptoms
(`pico`, `serv`) look unrelated at first glance and share one cause.

## Non-blocking diagnostics still present on these designs

These fire but do **not** stop a build; recorded so they are not mistaken for
the cause of the four failures:

- `pico.prj` (when it parses): `width_overflow`, `generate_not_lowered`
  (`picorv32_pcpi_fast_mul`), three `assign_not_lowered`
  (`dbg_insn_rs1`, `dbg_insn_rd`, `cpuregs_rs1`), `wide_cone`
  (`mem_la_addr_17`), `child_soft_incomplete`.
- `ibex_pin_wrap.prj`: `sim_only`, `function_not_called`,
  `function_called_not_inlined`, `sequential_not_lowered` (`clk_sys`).

`dbg_insn_rs1` / `dbg_insn_rd` are a multi-level `always @*` mux chain over
`cached_*` / `decoded_*` values (`picorv32.v:809-836`) — the same nested-mux
class as root cause C.

---

## Verification

```
cargo test --release -p helion-sv
```

Result on branch head `1181ba9` **without** this patch: 14 failed, 212 passed.
Result **with** this patch: 14 failed, 211 passed + 1 filtered. Identical
failure sets — the change introduces no regressions.

The 14 pre-existing failures (7 BRAM/readmem, `ibex_pin_wrap_soft_child_diag`,
`ysyx_ibex_lists_modules_and_synths_top`, `flatten_assign_case_is_one_cap_not_a_closed_cone`,
and 4 `equiv::prove::*`) are unrelated to this work and predate it. All 14 were
reproduced on an unpatched tree.

One further pre-existing failure, `tests::deep_priority_chain_50k_no_stack_overflow`,
aborts the whole test binary with a genuine stack overflow (SIGABRT). This also
reproduces **without** this patch, so it is not ours; it was skipped to obtain
the counts above:

```sh
cargo test --release -p helion-sv --lib -- --skip deep_priority_chain_50k
```

The new test binary passes 5/5:

```
cargo test --release -p helion-sv --test unpacked_word_copy
running 5 tests ... test result: ok. 5 passed; 0 failed
```

## Reproducer index

| File | Root cause | Fatal? |
|---|---|---|
| `/tmp/helion_repro/final/a_arraystruct_min.sv` | A — cross-array word copy | yes (fixed) |
| `/tmp/helion_repro/final/a2_struct_min.sv` | A — Ibex-shaped struct array | yes (fixed) |
| `/tmp/helion_repro/final/b_reset_mux_word.sv` | B — sync-clear mux | yes (fixed) |
| `/tmp/helion_repro/final/c2_gen_real.sv` | D — bare `assign` in for-generate | no (silent 0-LUT) |
| `/tmp/helion_repro/final/b_generate_assign.sv` | D — `if` in for-generate | no (silent 0-LUT) |
| `/tmp/helion_repro/final/e_wide_cone.sv` | E — wide cone cap | no (soft) |
| `/tmp/helion_repro/final/h2_varindex_small.sv` | F — variable-index read | no (soft, `rdata` undriven) |

## Files changed

- `crates/helion-sv/src/lib.rs` — `WordShiftSrc::Word` now carries the array
  name; `classify_word_src` accepts a same-width word of another unpacked
  array; `peel_word_enable` peels an all-zero synchronous-clear mux.
- `crates/helion-sv/tests/unpacked_word_copy.rs` — new, 5 tests.
