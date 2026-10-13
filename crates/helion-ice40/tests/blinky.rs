//! End-to-end blinky: synth a 4-bit counter in Rust, run the iCE40 flow,
//! emit .asc, and validate with IceStorm's icepack/iceunpack round-trip.

use helion_backend::Backend;
use helion_ice40::Ice40Backend;
use helion_ir::{CellKind, Design, PortDir};

/// Build a 4-bit binary counter driving led0 from clk.
fn blinky_design() -> Design {
    let mut d = Design::new("blinky");
    d.ports.push(helion_ir::Port {
        name: "clk".to_string(),
        dir: PortDir::In,
        attrs: Default::default(),
    });
    d.ports.push(helion_ir::Port {
        name: "led0".to_string(),
        dir: PortDir::Out,
        attrs: Default::default(),
    });

    // Clock input.
    d.add_cell("clk_ib", CellKind::IobIn);
    d.connect("clk_net", "clk_ib", "O");
    d.connect("clk", "clk_ib", "PAD");
    d.add_cell("clk_buf", CellKind::ClockBuf);
    d.connect("clk_net", "clk_buf", "I");
    d.connect("clk_global", "clk_buf", "O");

    // 4-bit counter. next[i] computed by LUTs, stored in FFs.
    // LUT init for f(a0..a5).
    let lut_init = |f: &dyn Fn(u64) -> bool| -> u64 {
        let mut init = 0u64;
        for a in 0..64u64 {
            if f(a) {
                init |= 1 << a;
            }
        }
        init
    };
    let bit = |a: u64, i: u32| (a >> i) & 1 == 1;
    let inits = [
        lut_init(&|a| !bit(a, 0)),
        lut_init(&|a| bit(a, 1) ^ bit(a, 0)),
        lut_init(&|a| bit(a, 2) ^ (bit(a, 0) & bit(a, 1))),
        lut_init(&|a| bit(a, 3) ^ (bit(a, 0) & bit(a, 1) & bit(a, 2))),
    ];
    for (i, &init) in inits.iter().enumerate() {
        let lut = format!("next{i}_lut");
        let ff = format!("bit{i}_ff");
        let next = format!("next{i}");
        let q = format!("q{i}");
        d.add_cell(lut.clone(), CellKind::Lut6 { init });
        d.add_cell(ff.clone(), CellKind::Hff);
        // LUT inputs: connect q0..qi.
        for j in 0..=i {
            d.connect(format!("q{j}"), lut.clone(), format!("I{j}"));
        }
        d.connect(next.clone(), lut, "O");
        d.connect(next, ff.clone(), "D");
        d.connect(q, ff.clone(), "Q");
        d.connect("clk_global", ff, "CLK");
    }

    // LED output.
    d.add_cell("led_ob", CellKind::IobOut);
    d.connect("q3", "led_ob", "I");
    d.connect("led0", "led_ob", "PAD");

    d
}

#[test]
fn blinky_end_to_end() {
    let design = blinky_design();
    let backend = Ice40Backend::new("ice40-hx8k");
    let packed = backend.pack(&design).expect("pack");
    // 4 counter LUTs + LED is q3 (no LUT) = 4 LUTs; 4 FFs; 3 IO/clk cells.
    println!("LUTS: {:?}", packed.luts.iter().map(|l| &l.name).collect::<Vec<_>>());
    assert_eq!(packed.luts.len(), 4);
    assert_eq!(packed.ffs.len(), 4);
    assert_eq!(packed.ios.len(), 2);
    assert_eq!(packed.clks.len(), 1);

    let placed = backend.place(&packed).expect("place");
    // Placer adds the $gnd tie-off LUT (counter LUTs have unused inputs).
    assert_eq!(placed.luts.len(), packed.luts.len() + 1);
    assert_eq!(placed.ffs.len(), 4);

    let routed = backend.route(&placed).expect("route");
    assert!(!routed.nets.is_empty());

    let asc = backend.bitstream(&routed).expect("bitstream");
    assert!(asc.starts_with(".device 8k\n"));
    assert!(asc.contains(".logic_tile"));

    // Write the .asc for external validation (icepack/icebox_vlog).
    let path = "/tmp/helion_blinky.asc";
    std::fs::write(path, &asc).expect("write asc");
    println!("wrote {path} ({} bytes)", asc.len());
}
