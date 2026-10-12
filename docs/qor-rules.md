# QoR Suggestions — Rule Catalog

The QoR suggestions engine (`crates/helion-gui/src/qor.rs`) is Helion's
equivalent of Vivado's `report_qor_suggestions`: a rule-based analysis over the
post-route design and STA results that produces actionable, measured
suggestions — never generic advice.

## Architecture

- **Rules are data.** `qor_rules()` returns the registry: `{ id, name,
  category, run: fn(&QorInputs) -> Vec<QorSuggestion> }`. Adding a rule means
  appending one entry; the IDE panel, the Tcl commands
  (`report_qor_suggestions`, `refresh_qor_suggestions`, `select_qor`,
  `select_qor_object`), and the report catalog pick it up untouched.
- **Pure engine.** `qor.rs` has no egui dependency. The IDE adapts its model
  into `QorInputs` (design, timing, paths, router result, constraints,
  effective clocks, utilization, period) and paints the resulting `QorReport`.
- **Measured numbers only.** Every suggestion cites real analysis outputs
  (logic levels, fanout, slack, router overuse). `gain_ps` (estimated WNS
  improvement) is attached only where computable from those numbers; otherwise
  it is `None`, never invented.
- **Deterministic.** Output sorts Critical → Warning → Info, then registry
  order. Re-running on an unchanged design returns the identical set (the IDE
  caches per paint like `methodology_report`).
- **Locate.** Each suggestion's `objects` are real design object names; the
  panel's Locate button cross-probes them through the master selection API
  (`apply_object_crossprobe`).

## Rules

| ID | Severity | Category | Fires when | Fix hint |
|----|----------|----------|------------|----------|
| QOR-LOGIC-1 | Critical (slack<0) / Warning (slack<10% period) | Logic Levels | A timing path has ≥6 LUT levels and is failing or near-critical | Pipeline the cone; gain = removed levels × path's own avg LUT delay |
| QOR-FANOUT-1 | Warning (≥128) / Info (≥32) | High Fanout | A net drives ≥32 loads | Replicate the driver per sink cluster |
| QOR-ROUTE-1 | Critical | Congestion | PathFinder left overused routing nodes | Spread logic (pblock), reduce packing density, routability strategy |
| QOR-CLOCK-1 | Warning | Constraints | Sequential design analyzed on the built-in clock (no user SDC/XDC) | Add `create_clock` with the real period |
| QOR-MACRO-1 | Warning | Macro Cascade | BRAM/DSP is ≥30% of a near-critical path's delay | Register the macro I/O (BRAM out-reg / DSP pipeline regs) |
| QOR-HOLD-1 | Critical | Hold | STA hold slack < 0 | Check clock skew; let the router's hold-fix add delay |
| QOR-IOB-1 | Info | IOB Packing | An output is driven by combinational fabric (router-measured `from_ff == false`) | Pack the driving register into the IOB |
| QOR-NET-1 | Warning | Route Delay | The worst setup path is >55% route delay | Floorplan endpoints together; ease congestion. Gain = half the net delay |
| QOR-TNS-1 | Warning | Failing Endpoints | TNS < 0 across ≥8 endpoints | Sweep implementation strategies (design runs) |
| QOR-WNS-1 | Critical | Strategy | WNS < 0 with <8 failing endpoints (narrow failure; silent when TNS-1 fires) | Open Design Runs and launch a timing-driven strategy sweep (one-click action) |
| QOR-UTIL-1 | Info | Utilization | LUTFF utilization ≥80% | Recover area first (`opt_design`, BRAM mapping) or sweep area-oriented strategies in Design Runs (one-click action) |

## Threshold rationale

Thresholds are tuned so the `examples/counter.sv` golden test stays quiet
(no Critical on a tiny passing design) while real problems fire:

- 6 logic levels: the HL10T LUT6 is fast; fewer than 6 levels rarely dominate
  a 10 ns period, and Vivado's guidance similarly targets deep cones.
- Fanout 32/128: below 32, replication costs more than it saves on this fabric.
- 55% net share: above half, the path is placement-limited, not logic-limited.
- 80% utilization: past this the placer measurably loses freedom on HL10T.

## Testing

- `qor.rs` unit tests: each rule fires on a crafted fixture (synthetic
  `Design` / `TimingPath`) and stays silent when the condition is absent.
- `ide.rs` golden test `qor_suggestions_counter_golden`: runs the full
  implement flow on `examples/counter.sv`, asserts every cited object exists
  in the design, every explanation cites a measured number, no Critical fires,
  and select/locate round-trips work.
