# Engineering notes

This directory holds Helion's internal engineering notes: work logs, design
proposals, investigation write-ups and campaign reports. They are kept for
history and context. They are not user documentation, they are not published
on the website, and they may describe plans that changed or numbers that have
since moved.

For the user-facing documentation, start at the [README](../README.md) and the
[website](https://helion-fpga.github.io/helion/). The current plan is
[ROADMAP.md](../ROADMAP.md); what shipped is in [CHANGELOG.md](../CHANGELOG.md).

| Path | What it holds |
|---|---|
| [`fm-hel/`](fm-hel/) | `FM-HEL-*` work notes from individual engineering pushes (IMUX reach, OFL/FTDI bring-up, corpus residuals, synthesis wall time, and so on) |
| [`design/`](design/) | Design proposals and research: AIG rewriting, SAT equivalence, DSP/BRAM wiring, timing calibration, perf budgets, the SoftDiag schema, and the QoR rule catalog of the legacy egui IDE |
| [`reports/`](reports/) | Point-in-time reports: the hand-off notes, the 77-PR campaign report, fuzzing logs, the 1,000-design corpus report, the October IDE UI review, the 2.0.4 perf table |
| [`github-page-notes.md`](github-page-notes.md) | Settings for the GitHub repository page that cannot be set from git |
| [`vivado-bench-mac.md`](vivado-bench-mac.md) | How the vendor-tool comparison numbers were collected |

When a note turns into something users rely on, promote it to `docs/` (and
link it from the site) rather than linking here.
