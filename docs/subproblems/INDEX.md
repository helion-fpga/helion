# Sub-problem index: Helion, Vivado-identical bitstreams without Vivado

Each file in this directory is one sub-problem, written so a single agent
instance can pick it up and solve it without re-analyzing the project.
Every file states its goal, what done looks like, what is already done,
what was tried and what failed, what to do, the exact files and commands,
its dependencies, and the rules.

Start from docs/HANDOFF.md (the fresh combined handoff) before any single
sub-problem. It holds the goal, the current state, the branch map, the file
map, and the read-this-first list.

## ZedBoard track (main focus: real hardware)

| ID | Sub-problem | Status | Depends on |
|----|-------------|--------|------------|
| ZB-00 | Mac access (Tailscale SSH + camera) | Open, blocked, current blocker | Nothing |
| ZB-01 | Flash v3 tie-off bitstream, LD0 blinks | Code done, hardware unproven | ZB-00 |
| ZB-02 | BUFGCTRL I0 config (clock passes) | Code done, hardware unproven | ZB-01 |
| ZB-03 | FF clock-enable and set/reset | Code done, hardware unproven | ZB-01 |
| ZB-04 | CARRY4 inference in helion-sv | Not started | Nothing (hardware proof needs ZB-01, ZB-08) |
| ZB-05 | BRAM port config + loud INIT failure | Not started | Nothing |
| ZB-06 | DSP fail loud or implement | Not started | Nothing |
| ZB-07 | Site allocators respect the used set | Not started | Nothing |
| ZB-08 | CARRY4 vertical adjacency | Not started | ZB-04 |
| ZB-09 | LD6/LD7 RIOB33_SING gap | Known gap, workaround in place | ZB-10 |
| ZB-10 | Vivado-vs-Helion bitstream diff (blinky) | Never run | Vivado bitstream located |
| ZB-11 | IOB output driver (t01, no clock) | Test written, never flashed | ZB-00, ZB-01 |
| ZB-12 | Remaining investigator findings (H/M/L) | Not started | Nothing |

## UltraScale+ track (parallel: encoding layer)

| ID | Sub-problem | Status | Depends on |
|----|-------------|--------|------------|
| US-01 | IO_CONFIG tilegrid + HPIO_RIGHT vs HPIO_L | Blocked (Q6 probe) | Vivado license / cluster |
| US-02 | BRAM golden design + INIT translation | Not started | Nothing |
| US-03 | DSP48E2, URAM288, CLK encodings | Blocked (Vivado fuzzing) | License |
| US-04 | LUTRAM table | Blocked (no public table) | Public data or license |
| US-05 | CARRY8 as its own harness block | Ruled, not implemented | Nothing |
| US-06 | xcvu3p frame-total gap (73,035 vs 71,820) | Open, provisional | Vivado confirmation if arithmetic fails |
| US-07 | Random-design generator + 100-design loop | Not started | US-01, US-02, US-03, US-04, US-05, US-12 |
| US-08 | Tier 2 Vivado diff (100 designs) | Never run | US-07 + verified license checkout |
| US-09 | End-to-end flow (Verilog in, bitstream out) | Staging only | US-01, INT PIP audit part 2 |
| US-10 | Synthesis quality (QoR vs Vivado) | Open (23 red, 13 green vs Yosys) | CTO decisions |
| US-11 | Bitstream details (preamble, CRC, padding) | Open | US-08 for items 1-3, US-07 for item 4 |
| US-12 | Harness speed | Open | Nothing |

## Suggested order

1. ZB-00 first. Nothing hardware can proceed without it.
2. ZB-01 next. One flash answers whether the clock fix was the whole story.
3. ZB-11, ZB-02, ZB-03, then the t04/t05 and t03 tests. The isolated tests
   in order.
4. In parallel (no hardware needed): ZB-05, ZB-06, ZB-07, ZB-12,
   US-05, US-06, US-12, US-02.
5. US-01 and US-03 wait on the license. US-07 waits on coverage. US-08
   waits on US-07 and the license. US-10 waits on CTO decisions.
6. ZB-04 and ZB-08 go together (inference plus adjacency). ZB-09 and
   ZB-10 go together (the Vivado diff supplies the SING evidence).

## Rules for every instance

- Read docs/HANDOFF.md first, then your sub-problem file, then the files
  it names. Do not re-analyze the project.
- No em dashes anywhere.
- Never fabricate terminal output, visuals, or numbers. Every claim needs
  its command plus output shown.
- One fix per commit. A fix that regresses a green block is reverted on
  the spot.
- A sub-problem is done only when its "Done when" section is met and the
  proof is shown. Unit tests are not hardware proof.
- Never merge to master without the user's explicit instruction.
- Work on the branch your file names. Commit there, push there, and
  update this index's status line when the status changes.
