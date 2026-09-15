# FM-HEL-14-W1b — Ibex mapping density

**Branch:** `wip/1.5-w1b-density`  
**Gold:** `examples/counter.sv` **WNS_PS=9640** (held)  
**Ibex WNS:** **WNS_PS=8100** (held; not faked)

## Before / after (`examples/ibex_pin_wrap.prj`)

| | cells | luts | place affinity lutffs | WNS_PS |
|---|---:|---:|---:|---:|
| **Before** (v1.4.0 / `8a49ddc`) | 113611 | **108545** | 8192 (cap) | 8100 |
| **After** | 13139 | **8073** | **8073** (under SKU 8192) | 8100 |

`AFTER_LUTS=8073` is 13.4× fewer than `BEFORE_LUTS=108545`, and fits the named SKU affinity (~8192).

Gold: `$IDE --headless examples/counter.sv` → `WNS_PS=9640`.  
Ibex wall (debug `helion-ide --headless`): ~9.7s.

## What changed (honest FlowMap / cone / sharing)

Serial: `crates/helion-sv/src/lib.rs` only.

- **≤6-PI collapse** for Ident-arm mux/case (one LUT6/bit), not identity + mux + buffer LUTs.
- **LUT6 sharing** of identical `(INIT, pins)` — opcode/select decode reused across a bus.
- **XNOR packing:** 3×2-input XNORs AND-reduced in one LUT6 (eq trees).
- **Wide-cone packing:** invert-into-INIT + fanout-1 AND absorb (≤6 inputs), not LUT2+INV trees.
- **No hang-class softing:** arbiter / bin2prio cascade and sha256_k ROM paths unchanged. WIDE_CONE caps unchanged.

## Holds

- `cargo test -p helion-sv --lib` — 128 passed
- Gold counter **WNS_PS=9640**
- Ibex **WNS_PS=8100** (same as baseline)
- Soft-hold; no merge to master
