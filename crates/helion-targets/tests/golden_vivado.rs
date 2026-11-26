//! Golden comparison: Helion's view contract against Vivado's own reports on
//! the same reference designs.
//!
//! The Vivado captures are not part of the repository (they carry tool
//! output that must not be committed). Point `HELION_VIVADO_REF` at a
//! capture folder for one part, laid out as `ref/<design>/{util.rpt,
//! summary.tsv, clock_util.rpt, power.rpt, timing_paths.rpt}`. Without it the
//! test prints why and passes without comparing.
//!
//! For each design Helion can build, the test builds it with the XDC flow,
//! fills the view and prints a side-by-side table: utilization rows, WNS,
//! WHS, clock buffers and loads, control sets, bonded IO and total power,
//! with the difference. Structural quantities (bonded IO, global clock
//! buffers) must match exactly; the rest within the tolerance stated per
//! row, which is the measured gap plus headroom so a regression fails. The
//! table is also written to `GOLDEN_COMPARE.md` two folders above the
//! capture folder (outside the repository).
//!
//! Run: `HELION_VIVADO_REF=<capture>/7series/xc7z020clg484-1 cargo test
//! --release -p helion-targets --test golden_vivado -- --nocapture`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use helion_targets::view::ImplView;

struct RefDesign {
    /// Capture folder under `ref/`.
    name: &'static str,
    /// Sources relative to the repository root.
    srcs: &'static [&'static str],
    /// XDC relative to the repository root; None: written from the capture's
    /// pins (the counter has no XDC in the repository).
    xdc: Option<&'static str>,
    top: &'static str,
}

const DESIGNS: &[RefDesign] = &[
    RefDesign { name: "counter", srcs: &["examples/counter.sv"], xdc: None, top: "counter" },
    RefDesign { name: "blinky", srcs: &["examples/zedboard_blinky.sv"], xdc: Some("examples/zedboard.xdc"), top: "zedboard_blinky" },
    RefDesign { name: "oled", srcs: &["examples/zedboard_oled_demo.sv"], xdc: Some("examples/zedboard_oled_demo.xdc"), top: "zedboard_oled_demo" },
    RefDesign {
        name: "pico",
        srcs: &["examples/zedboard_pico_soc.sv", "third_party/picorv32/picorv32.v"],
        xdc: Some("examples/zedboard_pico_soc.xdc"),
        top: "zedboard_pico_soc",
    },
    RefDesign { name: "bram", srcs: &["examples/zedboard_bram_fill.sv"], xdc: Some("examples/zedboard_bram_fill.xdc"), top: "zedboard_bram_fill" },
    RefDesign {
        name: "dsp_prims",
        srcs: &["examples/zedboard_dsp_fill_prims.sv"],
        xdc: Some("examples/zedboard_dsp_fill.xdc"),
        top: "zedboard_dsp_fill_prims",
    },
];

/// How a row is judged.
#[derive(Clone, Copy)]
enum Tol {
    /// Structural: must be equal.
    Exact,
    /// Relative, as a fraction of the Vivado value, with an absolute floor
    /// for small counts.
    Rel(f64, f64),
    /// Absolute, in the row's unit.
    Abs(f64),
}

impl Tol {
    fn ok(self, viv: f64, hel: f64) -> bool {
        let d = (hel - viv).abs();
        match self {
            Tol::Exact => d < 1e-9,
            Tol::Rel(r, floor) => d <= (r * viv.abs()).max(floor),
            Tol::Abs(a) => d <= a,
        }
    }

    fn label(self) -> String {
        match self {
            Tol::Exact => "exact".into(),
            Tol::Rel(r, floor) => format!("{:.0}% (min {floor})", r * 100.0),
            Tol::Abs(a) => format!("+/-{a}"),
        }
    }
}

/// `| name | used | ...` rows of a Vivado table report, first occurrence of
/// each name.
fn table_rows(text: &str) -> HashMap<String, Vec<String>> {
    let mut m = HashMap::new();
    for line in text.lines() {
        let cells: Vec<String> = line.split('|').map(|c| c.trim().to_string()).collect();
        if cells.len() >= 3 && !cells[1].is_empty() {
            m.entry(cells[1].clone()).or_insert_with(|| cells[2..].to_vec());
        }
    }
    m
}

fn num(s: &str) -> Option<f64> {
    s.trim().trim_start_matches('<').parse().ok()
}

struct Vivado {
    util: HashMap<String, Vec<String>>,
    summary: HashMap<String, String>,
    bufg: f64,
    clock_loads: f64,
    power_total_w: f64,
}

fn read_ref(dir: &Path) -> Result<Vivado, String> {
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).map_err(|e| format!("{}: {e}", dir.join(f).display()));
    let util = table_rows(&read("util.rpt")?);
    let summary = read("summary.tsv")?
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let clk = read("clock_util.rpt")?;
    let bufg = table_rows(&clk).get("BUFGCTRL").and_then(|r| num(&r[0])).ok_or("clock_util.rpt: no BUFGCTRL row")?;
    // Sum "Clock Loads" over the global clock table (rows g0, g1, ...).
    let mut clock_loads = 0.0;
    let mut col = None;
    for line in clk.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.get(1) == Some(&"Global Id") && col.is_none() {
            col = cells.iter().position(|c| *c == "Clock Loads");
        } else if let (Some(c), Some(id)) = (col, cells.get(1)) {
            if id.starts_with('g') && id[1..].chars().all(|d| d.is_ascii_digit()) && !id[1..].is_empty() {
                clock_loads += cells.get(c).and_then(|v| num(v)).unwrap_or(0.0);
            } else if id.starts_with("Source Id") {
                break;
            }
        }
    }
    let power = table_rows(&read("power.rpt")?);
    let power_total_w = power.get("Total On-Chip Power (W)").and_then(|r| num(&r[0])).ok_or("power.rpt: no total")?;
    Ok(Vivado { util, summary, bufg, clock_loads, power_total_w })
}

/// The counter's XDC, from the capture's own pin names: every port the
/// timing reports name with `(IN)` or `(OUT)` and its package pin, and the
/// clock period from `summary.tsv`.
fn counter_xdc(dir: &Path, v: &Vivado, out: &Path) -> Result<PathBuf, String> {
    let mut paths = String::new();
    for f in ["timing_paths.rpt", "timing_summary.rpt", "timing_hold.rpt"] {
        paths.push_str(&std::fs::read_to_string(dir.join(f)).unwrap_or_default());
    }
    let mut pins: Vec<(String, String)> = Vec::new();
    for line in paths.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if let [pin, .., port, dir] = t.as_slice() {
            if (*dir == "(IN)" || *dir == "(OUT)") && !pins.iter().any(|p| p.1 == *port) {
                pins.push((pin.to_string(), port.to_string()));
            }
        }
    }
    let period = v.summary.iter().find(|(k, _)| k.starts_with("clock.") && k.ends_with(".period_ns")).map(|(_, p)| p.clone()).ok_or("no clock period")?;
    let mut x = String::new();
    for (pin, port) in &pins {
        x.push_str(&format!("set_property PACKAGE_PIN {pin} [get_ports {{{port}}}]\nset_property IOSTANDARD LVCMOS33 [get_ports {{{port}}}]\n"));
    }
    x.push_str(&format!("create_clock -period {period} -name clk [get_ports {{clk}}]\n"));
    let f = out.join("counter.xdc");
    std::fs::write(&f, x).map_err(|e| e.to_string())?;
    Ok(f)
}

fn build(d: &RefDesign, xdc: &Path, out: &Path) -> Result<ImplView, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let srcs: Vec<String> = d.srcs.iter().map(|s| root.join(s).display().to_string()).collect();
    let bit = out.join(format!("{}.bit", d.name));
    let mut log = |_: &str| {};
    let b = helion_targets::xdc_flow::run(&srcs.join(","), &xdc.display().to_string(), &bit.display().to_string(), Some(d.top), false, "auto", "xc7z020clg484-1", None, &mut log)?
        .ok_or("the flow stopped before routing")?;
    Ok(helion_targets::view_xilinx7::from_built(&b))
}

struct Row {
    design: &'static str,
    what: String,
    vivado: f64,
    helion: f64,
    tol: Tol,
}

fn rows_for(d: &RefDesign, v: &Vivado, h: &ImplView) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut push = |what: &str, vivado: f64, helion: f64, tol: Tol| rows.push(Row { design: d.name, what: what.into(), vivado, helion, tol });
    let res = |name: &str| h.resources.iter().find(|r| r.name == name).map(|r| r.used).unwrap_or(f64::NAN);
    // Utilization: synthesis differs (Helion's own front end and mapper),
    // so counts get a relative tolerance; IO and global buffers are fixed by
    // the XDC and the clocking, so they are exact.
    for (name, tol) in [
        ("Slice LUTs", Tol::Rel(0.9, 30.0)),
        ("LUT as Logic", Tol::Rel(0.9, 30.0)),
        ("LUT as Memory", Tol::Rel(0.6, 4.0)),
        ("Slice Registers", Tol::Rel(0.6, 4.0)),
        ("F7 Muxes", Tol::Rel(1.0, 4.0)),
        ("F8 Muxes", Tol::Rel(1.0, 4.0)),
        ("Slice", Tol::Rel(1.2, 8.0)),
        ("Block RAM Tile", Tol::Rel(0.1, 1.0)),
        ("DSPs", Tol::Rel(0.1, 1.0)),
        ("Bonded IOB", Tol::Exact),
    ] {
        let viv = v.util.get(name).and_then(|r| num(&r[0])).unwrap_or(f64::NAN);
        push(name, viv, res(name), tol);
        // The part's capacity is structural: the same site count as Vivado's.
        if let Some(avail) = v.util.get(name).and_then(|r| num(&r[2])) {
            let ours = h.resources.iter().find(|r| r.name == name).map(|r| r.available as f64).unwrap_or(f64::NAN);
            push(&format!("{name} available"), avail, ours, Tol::Exact);
        }
    }
    let ns = |k: &str| v.summary.get(k).and_then(|s| num(s)).unwrap_or(f64::NAN);
    let whs_viv = v
        .summary
        .iter()
        .filter(|(k, _)| k.starts_with("clock.") && k.ends_with(".whs_ns"))
        .filter_map(|(_, s)| num(s))
        .fold(f64::INFINITY, f64::min);
    let t = h.timing.as_ref();
    push("WNS (ns)", ns("wns_all_ns"), t.map(|t| t.wns_ps as f64 / 1000.0).unwrap_or(f64::NAN), Tol::Abs(3.0));
    push("WHS (ns)", whs_viv, t.and_then(|t| t.whs_ps).map(|p| p as f64 / 1000.0).unwrap_or(f64::NAN), Tol::Abs(0.5));
    let c = h.clocks.as_ref();
    push("BUFGCTRL", v.bufg, c.map(|c| c.bufgs_used as f64).unwrap_or(f64::NAN), Tol::Exact);
    push("Clock loads", v.clock_loads, c.map(|c| c.clocks.iter().map(|n| n.loads).sum::<usize>() as f64).unwrap_or(f64::NAN), Tol::Rel(0.4, 4.0));
    let cs = v.util.get("Unique Control Sets").and_then(|r| num(&r[0])).unwrap_or(f64::NAN);
    push("Control sets", cs, h.control_sets.len() as f64, Tol::Rel(1.5, 4.0));
    push("Total power (W)", v.power_total_w, h.power.as_ref().map(|p| p.total_mw / 1000.0).unwrap_or(f64::NAN), Tol::Abs(0.045));
    rows
}

fn render(rows: &[Row], features: &str) -> String {
    let mut s = String::from("# Helion vs Vivado 2017.3, xc7z020clg484-1 reference designs\n\n");
    s.push_str("Generated by `crates/helion-targets/tests/golden_vivado.rs` (HELION_VIVADO_REF). Helion side: this tree's XDC flow and view contract.\n\n");
    s.push_str("| design | quantity | Vivado | Helion | Helion - Vivado | tolerance | ok |\n|---|---|---:|---:|---:|---|---|\n");
    for r in rows {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {:+.3} | {} | {} |\n",
            r.design,
            r.what,
            fmt(r.vivado),
            fmt(r.helion),
            r.helion - r.vivado,
            r.tol.label(),
            if r.tol.ok(r.vivado, r.helion) { "yes" } else { "NO" }
        ));
    }
    s.push_str("\n## Helion power inputs per design (class, count, mW)\n\n```\n");
    s.push_str(features);
    s.push_str("```\n");
    s
}

fn fmt(x: f64) -> String {
    if x.fract() == 0.0 { format!("{x}") } else { format!("{x:.3}") }
}

#[test]
fn helion_matches_vivado_reference_designs() {
    let Some(refdir) = std::env::var_os("HELION_VIVADO_REF").map(PathBuf::from) else {
        eprintln!("golden_vivado: HELION_VIVADO_REF is not set (a Vivado capture folder for xc7z020clg484-1); nothing compared");
        return;
    };
    helion_sv::v2::set_hard(true);
    let out = std::env::temp_dir().join(format!("helion-golden-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let mut rows = Vec::new();
    let mut features = String::new();
    let mut failed_builds = Vec::new();
    // HELION_GOLDEN_ONLY=counter,oled limits the run (the table file is then partial).
    let only = std::env::var("HELION_GOLDEN_ONLY").ok();
    for d in DESIGNS.iter().filter(|d| only.as_deref().is_none_or(|o| o.split(',').any(|n| n == d.name))) {
        let dir = refdir.join("ref").join(d.name);
        let v = match read_ref(&dir) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("golden_vivado: {}: no capture ({e}); skipped", d.name);
                continue;
            }
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let xdc = match d.xdc {
            Some(x) => root.join(x),
            None => counter_xdc(&dir, &v, &out).unwrap(),
        };
        let t0 = std::time::Instant::now();
        match build(d, &xdc, &out) {
            Ok(h) => {
                eprintln!("golden_vivado: {} built in {:.1} s", d.name, t0.elapsed().as_secs_f64());
                if let Some(p) = &h.power {
                    for b in &p.blocks {
                        features.push_str(&format!("{}\t{}\t{}\t{:.6}\n", d.name, b.block, b.used, b.total_mw));
                    }
                    if let Some(c) = h.clocks.as_ref().and_then(|c| c.clocks.iter().max_by_key(|n| n.loads)) {
                        features.push_str(&format!("{}\tfreq\t{:.3}\n", d.name, 1.0e6 / c.period_ps.max(1) as f64));
                    }
                }
                rows.extend(rows_for(d, &v, &h));
            }
            Err(e) => failed_builds.push(format!("{}: {e}", d.name)),
        }
    }
    let _ = std::fs::remove_dir_all(&out);
    let table = render(&rows, &features);
    println!("{table}");
    if let Some(top) = refdir.parent().and_then(Path::parent) {
        let f = top.join("GOLDEN_COMPARE.md");
        std::fs::write(&f, &table).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
    }
    assert!(failed_builds.is_empty(), "Helion failed to build: {failed_builds:?}");
    let bad: Vec<String> = rows
        .iter()
        .filter(|r| !r.tol.ok(r.vivado, r.helion))
        .map(|r| format!("{} {}: Vivado {} Helion {} (tolerance {})", r.design, r.what, fmt(r.vivado), fmt(r.helion), r.tol.label()))
        .collect();
    assert!(bad.is_empty(), "outside tolerance:\n{}", bad.join("\n"));
}
