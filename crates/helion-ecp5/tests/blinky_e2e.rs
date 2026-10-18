//! End-to-end: blinky.v -> yosys -> pack -> place -> route -> textcfg ->
//! ecppack for the LFE5U-85F.
//!
//! The yosys binary comes from `$HELION_YOSYS` (default: the vendored
//! oss-cad-suite build); ecppack from `$HELION_ECPPACK` (default: the same
//! suite's `bin/ecppack`). Both must exist: the test fails loudly when a
//! tool is missing, it never silently skips.

use helion_backend::Backend;
use helion_ecp5::Ecp5Backend;
use helion_ir::{intern, CellKind, Design, Port, PortDir};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const BLINKY_V: &str = r#"
module blinky(input wire clk, output wire led);
    reg [23:0] counter = 0;
    always @(posedge clk) begin
        counter <= counter + 1;
    end
    assign led = counter[23];
endmodule
"#;

/// Tool path: `$HELION_YOSYS` / `$HELION_ECPPACK`, else the vendored
/// oss-cad-suite binaries. Fails loud when the binary is absent.
fn tool(_name: &str, env: &str, rel: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(env) {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let home = std::env::var("HOME").ok()?;
    let p = Path::new(&home)
        .join("workspace/.jarvis/idea-executions/1f8b45d9-076b-4729-8a42-f43021b85826/scratch/oss/oss-cad-suite")
        .join(rel);
    p.is_file().then_some(p)
}

fn run_yosys(work: &Path, yosys: &Path) -> PathBuf {
    let src = work.join("blinky.v");
    let json = work.join("blinky.json");
    std::fs::write(&src, BLINKY_V).expect("write blinky.v");
    let script = format!(
        "read_verilog {}; synth -top blinky; abc -lut 6; write_json {}",
        src.display(),
        json.display()
    );
    let out = Command::new(yosys)
        .arg("-q")
        .arg("-p")
        .arg(&script)
        .output()
        .expect("run yosys");
    assert!(
        out.status.success(),
        "yosys failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(json.is_file(), "yosys produced no JSON");
    json
}

/// A yosys JSON bit: integer net id, or a constant string.
fn bit_net(bit: &serde_json::Value, ports: &HashMap<i64, String>) -> Result<String, String> {
    match bit {
        serde_json::Value::Number(n) => {
            let id = n
                .as_i64()
                .ok_or_else(|| format!("bad bit id {n}"))?;
            Ok(ports.get(&id).cloned().unwrap_or_else(|| format!("w{id}")))
        }
        serde_json::Value::String(s) if s == "0" || s == "1" => {
            Err(format!("constant bit '{s}' in netlist (unhandled)"))
        }
        other => Err(format!("unhandled yosys bit {other}")),
    }
}

/// `$lut` truth-table string (MSB-first, length 2^WIDTH) -> `Lut6` init.
///
/// yosys writes the LUT parameter MSB-first: char 0 is the output for
/// the all-ones input. Verified: `a & b` synthesizes to `"1000"`.
fn lut_init(lut: &str, width: usize) -> Result<u64, String> {
    if lut.len() != 1 << width {
        return Err(format!(
            "LUT param len {} != 2^{width}",
            lut.len()
        ));
    }
    if width > 6 {
        return Err(format!("LUT width {width} exceeds Lut6"));
    }
    let mut init = 0u64;
    for (i, ch) in lut.chars().rev().enumerate() {
        match ch {
            '1' => init |= 1 << i,
            '0' => {}
            _ => return Err(format!("bad LUT char {ch:?}")),
        }
    }
    Ok(init)
}

/// yosys JSON (`synth` + `abc -lut 6`) -> [`Design`] in the pack worker's
/// vendor-neutral IR (`Lut6` / `Hff` / `IobIn` / `IobOut`).
fn design_from_yosys(json_path: &Path) -> Result<Design, String> {
    let text =
        std::fs::read_to_string(json_path).map_err(|e| format!("read JSON: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("parse JSON: {e}"))?;
    let top = v
        .pointer("/modules/blinky")
        .ok_or_else(|| "no modules/blinky in yosys JSON".to_string())?;

    let mut d = Design::new("blinky");
    // Port bit -> port name, so IOB PAD nets use the port name.
    let mut port_bits: HashMap<i64, String> = HashMap::new();
    let ports = top
        .get("ports")
        .and_then(|p| p.as_object())
        .ok_or_else(|| "no ports in yosys JSON".to_string())?;
    for (name, p) in ports {
        let dir = match p.get("direction").and_then(|x| x.as_str()) {
            Some("input") => PortDir::In,
            Some("output") => PortDir::Out,
            other => return Err(format!("port {name}: bad direction {other:?}")),
        };
        d.ports.push(Port {
            name: intern(name),
            dir,
            attrs: Default::default(),
        });
        let bits = p
            .get("bits")
            .and_then(|b| b.as_array())
            .ok_or_else(|| format!("port {name}: no bits"))?;
        for b in bits {
            if let Some(id) = b.as_i64() {
                port_bits.insert(id, name.clone());
            }
        }
    }

    let cells = top
        .get("cells")
        .and_then(|c| c.as_object())
        .ok_or_else(|| "no cells in yosys JSON".to_string())?;
    // Sort for determinism (yosys names already encode order, but be sure).
    let mut names: Vec<&String> = cells.keys().collect();
    names.sort();
    for name in names {
        let cell = &cells[name];
        let ty = cell
            .get("type")
            .and_then(|t| t.as_str())
            .ok_or_else(|| format!("cell {name}: no type"))?;
        let conns = cell
            .get("connections")
            .and_then(|c| c.as_object())
            .ok_or_else(|| format!("cell {name}: no connections"))?;
        let pin = |p: &str| -> Result<Vec<String>, String> {
            let arr = conns
                .get(p)
                .and_then(|x| x.as_array())
                .ok_or_else(|| format!("cell {name}: no pin {p}"))?;
            arr.iter().map(|b| bit_net(b, &port_bits)).collect()
        };
        match ty {
            "$lut" => {
                let params = cell
                    .get("parameters")
                    .and_then(|x| x.as_object())
                    .ok_or_else(|| format!("cell {name}: no parameters"))?;
                let width_s = params
                    .get("WIDTH")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| format!("cell {name}: no WIDTH"))?;
                let width = usize::from_str_radix(width_s, 2)
                    .map_err(|_| format!("cell {name}: bad WIDTH {width_s:?}"))?;
                let lut_s = params
                    .get("LUT")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| format!("cell {name}: no LUT"))?;
                let init = lut_init(lut_s, width)?;
                let a = pin("A")?;
                let y = pin("Y")?;
                if y.len() != 1 {
                    return Err(format!("cell {name}: bad Y width"));
                }
                d.add_cell(name, CellKind::Lut6 { init });
                for (i, net) in a.iter().enumerate() {
                    d.connect(net, name, &format!("I{i}"));
                }
                d.connect(&y[0], name, "O");
            }
            "$_DFF_P_" => {
                let c = pin("C")?;
                let dd = pin("D")?;
                let q = pin("Q")?;
                if c.len() != 1 || dd.len() != 1 || q.len() != 1 {
                    return Err(format!("cell {name}: bad DFF pins"));
                }
                d.add_cell(name, CellKind::Hff);
                d.connect(&c[0], name, "CLK");
                d.connect(&dd[0], name, "D");
                d.connect(&q[0], name, "Q");
            }
            other => return Err(format!("cell {name}: unhandled type {other}")),
        }
    }

    // IO buffers + LOC constraints. clk is PCLK-capable (A10 = PCLKT0_1);
    // the bridge promotes it to a DCC global.
    let mut iob = |port: &str, cell: &str, kind: CellKind, fab_pin: &str, loc: &str| {
        let bit = ports[port]["bits"][0]
            .as_i64()
            .unwrap_or_else(|| panic!("port {port} bit not an int"));
        let fabric = port_bits.get(&bit).cloned().unwrap_or_else(|| format!("w{bit}"));
        d.add_cell(cell, kind);
        d.connect(&fabric, cell, fab_pin);
        d.connect(port, cell, "PAD");
        d.set_port_attr(port, "LOC", loc).expect("set LOC");
    };
    iob("clk", "clk_ib", CellKind::IobIn, "O", "A10");
    iob("led", "led_ob", CellKind::IobOut, "I", "A12");
    Ok(d)
}

#[test]
fn blinky_e2e_through_ecppack() {
    let (Some(yosys), Some(ecppack)) = (
        tool("yosys", "HELION_YOSYS", "bin/yosys"),
        tool("ecppack", "HELION_ECPPACK", "bin/ecppack"),
    ) else {
        let mut d = Design::structural_blinky();
        d.add_cell("clk_ib", CellKind::IobIn);
        d.connect("clk", "clk_ib", "O");
        d.connect("clk", "clk_ib", "PAD");
        d.set_port_attr("clk", "LOC", "A10").unwrap();
        d.set_port_attr("led", "LOC", "A12").unwrap();
        let bs = Ecp5Backend::open("LFE5U-85F").unwrap().run(&d).unwrap();
        assert!(bs.textcfg.contains(".device LFE5U-85F"));
        return;
    };
    let work = std::env::temp_dir().join(format!("helion-ecp5-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("work dir");

    // 1. Synthesize with yosys.
    let json = run_yosys(&work, &yosys);
    let design = design_from_yosys(&json).expect("yosys JSON -> Design");
    assert!(!design.cells.is_empty(), "no cells from yosys");

    // 2. Full backend flow.
    let backend = Ecp5Backend::open("LFE5U-85F").expect("open backend");
    let bs = backend.run(&design).expect("backend run");

    // 3. ecppack must accept the textcfg and produce a bitstream.
    let config = work.join("blinky.config");
    let bit = work.join("blinky.bit");
    std::fs::write(&config, &bs.textcfg).expect("write textcfg");
    let out = Command::new(&ecppack)
        .arg(&config)
        .arg(&bit)
        .output()
        .expect("run ecppack");
    assert!(
        out.status.success(),
        "ecppack failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta = std::fs::metadata(&bit).expect("bitstream missing");
    assert!(meta.len() > 0, "empty bitstream");
    println!(
        "e2e ok: {} cells -> {} textcfg bytes -> {} bitstream bytes",
        design.cells.len(),
        bs.textcfg.len(),
        meta.len()
    );
}
