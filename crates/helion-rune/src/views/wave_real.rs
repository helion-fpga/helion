//! Waveform tools on real simulation traces: the scene fixtures and the
//! end-to-end checks that the protocol decoders, analog display and
//! virtual buses read what the engines actually produced.
//!
//! Two real sources feed them:
//!
//! - `examples/wave_tools/bus_periph_tb.sv` through the testbench runner
//!   (`helion_verif::tb::run_testbench`, the Sim pane's worker): an SPI
//!   exchange, a UART "OK", an AXI-Lite write plus read-back, an FSM state
//!   register, a counter and a triangle ramp, every one a DUT port in the
//!   run's VCD.
//! - `examples/counter.sv` through the design run (`run_engine_vcd`:
//!   synthesis plus the `helion-sim` kernel), whose netlist dump bit-blasts
//!   the counter into `cnt_0..cnt_3`: the case virtual buses exist for.
//!
//! Nothing here fabricates samples; the protocol fixture in
//! [`crate::views::wave_demo`] stays a unit-test input only.

use std::path::PathBuf;

use crate::state::AppState;
use crate::views::wave_decode::DecoderKind;
use crate::views::wave_style::{Analog, TraceDisplay};

/// The example peripheral (`examples/wave_tools`): DUT plus testbench.
pub const PERIPH_FILES: [&str; 2] = ["bus_periph.sv", "bus_periph_tb.sv"];
/// Testbench name of the example peripheral.
pub const PERIPH_TB: &str = "bus_periph_tb";

/// Copy `examples/wave_tools` into a fresh temp directory and point the
/// Sim pane's discovery at it (runs write `.helion/` beside the copy,
/// never into the repo). Returns the directory and the testbench index.
pub fn stage_periph(s: &mut AppState) -> Result<(PathBuf, usize), String> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let ex = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/wave_tools");
    let dir = std::env::temp_dir().join(format!(
        "helion-rune-wave-real-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for f in PERIPH_FILES {
        std::fs::copy(ex.join(f), dir.join(f)).map_err(|e| format!("{f}: {e}"))?;
    }
    s.sim.dir_override = Some(dir.clone());
    s.sim.vcd = true;
    crate::views::tb::rescan(s);
    let i = s
        .sim
        .tbs
        .iter()
        .position(|t| t.name() == PERIPH_TB)
        .ok_or("bus_periph_tb not discovered")?;
    Ok((dir, i))
}

/// Run `bus_periph_tb` for real (synchronously, for scenes) and mount its
/// VCD in the Wave pane exactly as a finished run does.
pub fn seed_periph_run(s: &mut AppState) -> Result<PathBuf, String> {
    let (dir, i) = stage_periph(s)?;
    crate::views::tb::run_testbench_now(s, i)?;
    if !s.wave.has_live() {
        return Err("the run mounted no VCD".into());
    }
    Ok(dir)
}

/// Index of the trace named exactly `name` in the mounted import.
pub fn trace_index(s: &AppState, name: &str) -> Option<usize> {
    s.wave.live.as_ref()?.traces.iter().position(|t| t.name == name)
}

/// The `wave-real-tools` scene: the peripheral run with all four decoders
/// added from the panel's path (roles auto-assigned, FSM names read from
/// the DUT the run simulated), the ramp drawn linear and the counter as a
/// staircase.
pub fn seed_real_tools(s: &mut AppState) {
    if seed_periph_run(s).is_err() {
        return;
    }
    s.center = crate::state::CenterPane::Wave;
    // The run's toasts would cover the sheet.
    s.toasts = crate::state::ToastState::default();
    for kind in [DecoderKind::Spi, DecoderKind::Uart, DecoderKind::Fsm, DecoderKind::AxiLite] {
        crate::views::wave_decode::add_decoder(s, kind);
    }
    let lanes: Vec<usize> = ["state", "cnt", "ramp", "cs_n", "sclk", "mosi", "miso", "uart_tx"]
        .iter()
        .filter_map(|l| trace_index(s, &format!("testbench.dut.{l}")))
        .collect();
    s.wave.live_lanes = lanes;
    s.wave.lanes_custom = true;
    for (leaf, mode) in [("ramp", Analog::Linear), ("cnt", Analog::Step)] {
        s.wave.style.display.insert(
            format!("testbench.dut.{leaf}"),
            TraceDisplay {
                fixed: None,
                analog: Some(mode),
            },
        );
    }
    s.wave.live_selected = trace_index(s, "testbench.dut.ramp");
    // Fit the run into the scene's trace viewport (~400pt).
    s.wave.zoom_fit_to(Some(400.0));
    s.wave.cursor = 1_100;
    s.wave.clamp_to_grid();
}

/// Run `examples/counter.sv` through the design-run engine (synthesis plus
/// the `helion-sim` kernel) and mount the netlist dump.
pub fn seed_counter_run(s: &mut AppState, cycles: u32) -> Result<(), String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv");
    let src = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let (mut live, _) = crate::views::wave::run_engine_vcd(&src, "counter.sv", cycles, 10_000)?;
    live.source = format!("counter ({cycles} cycles, helion-sim)");
    s.vcd.timescale_ps = live.timescale_ps.max(1);
    s.vcd.duration_ps = live.duration_ps;
    s.vcd.signals = live.traces.iter().map(|t| t.name.clone()).collect();
    s.wave.mount_live(live);
    Ok(())
}

/// The `wave-real-bus` scene: the counter's bit-blasted netlist bits packed
/// into a virtual bus, shown as a staircase under its member bits.
pub fn seed_real_bus(s: &mut AppState) {
    if seed_counter_run(s, 40).is_err() {
        return;
    }
    s.center = crate::state::CenterPane::Wave;
    let bits: Vec<usize> = (0..4)
        .filter_map(|b| trace_index(s, &format!("counter.cnt_{b}")))
        .collect();
    let mut lanes: Vec<usize> = ["counter.clk", "counter.led"]
        .iter()
        .filter_map(|n| trace_index(s, n))
        .collect();
    lanes.extend(&bits);
    s.wave.live_lanes = lanes;
    s.wave.lanes_custom = true;
    s.wave.live_selected = bits.first().copied();
    s.wave.style.multi = bits.iter().skip(1).copied().collect();
    if let Ok(name) = s.wave.create_virtual_bus() {
        s.wave.style.display.insert(
            name,
            TraceDisplay {
                fixed: None,
                analog: Some(Analog::Step),
            },
        );
    }
    s.wave.zoom_fit_to(Some(400.0));
    s.wave.cursor = 2_000;
    s.wave.clamp_to_grid();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init;
    use crate::views::wave_style::NO_TRACE_WHY;
    use rune::{Env, Runtime};

    fn labels(s: &AppState, kind: DecoderKind) -> Vec<(String, Vec<String>)> {
        let d = s
            .wave
            .decode
            .decoders
            .iter()
            .find(|d| d.kind == kind)
            .expect("decoder added")
            .clone();
        s.wave
            .run_decoder(&d)
            .expect("decodes")
            .into_iter()
            .map(|l| (l.name, l.spans.into_iter().map(|s| s.label).collect()))
            .collect()
    }

    fn lane(name: &str, labels: &[&str]) -> (String, Vec<String>) {
        (name.to_string(), labels.iter().map(|l| l.to_string()).collect())
    }

    /// The Sim pane's worker runs the testbench; every decoder then reads
    /// the protocol the DUT really drove.
    #[test]
    fn testbench_worker_trace_decodes_every_protocol() {
        let mut s = init();
        let (dir, i) = stage_periph(&mut s).unwrap();
        crate::views::tb::launch_testbench(&mut s, i).unwrap();
        let start = std::time::Instant::now();
        while s.sim.is_active() {
            crate::views::tb::poll_sim(&mut s);
            assert!(start.elapsed() < std::time::Duration::from_secs(120));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let out = s.sim.tbs[i].last.clone().unwrap().unwrap();
        assert!(out.passed, "{}", out.summary);
        assert!(s.wave.has_live(), "the run mounts its VCD");
        assert!(s.wave.live.as_ref().unwrap().source.contains(PERIPH_TB));
        // The DUT is not open in the editor: FSM names come from the run.
        assert!(!s.editor.lines.iter().any(|l| l.contains("SPI_XFER")));
        for kind in DecoderKind::ALL {
            crate::views::wave_decode::add_decoder(&mut s, kind);
        }
        assert_eq!(
            labels(&s, DecoderKind::Spi),
            vec![lane("MOSI", &["0xA5", "0x3C"]), lane("MISO", &["0x5A", "0xC3"])]
        );
        assert_eq!(
            labels(&s, DecoderKind::Uart),
            vec![lane("UART", &["0x4F 'O'", "0x4B 'K'"])]
        );
        assert_eq!(
            labels(&s, DecoderKind::Fsm),
            vec![lane("State", &["IDLE", "SPI_XFER", "UART_SEND", "AXI_RW", "IDLE"])]
        );
        assert_eq!(
            labels(&s, DecoderKind::AxiLite),
            vec![
                lane("Write", &["WR 0x0010 ← 0x0000BEEF OKAY"]),
                lane("Read", &["RD 0x0010 → 0x0000BEEF"]),
            ]
        );
        // Every auto-assigned role reads a trace that moves in this run
        // (BRESP is tied OKAY).
        let live = s.wave.live.as_ref().unwrap();
        for d in &s.wave.decode.decoders {
            for name in d.inputs.iter().flatten() {
                let t = live.traces.iter().find(|t| &t.name == name).unwrap();
                let moves = t.samples.windows(2).any(|w| w[0] != w[1]);
                assert!(moves || t.leaf() == "s_axi_bresp", "{name} never toggles");
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn analog_draws_the_real_ramp_and_counter() {
        let mut s = init();
        seed_real_tools(&mut s);
        let ramp = trace_index(&s, "testbench.dut.ramp").expect("ramp traced");
        let cnt = trace_index(&s, "testbench.dut.cnt").expect("cnt traced");
        let sclk = trace_index(&s, "testbench.dut.sclk").expect("sclk traced");
        let live = s.wave.live.clone().unwrap();
        let n = live.sample_len();
        let (h, pps) = (48.0, s.wave.pps());
        // Linear ramp: one vertex per change (plus the start and the tail),
        // spanning the lane between its 3pt insets, peaks at 248 on top.
        let tr = &live.traces[ramp];
        let pts = crate::views::wave_style::analog_points(&s, tr, h, n);
        let changes = tr.samples.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(pts.len(), changes + 2);
        let top = pts.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        let bottom = pts.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max);
        assert_eq!((top, bottom), (3.0, h - 3.0));
        let peak = tr.samples.iter().position(|&v| v == 248).unwrap();
        assert!(pts.iter().any(|p| p[0] == peak as f32 * pps && p[1] == 3.0));
        // The ramp really is a triangle: +8 per clock up to 248, then down.
        assert_eq!(tr.samples[peak - 10], 240);
        assert_eq!(tr.samples[peak + 10], 240);
        // Step counter: two vertices per change, every segment flat or
        // vertical.
        let tc = &live.traces[cnt];
        let pts = crate::views::wave_style::analog_points(&s, tc, h, n);
        let changes = tc.samples.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(pts.len(), 2 * changes + 2);
        assert!(pts.windows(2).all(|w| w[0][0] == w[1][0] || w[0][1] == w[1][1]));
        // Analog lanes are taller than digital ones on the sheet.
        assert!(s.wave.live_lane_h(tr) > s.wave.live_lane_h(&live.traces[sclk]));
    }

    #[test]
    fn virtual_bus_packs_the_real_counter_bits() {
        let mut s = init();
        seed_real_bus(&mut s);
        let live = s.wave.live.as_ref().expect("design run mounted");
        let bus = live.traces.last().unwrap();
        assert!(s.wave.is_virtual(&bus.name), "{}", bus.name);
        assert_eq!(bus.width, 4);
        let bits: Vec<&crate::views::wave::LiveTrace> = (0..4)
            .map(|b| {
                live.traces
                    .iter()
                    .find(|t| t.name == format!("counter.cnt_{b}"))
                    .unwrap()
            })
            .collect();
        for i in 0..bus.samples.len() {
            let packed = (0..4).fold(0, |v, b| v | (bits[b].samples[i] << b));
            assert_eq!(bus.samples[i], packed);
        }
        // The packed bus counts: each change is +1 mod 16, and it wraps.
        let steps: Vec<(u64, u64)> = bus
            .samples
            .windows(2)
            .filter(|w| w[0] != w[1])
            .map(|w| (w[0], w[1]))
            .collect();
        assert!(steps.len() >= 30, "{} increments", steps.len());
        assert!(steps.iter().all(|&(a, b)| b == (a + 1) % 16));
        assert!(steps.contains(&(15, 0)));
        assert_eq!(s.wave.display_of(&bus.name).analog, Some(Analog::Step));
    }

    #[test]
    fn fit_shows_a_whole_real_run_with_a_readable_grid() {
        let mut s = init();
        seed_counter_run(&mut s, 40).unwrap();
        let n = s.wave.sample_len();
        assert!(n > 7_000, "a design run fills the grid: {n}");
        s.wave.zoom_fit_to(Some(500.0));
        assert!((s.wave.sheet_w() - 500.0).abs() < 1.0, "{}", s.wave.sheet_w());
        // Grid lines stay >= 8pt apart at that zoom.
        let step = crate::views::wave::grid_step(s.wave.pps());
        assert!(step as f32 * s.wave.pps() >= 8.0);
        assert_eq!(crate::views::wave::grid_step(4.0), 16, "100% keeps the 16-sample grid");
        // Unknown width (before the first layout) keeps 100%.
        s.wave.zoom_fit_to(None);
        assert_eq!(s.wave.zoom, 1.0);
    }

    fn rt(s: AppState) -> Runtime<AppState> {
        let spec = crate::app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        let mut rt = Runtime::new(s, crate::view, spec.size, env);
        rt.frame(0.0);
        rt
    }

    fn enabled(r: &Runtime<AppState>, key: &str) -> Result<(), Option<String>> {
        let e = r
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("missing {key}"));
        if e.focusable { Ok(()) } else { Err(e.help.clone()) }
    }

    #[test]
    fn trace_tools_enable_on_a_real_trace_and_explain_otherwise() {
        let mut demo = init();
        demo.center = crate::state::CenterPane::Wave;
        crate::views::wave_demo::seed_demo_wave(&mut demo);
        let r = rt(demo);
        for key in ["wave-decoders-toggle", "wave-make-bus", "wave-style-linear", "wave-fixed-q"] {
            assert_eq!(
                enabled(&r, key),
                Err(Some(NO_TRACE_WHY.to_string())),
                "{key} on the demo"
            );
        }
        let mut s = init();
        seed_real_tools(&mut s);
        let r = rt(s);
        for key in [
            "wave-decoders-toggle",
            "wave-style-linear",
            "wave-fixed-q",
            "wave-dec-add-spi",
            "wave-dec-add-axi",
        ] {
            assert_eq!(enabled(&r, key), Ok(()), "{key} on the real trace");
        }
        // Make Bus needs two 1-bit picks, and says so until it has them.
        assert_eq!(
            enabled(&r, "wave-make-bus"),
            Err(Some("Select two or more 1-bit traces (Cmd-click names to add)".into()))
        );
        for key in [
            "wave-dec-lane-0-mosi",
            "wave-dec-lane-1-uart",
            "wave-dec-lane-2-state",
            "wave-dec-lane-3-write",
            "wave-dec-lane-3-read",
        ] {
            assert!(r.find(key).is_some(), "{key}");
        }
        let mut s = r.state.clone();
        s.wave.live_selected = trace_index(&s, "testbench.dut.sclk");
        s.wave.style.multi = vec![trace_index(&s, "testbench.dut.mosi").unwrap()];
        let r = rt(s);
        assert_eq!(enabled(&r, "wave-make-bus"), Ok(()));
    }
}
