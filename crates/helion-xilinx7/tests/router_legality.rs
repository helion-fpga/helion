//! A 7-series INT mux output is one-hot.
//!
//! Two nets selecting different inputs of the same mux output is an
//! electrical over-drive, and prjxray writes some of those selects with
//! OPPOSITE polarity on the SAME configuration bit (e.g.
//! `INT_R.BYP_ALT4.SR1BEG_S0` writes `23_23` while `.NN2END1` writes
//! `!23_23`), so the bitstream commits an illegal configuration while
//! `route()` returns `Ok` and every other test passes.
//!
//! Routing each net independently produced 30 such conflicts on the
//! ZedBoard blinky. This asserts none exist.

use std::collections::{HashMap, HashSet};

fn default_db_root() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("HELION_XILINX_DB") {
        return std::path::PathBuf::from(p);
    }
    // tests run with CWD = the crate root, so the DB sits three levels up.
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../work/prjxray-db")
        .canonicalize()
        .expect("set HELION_XILINX_DB to the prjxray-db root")
}

#[test]
fn no_mux_output_is_driven_twice() {
    let db = default_db_root();
    let mut device = helion_xilinx7::Device::load_xc7z020(&db).expect("load xc7z020");
    let node_maps = helion_xilinx7::build_node_map(&mut device);
    let xdev = helion_xilinx7::XDevice::new(&device, node_maps);

    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/zedboard_blinky.sv")).expect("blinky");
    let mut d = helion_sv::synth_sv(&src, "zedboard_blinky.sv").expect("synth");

    // Pin the clock and the LED ports, as the board's XDC does.
    for port in &mut d.ports {
        match port.name.as_str() {
            "clk" => {
                port.attrs.set("LOC", "Y9");
                port.attrs.set("IOSTANDARD", "LVCMOS33");
            }
            "led" => {
                port.attrs.set("IOSTANDARD", "LVCMOS33");
            }
            _ => {}
        }
    }

    let mut packed = helion_xilinx7::pack(&d).expect("pack");
    // The LED driver nets are `cnt_26` .. `cnt_19` for led[0] .. led[7]; no
    // fixup is needed here because the front end now connects them directly.
    let led_pins = ["T22", "T21", "U22", "U21", "V22", "W22", "U19", "U14"];
    let mut n_led = 0;
    for iob in &mut packed.iobs {
        if iob.port != "led" {
            continue;
        }
        let m: u32 = iob
            .net
            .strip_prefix("cnt_")
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("unexpected led driver net `{}`", iob.net));
        assert!((19..=26).contains(&m), "cnt bit out of range: {m}");
        iob.loc = Some(led_pins[(26 - m) as usize].to_string());
        iob.iostandard = "LVCMOS33".to_string();
        n_led += 1;
    }
    assert_eq!(n_led, 8, "expected 8 led IOBs driven by cnt_26..cnt_19");

    let mut xdev = xdev;
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = helion_xilinx7::place(&packed, &xdev).expect("place");
    let routed = helion_xilinx7::route(&placed, &xdev).expect("route");

    // (tile, dst_wire) -> the set of source wires driving it.
    let mut drivers: HashMap<(String, String), HashSet<String>> = HashMap::new();
    let mut npips = 0usize;
    for n in &routed.nets {
        if let helion_xilinx7::route::RouteKind::Pips(pips) = &n.kind {
            for pip in pips {
                // Pseudo-pips are always-on and never emitted; tileconn
                // stitches are marked with `~` in the tile name.
                if pip.is_pseudo || pip.tile.contains('~') {
                    continue;
                }
                npips += 1;
                drivers
                    .entry((pip.tile.clone(), pip.dst_wire.clone()))
                    .or_default()
                    .insert(pip.src_wire.clone());
            }
        }
    }
    assert!(npips > 100, "expected a real route, got {npips} pips");

    let mut conflicts: Vec<_> = drivers
        .iter()
        .filter(|(_, s)| s.len() > 1)
        .map(|(k, s)| {
            let mut v: Vec<_> = s.iter().cloned().collect();
            v.sort();
            (k.clone(), v)
        })
        .collect();
    conflicts.sort();
    for (k, s) in conflicts.iter().take(10) {
        eprintln!("MUX_OVERDRIVE {}.{} <- {s:?}", k.0, k.1);
    }
    assert!(
        conflicts.is_empty(),
        "{} mux output(s) are driven by more than one input",
        conflicts.len()
    );
}
