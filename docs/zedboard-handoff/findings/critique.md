# ZedBoard Fix Crew - CRITIQUE (Hardware Verification Critic)

Critic: subagent eaa335a9-a6f0-4334-a931-4d5ebfca33d7
Rule: PASS requires all three checkboxes. User "no" = FAIL, no exceptions.
No em dashes used anywhere in this file.

## Completed Verdicts

### V1: zedboard_blinky.bit (original, pre-BUFG-fix)
- Bitstream: zedboard_blinky.bit, 3159295 bytes, md5 03b7b30ee071ae56640c6f6c9bfd4e6f, built 2026-10-22 20:48 EDT
- Programming: SUCCESS, done=1 (verified in runs 21:03, 21:06, 21:13, 21:16 EDT)
- User observation: LEDs do NOT blink
- Verdict: FAIL (functional). Programs fine, design does not function.

### V2: zedboard_blinky_fixed.bit (BUFG-fix v1, 5 of 10 BUFGCTRL bits)
- Bitstream: zedboard_blinky_fixed.bit, 3159295 bytes, md5 2b242e1e6403a5e9d9ab2002937cee2d, built 2026-10-22 20:54 EDT
- Programming: SUCCESS, done=1 (verified in runs 21:03, 21:06, 21:13, 21:16 EDT)
- User observation: LEDs do NOT blink
- Verdict: FAIL (functional). 5/10 BUFGCTRL bits was insufficient, as the emit audit predicted.

### V3: zedboard_blinky_v2.bit (BUFG 10-bit + CARRY4 mux bits)
- Bitstream: zedboard_blinky_v2.bit, 3159295 bytes, md5 188639e922dae59e2d3a69b81d68ab93, built 2026-10-22 21:14 EDT
- Programming: SUCCESS, done=1 (verified in run 21:16 EDT, test_20261023_011549.log)
- User observation: LEDs do NOT blink
- Verdict: FAIL (functional). Full BUFGCTRL 10-bit config still not enough; root cause elsewhere or polarity wrong.

Pattern note: all three program with done=1. Programming success tells us nothing about functional correctness. This is exactly the silent-failure mode the emit audit warned about.

## Pending Verifications (workers active, nothing submitted yet)

### BUFGCTRL fix (worker: bufg)
Fix target: BUFGCTRL ZINV_ polarity validated against hardware
- [ ] Bitstream programs (done=1)
- [ ] User confirms LEDs blink at expected rate
- [ ] No regressions (previous working features still work)
Awaiting: new bitstream from worker. Note: V3 already had all 10 BUFGCTRL bits and failed, so a polarity change alone must be hardware-validated bit by bit, not shipped as another guess.

### CARRY4 fix (worker: carry4)
Fix target: full CARRY4 configuration (CYINIT/DI/S, not mux bits only)
- [ ] Bitstream programs (done=1)
- [ ] User confirms LEDs blink at expected rate
- [ ] No regressions (previous working features still work)
Awaiting: new bitstream from worker. Note: the old `CARRY4.IN_USE` FASM was invalid (no such feature in X-Ray DB). Verify the emitted FASM parses to real bit addresses before it reaches hardware.

### FF CE/SR fix (worker: ffctrl)
Fix target: ZRST, CEUSEDMUX, SRUSEDMUX emitted when ff has ce/sr nets
- [ ] Bitstream programs (done=1)
- [ ] User confirms LEDs blink at expected rate
- [ ] No regressions (previous working features still work)
Awaiting: new bitstream from worker. Note: the zedboard_blinky design does not use CE/SR, so this fix cannot be the cause of the current LED failure. It prevents future silent failures, not the present one.

## Open Questions for the Orchestrator
1. V3 (full 10-bit BUFGCTRL + CARRY4 mux bits) programs but LEDs still do not blink. The workers should confirm whether the emit path is even emitting the intended FASM (check the FASM output for the actual features/bit addresses) before iterating on hardware again. A bitstream that does not contain the fix bits cannot validate the fix.
2. The 3-minute loop keeps reprogramming known-failed bitstreams. Suggest the loop only program bitstreams newer than the last FAIL verdict, to avoid wearing the board and confusing the user's visual checks.
3. Functional verification depends entirely on the user's LED observation. The critic cannot confirm a PASS without an explicit user report per bitstream.

## Verdict Log (append only)
- 2026-10-22 21:18 EDT: V1 FAIL, V2 FAIL, V3 FAIL recorded. Three checklists opened for BUFGCTRL, CARRY4, FF CE/SR. No new fixes submitted for verification.
