//! Boards and parts: every board's pin file names real user I/O pins of its
//! part, every board's part resolves from the Project X-Ray database, and a
//! counter builds to a bitstream for every board through the whole flow
//! (v2 front end, pack, place, route, FASM, `.bit`), with the part's IDCODE
//! and no FASM line the encoder cannot place.
//!
//! The bitstreams are checked against their RTL by `tools/x7v`
//! (`tools/x7v/tests/boards.sh`), which shares no code with this crate.
//!
//! Needs the X-Ray database (`HELION_XILINX_DB`, see `device.rs`); without
//! it the tests say SKIP and pass.

use std::collections::HashMap;
use std::path::PathBuf;

use helion_backend::Backend;
use helion_ir::{resolve, CellKind};
use helion_xilinx7::backend::Xilinx7Backend;
use helion_xilinx7::device::default_db_root;
use helion_xilinx7::parts::{self, BOARDS};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn have_db() -> bool {
    let ok = !parts::families(&default_db_root()).is_empty();
    if !ok {
        eprintln!("SKIP: no Project X-Ray database at {}", default_db_root().display());
    }
    ok
}

/// `(port bit, package pin)` pairs of an XDC, in file order.
fn xdc_pins(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        let (Some(a), Some(b)) = (line.find("PACKAGE_PIN"), line.find("get_ports")) else { continue };
        let pin = line[a + "PACKAGE_PIN".len()..].split_whitespace().next().unwrap_or("").to_string();
        let rest = &line[b + "get_ports".len()..];
        let port = match (rest.find('{'), rest.find('}')) {
            (Some(i), Some(j)) if i < j => rest[i + 1..j].trim().to_string(),
            _ => rest.trim().trim_end_matches(']').trim().to_string(),
        };
        out.push((port, pin));
    }
    out
}

/// `pin -> (bank, site)` from the part's `package_pins.csv`.
fn package_pins(part: &parts::Part) -> HashMap<String, (String, String)> {
    let text = std::fs::read_to_string(part.part_dir().join("package_pins.csv")).unwrap();
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            (f.len() >= 3).then(|| (f[0].to_string(), (f[1].to_string(), f[2].to_string())))
        })
        .collect()
}

#[test]
fn board_parts_resolve() {
    if !have_db() {
        return;
    }
    let root = default_db_root();
    let want = [
        ("zedboard", "zynq7", "xc7z020", 0x0372_7093),
        ("arty-a7-35", "artix7", "xc7a50t", 0x0362_D093),
        ("basys3", "artix7", "xc7a50t", 0x0362_D093),
        ("nexys-a7-100t", "artix7", "xc7a100t", 0x0363_1093),
    ];
    for (b, family, fabric, idcode) in want {
        let p = parts::resolve(&root, parts::board(b).unwrap().part).unwrap();
        assert_eq!((p.family.as_str(), p.fabric.as_str(), p.idcode), (family, fabric, idcode), "{b}");
    }
    // The short names Helion has always accepted.
    assert_eq!(parts::resolve(&root, "xc7a35t").unwrap().name, "xc7a35tcsg324-1");
    assert_eq!(parts::resolve(&root, "xc7z020").unwrap().name, "xc7z020clg484-1");
    let err = parts::resolve(&root, "xc7k325tffg900-2").unwrap_err();
    assert!(err.contains("unknown part"), "{err}");
}

/// Every pin in every board file is a user I/O pin of the board's part, and
/// no pin is given twice.
#[test]
fn board_xdc_pins_exist_on_their_parts() {
    if !have_db() {
        return;
    }
    let root = default_db_root();
    for b in BOARDS {
        let part = parts::resolve(&root, b.part).unwrap();
        let pkg = package_pins(&part);
        let text = std::fs::read_to_string(repo().join(b.xdc)).unwrap();
        let pins = xdc_pins(&text);
        assert!(pins.len() >= 10, "{}: only {} pins", b.name, pins.len());
        let mut seen = HashMap::new();
        for (port, pin) in &pins {
            let (_bank, site) = pkg.get(pin).unwrap_or_else(|| panic!("{}: {port} on {pin}, which {} does not have", b.name, b.part));
            assert!(site.starts_with("IOB"), "{}: {port} on {pin} is {site}, not a user I/O", b.name);
            if let Some(other) = seen.insert(pin.clone(), port.clone()) {
                panic!("{}: {pin} given to both {other} and {port}", b.name);
            }
        }
        assert!(pins.iter().any(|(p, _)| p == "clk"), "{}: no clock", b.name);
    }
}

/// A counter on each board: v2 front end to `.bit`, using the board's own
/// pin file.
#[test]
fn counter_builds_for_every_board() {
    if !have_db() {
        return;
    }
    // Four LEDs: the fewest any of the boards has.
    let src = "module counter(input logic clk, input logic [1:0] sw, output logic [3:0] led);
  logic [7:0] q = 8'd0;
  always_ff @(posedge clk) begin
    if (sw[0]) q <= 8'd0;
    else if (sw[1]) q <= q + 8'd1;
  end
  assign led = q[7:4];
endmodule
";
    for b in BOARDS {
        let mut d = helion_sv::synth_sv_v2(src, "counter.sv", Some("counter")).expect("synth").design;
        let xdc: HashMap<String, String> = xdc_pins(&std::fs::read_to_string(repo().join(b.xdc)).unwrap()).into_iter().collect();
        d.set_loc("clk", &xdc["clk"]).unwrap();
        let be = Xilinx7Backend::open(b.part).unwrap_or_else(|e| panic!("{}: {e}", b.name));
        let mut packed = be.pack(&d).unwrap();
        // Bus bits: outputs by their pad's PORT/BIT, inputs by `port_<bit>`.
        let bit_of: HashMap<String, (String, String)> = d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::IobOut))
            .filter_map(|c| Some((resolve(c.name).to_string(), (c.attrs.get("PORT")?.to_string(), c.attrs.get("BIT")?.to_string()))))
            .collect();
        for iob in packed.iobs.iter_mut() {
            let key = match bit_of.get(&iob.name) {
                Some((p, bit)) => format!("{p}[{bit}]"),
                None => match iob.net.rsplit_once('_') {
                    Some((p, bit)) if bit.chars().all(|c| c.is_ascii_digit()) => format!("{p}[{bit}]"),
                    _ => iob.port.clone(),
                },
            };
            iob.loc = Some(xdc.get(&key).unwrap_or_else(|| panic!("{}: no pin for {key}", b.name)).clone());
        }
        let routed = be.route(&be.place(&packed).unwrap()).unwrap();
        let fasm = be.bitstream(&routed).unwrap().fasm.join("\n") + "\n";
        let r = helion_xilinx7::bitgen::fasm_to_bit(&fasm, b.part, "counter").unwrap();
        assert!(r.rejected.is_empty(), "{}: {:?}", b.name, &r.rejected[..r.rejected.len().min(5)]);
        // The IDCODE write: type-1 header 0x30018001, then the IDCODE.
        // (The .bit header is not word-aligned: search the bytes.)
        let at = r.bit.windows(4).position(|w| w == [0x30, 0x01, 0x80, 0x01]).expect("IDCODE write");
        let id = u32::from_be_bytes(r.bit[at + 4..at + 8].try_into().unwrap());
        let want = parts::resolve(&default_db_root(), b.part).unwrap().idcode;
        assert_eq!(id, want, "{}: IDCODE", b.name);
    }
}
