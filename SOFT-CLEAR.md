# FM-HEL-NEXT-2.0 — soft_incomplete clear (picked track)

After v1.5.0 (Ibex LUTs 108545→8073). Highest-value next: remaining softs on real designs, not lab/IDE (other owners).

**Branch:** `wip/2.0-soft-clear` @ ae841b7 (v1.5.0)  
**Lock:** `crates/helion-sv/src/lib.rs`  
**Gold:** empty-XDC counter **WNS_PS=9640**

## Ibex pin-wrap soft scan (post-1.5)

Mapped density held: `cells=13139 luts=8073` WNS_PS=8100.

### Priority (honest HARD — cells>0 + clock; no fake WNS)
1. `wide_cone` **ibex_id_stage** `csr_pipe_flush`
2. `wide_cone` **ibex_multdiv_fast** `op_remainder_d_0`
3. `wide_cone` **ibex2axi** `r_state_1`
4. `generate_not_lowered` **ibex_if_stage** (and cascade child_soft_incomplete)

### Also seen (later / sim_only)
- generate_not_lowered: icache, prim_*, pmp, lockstep, multdiv_slow, timer, dummy_instr, top, cs_registers
- assign_not_lowered: ibex_top clock_en / unused_* / delay_buffer; ibex_core core_busy_o / rvfi_*
- child_soft_incomplete cascade from above + sim_only controller/simulator_ctrl

## Plan
Clear wide_cone trio first without raising hang caps or reintroducing arbiter/sha256/bin2prio hangs. Soft-hold / no self-merge.
