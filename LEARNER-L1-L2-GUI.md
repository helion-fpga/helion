# FM-HEL-L1-GUI + FM-HEL-L2-UI (helion-gui)

Branch: `wip/learner-L1-gui` (base v2.0.1 / HAD tip `30d3f68`).

## Lock
`helion-gui` only for new code. HAD tip merged in (helion-device authored on `wip/learner-L2-had`).

## L1 (stubs until Proj tips)
- `crates/helion-gui/src/learner_l1.rs` — `SessionStage`, `ConstraintProvenance`, `StageStatus`, `StageError`
- Adapters: `session_stage_of` / `constraint_provenance_of` / `can_advance` from Session Options + `user_sdc` + honesty
- Status rail: `provenance=UserXdc|DefaultPeriod|NoClockPath` beside WNS
- Shared `surface_stage_error` — Open + Implement → status + Messages + Failed chip
- Illegal steps: dimmed chips via `step_blocked` (tips cite `code=STAGE_PREREQ`)

## L2 (live HAD)
- `crates/helion-gui/src/learner_l2.rs` — `HighlightSet`, `packing_summary_english`
- Consumes `Site::id`, `Device::site_by_id` / `contains_site` / `resolve_path_sites` / `resolve_cell_site` / `bels_in_site`
- Path select → HighlightSet sites (CLB_/IOB_) + nets; drives existing schematic/device highlights
- Packing: `CLB_XxYy: N LUTFF` from `Placed.lutff_sites` via `Site::id`
- PIP highlight: not this ship
- Soft→RTL: feature `learner_l2_soft_span` default **off**; no fake spans

## Gold
empty-XDC counter (rtl-only temp) `WNS_PS=9640`, provenance `DefaultPeriod`.
