//! End-to-end: blinky.sv → pack → place → route → FASM for xc7a35t.
//!
//! Requires the X-Ray DB at `~/workspace/xilinx-dbsrc` (or `HELION_XILINX_DB`).

use helion_xilinx7::device::{default_db_root, Device};
use helion_xilinx7::xdevice::{build_node_map, XDevice};
use helion_xilinx7::{emit_fasm, pack, place, route};

#[test]
fn blinky_e2e_fasm() {
    let db_root = default_db_root();
    let mut device = Device::load_xc7a35t(&db_root).expect("load device");
    assert!(device.tile_count() > 10000, "tile_count = {}", device.tile_count());
    let node_maps = build_node_map(&mut device);

    // Synthesize.
    let src = std::fs::read_to_string("../../examples/blinky.sv").expect("blinky.sv");
    let mut design = helion_sv::synth_sv(&src, "blinky.sv").expect("synth");
    // LOC constraints (Arty A7).
    for port in &mut design.ports {
        match port.name.as_str() {
            "clk" => port.attrs.set("LOC", "E3"),
            "led" => port.attrs.set("LOC", "H5"),
            _ => {}
        }
    }

    let packed = pack(&design).expect("pack");
    assert!(!packed.luts.is_empty(), "no LUTs packed");
    assert!(!packed.ffs.is_empty(), "no FFs packed");
    assert_eq!(packed.bufgs.len(), 1, "expected 1 BUFG");

    let mut xdev = XDevice::new(&device, node_maps);
    // Clock nets for the router.
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = place(&packed, &xdev).expect("place");
    assert_eq!(placed.iobs.len(), 2, "expected 2 IOBs");

    let routed = route(&placed, &xdev).expect("route");
    assert!(!routed.nets.is_empty(), "no nets routed");

    let bs = emit_fasm(&routed, &xdev).expect("emit");
    assert!(!bs.fasm.is_empty(), "empty FASM");
    let text = bs.fasm.join("\n");

    // Spot-check key features.
    assert!(text.contains(".ALUT.INIT[63:0]"), "missing LUT INIT:\n{text}");
    assert!(text.contains("LVCMOS"), "missing IOB config:\n{text}");
    assert!(text.contains("BUFGCTRL"), "missing BUFG:\n{text}");

    // A flop needs BOTH of its configuration bits, and the old assertion here
    // was `contains(".AFF.ZINI") || contains("FF.ZINI")` -- the `||` made it
    // pass on a build with no `AFF.ZINI` at all, and it never looked at `ZRST`.
    //
    // `ZINI` sets the init value and `ZRST` selects what the SR pin does to it.
    // Leaving `ZRST` unconfigured is precisely the failure this project spent
    // rounds on: the flop holds its init value, the LED lights, and nothing
    // ever toggles it. That is a configured-looking bitstream that does not
    // run, and no test in the tree noticed.
    //
    // Assert per packed FF, so the count has to match the netlist rather than
    // merely being non-zero.
    for f in &placed.ffs {
        // `placed.ffs[].bel` is already `"AFF"`..`"DFF"`, the same spelling the
        // emitter writes.
        let zini = format!(".{}.ZINI", f.bel);
        let zrst = format!(".{}.ZRST", f.bel);
        assert!(
            text.contains(&zini),
            "FF {} has no {}: its power-up value is unconfigured\n{text}",
            f.name,
            zini
        );
        assert!(
            text.contains(&zrst),
            "FF {} has no {}: SRVAL is unconfigured, which is NOT the same as \
             no effect -- the register then stores 1, so a design that inits to \
             0 powers up as a constant 1\n{text}",
            f.name,
            zrst
        );
    }
    // Tie the check to the netlist rather than to this design's shape: every
    // packed FF must have been placed, or the loop above validated fewer
    // flops than exist and would silently pass on a truncated placement.
    assert_eq!(
        placed.ffs.len(),
        packed.ffs.len(),
        "placed {} of {} flops; the ZINI/ZRST checks above only covered the \
         ones that were placed",
        placed.ffs.len(),
        packed.ffs.len()
    );

    // No one-hot mux may have two arms written.
    //
    // `BYP_ALT<n>` and `IMUX<n>` select exactly one source, and prjxray encodes
    // each arm as a coarse select bit plus a fine one WITHOUT clearing the
    // other coarse bits. Two arms therefore light two coarse bits and the mux
    // drives neither input -- a configured-looking bitstream with an undefined
    // data path.
    //
    // This actually happened: the clock pass wrote `BYP_ALT1.GFAN0` while the
    // router had routed a counter bit through `BYP_ALT1.SR1BEG_S0` on the same
    // tile, leaving `INT_R_X71Y41.BYP_ALT1` with both coarse bits set. The
    // reference never does this anywhere on the die, and every test passed.
    let mut arms: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for l in &bs.fasm {
        let mut it = l.splitn(3, '.');
        let (Some(tile), Some(field), Some(arm)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let is_mux = ["IMUX", "BYP_ALT", "FAN_ALT"].iter().any(|p| {
            field
                .strip_prefix(p)
                .is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()))
        });
        if is_mux {
            arms.entry(format!("{tile}.{field}"))
                .or_default()
                .push(arm.to_string());
        }
    }
    for (mux, a) in &arms {
        assert!(
            a.len() == 1,
            "one-hot mux {mux} has {} arms selected: {a:?}. The arms' coarse \
             select bits are not cleared against each other, so the mux drives \
             neither input.\n{text}",
            a.len()
        );
    }

    // Write FASM for inspection / Worker C validation.
    let out = std::path::PathBuf::from("/tmp/x7/blinky.fasm");
    std::fs::create_dir_all(out.parent().unwrap()).ok();
    std::fs::write(&out, bs.fasm.join("\n") + "\n").unwrap();
    println!("wrote {} ({} lines)", out.display(), bs.fasm.len());
    println!("--- FASM ---\n{text}\n--- end ---");
}
