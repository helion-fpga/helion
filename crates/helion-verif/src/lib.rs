//! Helion verification engines with no UI dependency.
//!
//! - [`ila`]: simulation-based integrated logic analyzer: probe/trigger
//!   configuration, single-pass `helion-sim` capture (sync and on a worker
//!   thread with progress/cancel atomics), the zoom/cursor viewer model,
//!   trigger suggestion from a capture, JSON session save/load, and VCD
//!   export.
//! - [`tb`]: the integrated testbench runner: discovery (`*_tb.sv`,
//!   `tb_*.sv`), the stimulus-subset parser, DUT lookup, the interpreter that
//!   drives `helion-sim` (`$display`/`assert`/`$error`/`$fatal`), VCD/log
//!   writing, and the persisted run history.
//! - [`classtb`]: class-based / UVM testbenches on the `helion-svtb` engine
//!   (routed from [`tb::run_testbench`]), UVM test discovery and UVM
//!   message parsing for the IDE.
//! - [`wave`]: waveform display math (fixed-point `Qm.n`, analog Y
//!   mapping, LSB-first virtual-bus packing) and the SPI / UART / AXI-Lite /
//!   FSM protocol decoders behind the viewer's decoded lanes. The display
//!   math and the FSM fallback wording were extracted from
//!   `helion-gui/src/ide.rs` (`WaveformDisplayMode`,
//!   `compute_analog_samples_y`, `WaveTrace::decode_transactions`); the
//!   multi-signal decoders are new here.
//!
//! **Duplication note.** Both modules were extracted verbatim (plus the
//! UI-free net-list helpers in [`ila`]) from `helion-gui/src/ila.rs` and
//! `helion-gui/src/tb.rs`. `helion-gui` is being retired and is deliberately
//! left untouched, so the two copies coexist until it is deleted; fix bugs
//! here, and do not add new callers of the `helion-gui` copies. The Rune IDE
//! (`helion-rune`) depends on this crate, never on `helion-gui`.

pub mod classtb;
pub mod ila;
pub mod tb;
pub mod wave;
