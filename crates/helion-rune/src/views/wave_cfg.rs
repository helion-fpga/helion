//! Saved waveform configurations (Vivado `.wcfg` equivalent): the Wave
//! pane layout — mounted signals in lane order, default and per-signal
//! radix, fixed-point / analog display modes, density, virtual buses,
//! protocol decoders, markers, cursor and zoom — as JSON next to the
//! testbench (`<tb>.wcfg`). A testbench run that loads its VCD restores the
//! file automatically ([`auto_restore`]).
//!
//! Signals are stored by full trace name, never by index, so a layout
//! survives re-runs whose VCD orders or adds signals differently; names
//! that no longer exist are skipped and counted, never invented.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rune::{El, Tx};
use serde::{Deserialize, Serialize};

use crate::state::{AppState, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::wave::{WaveMarker, WaveRadix};
use crate::views::wave_decode::{Decoder, DecoderKind};
use crate::views::wave_style::{Analog, Density, TraceDisplay, VirtualBus};

/// Format version written into every file.
pub const WCFG_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DisplayCfg {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_frac: Option<u8>,
    #[serde(default)]
    pub fixed_signed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analog: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecoderCfg {
    pub kind: String,
    pub inputs: Vec<Option<String>>,
    pub spi_mode: u8,
    pub spi_bits: u8,
    pub uart_period: Option<usize>,
    #[serde(default)]
    pub fsm_names: Vec<(u64, String)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarkerCfg {
    pub name: String,
    pub sample: usize,
}

/// One saved wave layout.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WaveConfig {
    pub version: u32,
    #[serde(default)]
    pub testbench: String,
    pub radix: String,
    /// Mounted signals, in lane order (full trace names).
    pub signals: Vec<String>,
    #[serde(default)]
    pub signal_radix: BTreeMap<String, String>,
    #[serde(default)]
    pub display: BTreeMap<String, DisplayCfg>,
    #[serde(default)]
    pub density: String,
    #[serde(default)]
    pub virtual_buses: Vec<(String, Vec<String>)>,
    #[serde(default)]
    pub decoders: Vec<DecoderCfg>,
    #[serde(default)]
    pub markers: Vec<MarkerCfg>,
    #[serde(default)]
    pub cursor: usize,
    #[serde(default = "one")]
    pub zoom: f32,
}

fn one() -> f32 {
    1.0
}

fn radix_name(r: WaveRadix) -> &'static str {
    match r {
        WaveRadix::Hex => "hex",
        WaveRadix::Bin => "bin",
        WaveRadix::Dec => "dec",
        WaveRadix::Signed => "signed",
    }
}

fn radix_of(s: &str) -> Option<WaveRadix> {
    Some(match s {
        "hex" => WaveRadix::Hex,
        "bin" => WaveRadix::Bin,
        "dec" => WaveRadix::Dec,
        "signed" => WaveRadix::Signed,
        _ => return None,
    })
}

fn kind_name(k: DecoderKind) -> &'static str {
    match k {
        DecoderKind::AxiLite => "axi_lite",
        DecoderKind::Spi => "spi",
        DecoderKind::Uart => "uart",
        DecoderKind::Fsm => "fsm",
    }
}

fn kind_of(s: &str) -> Option<DecoderKind> {
    DecoderKind::ALL.into_iter().find(|k| kind_name(*k) == s)
}

/// Snapshot the live wave layout.
pub fn capture(s: &AppState, testbench: &str) -> Result<WaveConfig, String> {
    let w = &s.wave;
    let live = w.live.as_ref().ok_or("save_wave_config: no waveform loaded")?;
    let signals = w.lanes().into_iter().filter_map(|i| live.traces.get(i).map(|t| t.name.clone())).collect();
    Ok(WaveConfig {
        version: WCFG_VERSION,
        testbench: testbench.to_string(),
        radix: radix_name(w.radix).into(),
        signals,
        signal_radix: w.trace_radix.iter().map(|(k, v)| (k.clone(), radix_name(*v).to_string())).collect(),
        display: w
            .style
            .display
            .iter()
            .map(|(k, d)| {
                (
                    k.clone(),
                    DisplayCfg {
                        fixed_frac: d.fixed.map(|f| f.frac_bits),
                        fixed_signed: d.fixed.is_some_and(|f| f.signed),
                        analog: d.analog.map(|a| match a {
                            Analog::Step => "step".into(),
                            Analog::Linear => "linear".into(),
                        }),
                    },
                )
            })
            .collect(),
        density: match w.style.density {
            Density::Comfortable => "comfortable".into(),
            Density::Compact => "compact".into(),
        },
        virtual_buses: w.style.buses.iter().map(|b| (b.name.clone(), b.members.clone())).collect(),
        decoders: w
            .decode
            .decoders
            .iter()
            .map(|d| DecoderCfg {
                kind: kind_name(d.kind).into(),
                inputs: d.inputs.clone(),
                spi_mode: d.spi_mode,
                spi_bits: d.spi_bits,
                uart_period: d.uart_period,
                fsm_names: d.fsm_names.clone(),
            })
            .collect(),
        markers: w.markers.iter().map(|m| MarkerCfg { name: m.name.clone(), sample: m.sample }).collect(),
        cursor: w.cursor,
        zoom: w.zoom,
    })
}

/// Apply `cfg` to the loaded waveform. Returns `(restored, missing)`
/// signal counts.
pub fn apply(s: &mut AppState, cfg: &WaveConfig) -> Result<(usize, usize), String> {
    if cfg.version > WCFG_VERSION {
        return Err(format!("wave config version {} is newer than this IDE ({WCFG_VERSION})", cfg.version));
    }
    if s.wave.live.is_none() {
        return Err("open_wave_config: no waveform loaded".into());
    }
    let w = &mut s.wave;
    if let Some(r) = radix_of(&cfg.radix) {
        w.radix = r;
    }
    w.style.density = if cfg.density == "compact" { Density::Compact } else { Density::Comfortable };
    // Virtual buses first: they become traces the lane list may name.
    w.style.buses = cfg
        .virtual_buses
        .iter()
        .map(|(name, members)| VirtualBus { name: name.clone(), members: members.clone() })
        .collect();
    w.rebuild_virtual_buses();
    let live = w.live.as_ref().ok_or("no waveform")?;
    let mut lanes = Vec::new();
    let mut missing = 0;
    for name in &cfg.signals {
        match live.traces.iter().position(|t| &t.name == name) {
            Some(i) if !lanes.contains(&i) => lanes.push(i),
            Some(_) => {}
            None => missing += 1,
        }
    }
    let restored = lanes.len();
    w.live_lanes = lanes;
    w.lanes_custom = true;
    w.trace_radix = cfg.signal_radix.iter().filter_map(|(k, v)| radix_of(v).map(|r| (k.clone(), r))).collect();
    w.style.display = cfg
        .display
        .iter()
        .map(|(k, d)| {
            (
                k.clone(),
                TraceDisplay {
                    fixed: d.fixed_frac.map(|frac_bits| helion_verif::wave::FixedPoint { frac_bits, signed: d.fixed_signed }),
                    analog: d.analog.as_deref().map(|a| if a == "linear" { Analog::Linear } else { Analog::Step }),
                },
            )
        })
        .collect();
    w.decode.decoders = cfg
        .decoders
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            Some(Decoder {
                id: i,
                kind: kind_of(&d.kind)?,
                inputs: d.inputs.clone(),
                spi_mode: d.spi_mode,
                spi_bits: d.spi_bits,
                uart_period: d.uart_period,
                fsm_names: d.fsm_names.clone(),
            })
        })
        .collect();
    w.decode.next_id = w.decode.decoders.len();
    w.markers = cfg.markers.iter().map(|m| WaveMarker { name: m.name.clone(), sample: m.sample }).collect();
    w.zoom = cfg.zoom.clamp(0.1, 64.0);
    w.cursor = cfg.cursor;
    w.clamp_to_grid();
    Ok((restored, missing))
}

/// `<tb_dir>/<tb_stem>.wcfg`.
pub fn wcfg_path(tb: &Path) -> PathBuf {
    tb.with_extension("wcfg")
}

/// The selected testbench (path, name).
fn current_tb(s: &AppState) -> Option<(PathBuf, String)> {
    let t = s.sim.tbs.get(s.sim.selected?)?;
    Some((t.file.path.clone(), t.file.name.clone()))
}

pub fn save(s: &mut AppState, path: Option<PathBuf>) -> Result<String, String> {
    let tb = current_tb(s);
    let path = path.or_else(|| tb.as_ref().map(|(p, _)| wcfg_path(p))).ok_or("save_wave_config: select a testbench or give a file")?;
    let cfg = capture(s, tb.as_ref().map(|(_, n)| n.as_str()).unwrap_or(""))?;
    let text = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("save_wave_config {}: {e}", path.display()))?;
    Ok(format!("save_wave_config {} signals={}", path.display(), cfg.signals.len()))
}

pub fn load(s: &mut AppState, path: Option<PathBuf>) -> Result<String, String> {
    let path = path
        .or_else(|| current_tb(s).map(|(p, _)| wcfg_path(&p)))
        .ok_or("open_wave_config: select a testbench or give a file")?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("open_wave_config {}: {e}", path.display()))?;
    let cfg: WaveConfig = serde_json::from_str(&text).map_err(|e| format!("open_wave_config {}: {e}", path.display()))?;
    let (ok, missing) = apply(s, &cfg)?;
    Ok(format!(
        "open_wave_config {} signals={ok}{}",
        path.display(),
        if missing > 0 { format!(" missing={missing}") } else { String::new() }
    ))
}

/// After a testbench VCD loads: restore `<tb>.wcfg` when it exists.
pub fn auto_restore(s: &mut AppState, tb: &Path) {
    let p = wcfg_path(tb);
    if !p.is_file() {
        return;
    }
    let line = match load(s, Some(p)) {
        Ok(v) => ConsoleLine { text: format!("{v} (restored)"), sev: MsgSev::Info },
        Err(e) => ConsoleLine { text: format!("WARNING: {e}"), sev: MsgSev::Warn },
    };
    s.console.lines.push(line);
}

fn note(s: &mut AppState, r: Result<String, String>) {
    let line = match r {
        Ok(v) => ConsoleLine { text: v, sev: MsgSev::Info },
        Err(e) => ConsoleLine { text: format!("ERROR: {e}"), sev: MsgSev::Error },
    };
    s.console.lines.push(line);
}

/// Wave toolbar buttons: Save Layout / Load Layout (the picker opens when
/// no testbench is selected).
pub fn toolbar_buttons(s: &AppState, t: &VivadoTheme) -> El {
    let loaded = s.wave.live.is_some();
    let tb = current_tb(s);
    let has_file = tb.as_ref().is_some_and(|(p, _)| wcfg_path(p).is_file());
    let btn = |key: &'static str, label: &'static str, reason: Option<&'static str>| {
        let b = rune::el()
            .key(key)
            .role("button")
            .label(label)
            .h(20.0)
            .px(6.0)
            .center_y()
            .radius(3.0)
            .fill(t.panel_bg)
            .border(1.0, t.border);
        match reason {
            None => b.focusable(true).hover(t.selection).child(rune::text(label).font(VivadoTheme::FONT_UI).color(t.text)),
            Some(r) => b.disabled(true).description(r).help(r).child(rune::text(label).font(VivadoTheme::FONT_UI).color(t.text_faint)),
        }
    };
    let save_reason = if !loaded { Some("Load a waveform first") } else { None };
    let load_reason = if !loaded { Some("Load a waveform first") } else { None };
    let save_btn = btn("wave-cfg-save", "Save", save_reason);
    let save_btn = if save_reason.is_none() {
        save_btn.help("Save the wave layout (signals, radix, display, decoders, markers) to <testbench>.wcfg")
    } else {
        save_btn
    };
    let save_btn = if save_reason.is_none() {
        if tb.is_some() {
            save_btn.on_press(|s: &mut AppState| {
                let r = save(s, None);
                note(s, r);
            })
        } else {
            save_btn.on_press_tx(|_: &mut AppState, tx: &mut Tx| {
                tx.save_file(
                    rune::FileDialogOptions { title: "Save Wave Configuration".into(), ..Default::default() }.filter("Wave configuration", &["wcfg"]),
                    |s: &mut AppState, r, _| {
                        if let Ok(Some(p)) = r {
                            let r = save(s, Some(p));
                            note(s, r);
                        }
                    },
                );
            })
        }
    } else {
        save_btn
    };
    let load_btn = btn("wave-cfg-load", "Load", load_reason);
    let load_btn = if load_reason.is_none() { load_btn.help("Restore a saved wave layout") } else { load_btn };
    let load_btn = if load_reason.is_none() {
        if has_file {
            load_btn.on_press(|s: &mut AppState| {
                let r = load(s, None);
                note(s, r);
            })
        } else {
            load_btn.on_press_tx(|_: &mut AppState, tx: &mut Tx| {
                tx.open_file(
                    rune::FileDialogOptions { title: "Open Wave Configuration".into(), ..Default::default() }.filter("Wave configuration", &["wcfg"]),
                    |s: &mut AppState, r, _| {
                        if let Ok(Some(p)) = r {
                            let r = load(s, Some(p));
                            note(s, r);
                        }
                    },
                );
            })
        }
    } else {
        load_btn
    };
    rune::row()
        .gap(3.0)
        .items_center()
        .child(rune::text("Layout").font(VivadoTheme::FONT_UI).color(t.text_dim))
        .child(save_btn)
        .child(load_btn)
}

pub const VERBS: &[&str] = &["save_wave_config", "open_wave_config"];

pub fn help(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "save_wave_config" => ("save_wave_config ?file.wcfg?", "save the wave layout (default <testbench>.wcfg)"),
        "open_wave_config" => ("open_wave_config ?file.wcfg?", "restore a saved wave layout"),
        _ => return None,
    })
}

pub fn tcl_verb(s: &mut AppState, name: &str, args: &[String]) -> Result<String, String> {
    let p = args.first().map(PathBuf::from);
    match name {
        "save_wave_config" => save(s, p),
        "open_wave_config" => load(s, p),
        _ => Err(format!("{name}: not a wave config verb")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VCD: &str = "$timescale 1ns $end\n$scope module top $end\n$var wire 1 ! clk $end\n$var wire 4 \" cnt $end\n$var wire 1 # a $end\n$var wire 1 $ b $end\n$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n0!\nb0000 \"\n0#\n0$\n$end\n#5\n1!\n1#\n#10\n0!\nb0011 \"\n1$\n";

    fn loaded() -> AppState {
        let mut s = AppState::demo();
        let live = crate::views::wave::import_vcd_text(VCD).unwrap();
        s.wave.mount_live(live);
        s
    }

    #[test]
    fn layout_round_trips_by_signal_name() {
        let mut s = loaded();
        let names: Vec<String> = s.wave.live.as_ref().unwrap().traces.iter().map(|t| t.name.clone()).collect();
        let cnt = names.iter().position(|n| n.ends_with("cnt")).unwrap();
        let clk = names.iter().position(|n| n.ends_with("clk")).unwrap();
        s.wave.live_lanes = vec![cnt, clk];
        s.wave.lanes_custom = true;
        s.wave.radix = WaveRadix::Bin;
        s.wave.trace_radix.insert(names[cnt].clone(), WaveRadix::Dec);
        s.wave.style.density = Density::Compact;
        s.wave.style.display.insert(
            names[cnt].clone(),
            TraceDisplay { fixed: Some(helion_verif::wave::FixedPoint { frac_bits: 2, signed: true }), analog: Some(Analog::Linear) },
        );
        s.wave.markers.push(WaveMarker { name: "m1".into(), sample: 3 });
        let dir = std::env::temp_dir().join(format!("helion-rune-wcfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wcfg");
        save(&mut s, Some(path.clone())).unwrap();

        // A fresh import in a different signal order restores the layout.
        let mut t = AppState::demo();
        let mut live = crate::views::wave::import_vcd_text(VCD).unwrap();
        live.traces.reverse();
        t.wave.mount_live(live);
        let msg = load(&mut t, Some(path)).unwrap();
        assert!(msg.contains("signals=2"), "{msg}");
        let tn: Vec<String> = t.wave.lanes().iter().map(|&i| t.wave.live.as_ref().unwrap().traces[i].name.clone()).collect();
        assert_eq!(tn, vec![names[cnt].clone(), names[clk].clone()]);
        assert_eq!(t.wave.radix, WaveRadix::Bin);
        assert_eq!(t.wave.trace_radix.get(&names[cnt]), Some(&WaveRadix::Dec));
        assert_eq!(t.wave.style.density, Density::Compact);
        assert_eq!(t.wave.display_of(&names[cnt]).analog, Some(Analog::Linear));
        assert_eq!(t.wave.markers.len(), 1);
    }

    #[test]
    fn missing_signals_are_counted_not_invented() {
        let mut s = loaded();
        let cfg = WaveConfig {
            version: 1,
            radix: "hex".into(),
            signals: vec!["top.gone".into()],
            zoom: 1.0,
            ..Default::default()
        };
        assert_eq!(apply(&mut s, &cfg).unwrap(), (0, 1));
        let newer = WaveConfig { version: 99, ..cfg };
        assert!(apply(&mut s, &newer).is_err());
        assert!(save(&mut AppState::demo(), None).is_err());
    }
}
