//! Global clock network tests (Part A) and the neigh_op routing test.
//!
//! - A clocked counter with a `ClockBuf` must ride the global network:
//!   the `.asc` carries the `ColBufCtrl` bits and the per-tile
//!   `glb_netwk -> lutff_global/clk` buffers, and no edge routes the
//!   clock through fabric to `lutff_global/clk`.
//! - A PLL-clocked design must drive `PLLOUTGLOBAL` through the global
//!   network via the `padin_glb_netwk` extra bit.
//! - icepack/iceunpack round-trips must preserve all of the above.
//! - A buffer chain spread over tiles must let the router use `neigh_op`
//!   edges (Part B).

use helion_backend::Backend;
use helion_ice40::route::Router;
use helion_ice40::Ice40Backend;
use helion_ir::{intern, CellKind, Design, PortDir};
use std::collections::HashMap;
use std::process::Command;

/// Parse `.asc` tile sections into `(kind, x, y) -> 16 bank strings`.
fn asc_tiles(asc: &str) -> HashMap<(String, u8, u8), Vec<String>> {
    let mut m = HashMap::new();
    let lines: Vec<&str> = asc.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let ln = lines[i];
        if ln.starts_with(".io_tile")
            || ln.starts_with(".logic_tile")
            || ln.starts_with(".ramb_tile")
            || ln.starts_with(".ramt_tile")
        {
            let parts: Vec<&str> = ln.split_whitespace().collect();
            let banks: Vec<String> = lines[i + 1..i + 17].iter().map(|s| s.to_string()).collect();
            m.insert(
                (
                    parts[0].to_string(),
                    parts[1].parse().unwrap(),
                    parts[2].parse().unwrap(),
                ),
                banks,
            );
            i += 17;
        } else {
            i += 1;
        }
    }
    m
}

fn bit(banks: &[String], bank: usize, bit: usize) -> bool {
    banks[bank].as_bytes()[bit] == b'1'
}

/// icepack -> iceunpack round-trip; returns the re-emitted text.
fn round_trip(asc: &str, stem: &str) -> String {
    if !std::path::Path::new("/usr/bin/icepack").exists() {
        return asc.to_string();
    }
    let dir = "/tmp/helion_gclk";
    std::fs::create_dir_all(dir).expect("mkdir");
    let asc_path = format!("{dir}/{stem}.asc");
    let bin_path = format!("{dir}/{stem}.bin");
    let asc2_path = format!("{dir}/{stem}.rt.asc");
    std::fs::write(&asc_path, asc).expect("write asc");
    let st = Command::new("/usr/bin/icepack")
        .args([&asc_path, &bin_path])
        .status()
        .expect("icepack runs");
    assert!(st.success(), "icepack failed on {stem}");
    let st = Command::new("/usr/bin/iceunpack")
        .args([&bin_path, &asc2_path])
        .status()
        .expect("iceunpack runs");
    assert!(st.success(), "iceunpack failed on {stem}");
    std::fs::read_to_string(&asc2_path).expect("read round-tripped asc")
}

/// Column-buffer tile row covering logic-tile row `y` (colbuf_db "8k").
fn colbuf_src_y(y: u8) -> u8 {
    if y <= 8 {
        8
    } else if y <= 16 {
        9
    } else if y <= 24 {
        24
    } else {
        25
    }
}

/// `ColBufCtrl` bit `(bank, bit)` for `glb_netwk_N` in a logic tile
/// (B9[7], B8[7], B11[7], B10[7], B13[7], B12[7], B15[7], B14[7]).
fn colbuf_bit_logic(glb: u8) -> (usize, usize) {
    [(9, 7), (8, 7), (11, 7), (10, 7), (13, 7), (12, 7), (15, 7), (14, 7)][glb as usize]
}

/// Build a 4-bit binary counter driving led0 from clk (via a ClockBuf).
fn counter_design() -> Design {
    let mut d = Design::new("gclk_counter");
    d.add_port("clk", PortDir::In);
    d.add_port("led0", PortDir::Out);
    d.add_cell("clk_ib", CellKind::IobIn);
    d.connect("clk_net", "clk_ib", "O");
    d.connect("clk", "clk_ib", "PAD");
    d.add_cell("clk_buf", CellKind::ClockBuf);
    d.connect("clk_net", "clk_buf", "I");
    d.connect("clk_global", "clk_buf", "O");
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
        d.add_cell(&lut, CellKind::Lut6 { init });
        d.add_cell(&ff, CellKind::Hff);
        for j in 0..=i {
            d.connect(&format!("q{j}"), &lut, &format!("I{j}"));
        }
        d.connect(&next, &lut, "O");
        d.connect(&next, &ff, "D");
        d.connect(&q, &ff, "Q");
        d.connect("clk_global", &ff, "CLK");
    }
    d.add_cell("led_ob", CellKind::IobOut);
    d.connect("q3", "led_ob", "I");
    d.connect("led0", "led_ob", "PAD");
    d
}

#[test]
fn gclk_counter_end_to_end() {
    let design = counter_design();
    let backend = Ice40Backend::new("ice40-hx8k");
    let packed = backend.pack(&design).expect("pack");
    assert_eq!(packed.clks.len(), 1);
    let placed = backend.place(&packed).expect("place");
    let routed = backend.route(&placed).expect("route");
    let router = Router::build();

    // One global clock: clk on J3 = IO tile (0,16), co-located with the
    // gbufin tile for glb net 6 (icebox gbufin_db "8k").
    assert_eq!(routed.glb_clocks.len(), 1);
    let gc = &routed.glb_clocks[0];
    assert_eq!(gc.glb, 6);
    assert_eq!(gc.extra_bit, None);
    assert!(!gc.tiles.is_empty(), "FF tiles recorded");

    // NOT a fabric-routed clock: no routed edge targets lutff_global/clk.
    for net in &routed.nets {
        for e in &net.edges {
            assert_ne!(
                router.rel_dst(e.kind_idx, e.rel_idx),
                "lutff_global/clk",
                "clock reaches lutff_global/clk through fabric"
            );
        }
    }
    // The source net reaches the global buffer: clk_net has a fabout load.
    let clk_net = routed
        .nets
        .iter()
        .find(|n| n.name == "clk_net")
        .expect("clk_net routed");
    assert!(
        clk_net
            .edges
            .iter()
            .any(|e| router.rel_dst(e.kind_idx, e.rel_idx) == "fabout"),
        "clk_net must be routed to the global buffer's fabout"
    );

    let asc = backend.bitstream(&routed).expect("bitstream");
    assert!(!asc.contains(".extra_bit"), "no padin bit for a pin clock");
    let tiles = asc_tiles(&asc);
    // Per-tile glb_netwk_6 -> lutff_global/clk buffer: B2[0],B2[1],B2[2]
    // (from the logic_8k DB entry).
    for &(x, y) in &gc.tiles {
        let t = &tiles[&(".logic_tile".to_string(), x, y)];
        assert!(bit(t, 2, 0) && bit(t, 2, 1) && bit(t, 2, 2),
            "glb_netwk_6 -> lutff_global/clk not programmed at ({x},{y})");
        // ColBufCtrl glb_netwk_6 in the covering column-buffer tile.
        let by = colbuf_src_y(y);
        let bt = &tiles[&(".logic_tile".to_string(), x, by)];
        let (bank, b) = colbuf_bit_logic(6);
        assert!(bit(bt, bank, b),
            "ColBufCtrl glb_netwk_6 not set at ({x},{by})");
    }

    // icepack/iceunpack round-trip preserves the global-network bits.
    let asc2 = round_trip(&asc, "gclk_counter");
    let tiles2 = asc_tiles(&asc2);
    for &(x, y) in &gc.tiles {
        let t = &tiles2[&(".logic_tile".to_string(), x, y)];
        assert!(bit(t, 2, 0) && bit(t, 2, 1) && bit(t, 2, 2),
            "clk buffer lost at ({x},{y}) in round-trip");
        let by = colbuf_src_y(y);
        let bt = &tiles2[&(".logic_tile".to_string(), x, by)];
        let (bank, b) = colbuf_bit_logic(6);
        assert!(bit(bt, bank, b), "ColBufCtrl lost at ({x},{by}) in round-trip");
    }
}

/// PLL-clocked design: SB_PLL40_PAD -> ClockBuf -> FFs.
fn pll_clocked_design() -> Design {
    let mut d = Design::new("gclk_pll");
    d.add_port("clk", PortDir::In);
    d.add_port("led0", PortDir::Out);
    d.add_cell("clk_ib", CellKind::IobIn);
    d.connect("clk_net", "clk_ib", "O");
    d.connect("clk", "clk_ib", "PAD");
    d.add_cell(
        "pll0",
        CellKind::BlackBox {
            module: intern("SB_PLL40_PAD"),
        },
    );
    d.connect("clk_net", "pll0", "REFERENCECLK");
    d.connect("pll_clk", "pll0", "PLLOUTGLOBAL");
    d.set_cell_attr("pll0", "FREQ", "48000000").unwrap();
    d.set_cell_attr("pll0", "REF_FREQ", "12000000").unwrap();
    d.add_cell("clk_buf", CellKind::ClockBuf);
    d.connect("pll_clk", "clk_buf", "I");
    d.connect("clk_global", "clk_buf", "O");
    // 2-bit counter on the PLL clock.
    for i in 0..2 {
        let lut = format!("next{i}_lut");
        let ff = format!("bit{i}_ff");
        d.add_cell(&lut, CellKind::Lut6 { init: 0xAAAAAAAAAAAAAAAA });
        d.add_cell(&ff, CellKind::Hff);
        d.connect("q0", &lut, "I0");
        if i == 1 {
            d.connect("q1", &lut, "I1");
        }
        let next = format!("next{i}");
        let q = format!("q{i}");
        d.connect(&next, &lut, "O");
        d.connect(&next, &ff, "D");
        d.connect(&q, &ff, "Q");
        d.connect("clk_global", &ff, "CLK");
    }
    d.add_cell("led_ob", CellKind::IobOut);
    d.connect("q1", "led_ob", "I");
    d.connect("led0", "led_ob", "PAD");
    d
}

#[test]
fn gclk_pll_goes_through_global_network() {
    let design = pll_clocked_design();
    let backend = Ice40Backend::new("ice40-hx8k");
    let packed = backend.pack(&design).expect("pack");
    assert_eq!(packed.plls.len(), 1);
    assert_eq!(packed.clks.len(), 1);
    let placed = backend.place(&packed).expect("place");
    let routed = backend.route(&placed).expect("route");
    let router = Router::build();

    // Bottom PLL -> glb net 6 via padin_glb_netwk (icebox extra_bits_db).
    assert_eq!(routed.glb_clocks.len(), 1);
    let gc = &routed.glb_clocks[0];
    assert_eq!(gc.glb, 6);
    assert_eq!(gc.extra_bit, Some((0, 870, 271)));
    assert!(!gc.tiles.is_empty());

    // NOT a fabric-routed clock.
    for net in &routed.nets {
        for e in &net.edges {
            assert_ne!(
                router.rel_dst(e.kind_idx, e.rel_idx),
                "lutff_global/clk",
                "PLL clock reaches lutff_global/clk through fabric"
            );
        }
    }
    // The PLL output net is not routed through fabric at all (padin bit).
    assert!(
        routed.nets.iter().all(|n| n.name != "pll_clk"),
        "pll_clk must not be a routed fabric net"
    );

    let asc = backend.bitstream(&routed).expect("bitstream");
    assert!(
        asc.contains(".extra_bit 0 870 271"),
        "padin_glb_netwk 6 bit missing"
    );
    let tiles = asc_tiles(&asc);
    for &(x, y) in &gc.tiles {
        let t = &tiles[&(".logic_tile".to_string(), x, y)];
        assert!(bit(t, 2, 0) && bit(t, 2, 1) && bit(t, 2, 2),
            "glb_netwk_6 -> lutff_global/clk not programmed at ({x},{y})");
        let by = colbuf_src_y(y);
        let bt = &tiles[&(".logic_tile".to_string(), x, by)];
        let (bank, b) = colbuf_bit_logic(6);
        assert!(bit(bt, bank, b),
            "ColBufCtrl glb_netwk_6 not set at ({x},{by})");
    }

    // Round-trip preserves the extra bit and the buffers.
    let asc2 = round_trip(&asc, "gclk_pll");
    assert!(asc2.contains(".extra_bit 0 870 271"), "extra bit lost");
    let tiles2 = asc_tiles(&asc2);
    for &(x, y) in &gc.tiles {
        let t = &tiles2[&(".logic_tile".to_string(), x, y)];
        assert!(bit(t, 2, 0) && bit(t, 2, 1) && bit(t, 2, 2),
            "clk buffer lost at ({x},{y}) in round-trip");
    }
}

#[test]
fn gclk_two_clocks_get_distinct_global_nets() {
    let mut d = counter_design();
    // Second clock on another pin (LOC'd to an LED pin used as input).
    d.add_port("clk2", PortDir::In);
    d.set_loc("clk2", "B4").unwrap();
    d.add_cell("clk2_ib", CellKind::IobIn);
    d.connect("clk2_net", "clk2_ib", "O");
    d.connect("clk2", "clk2_ib", "PAD");
    d.add_cell("clk2_buf", CellKind::ClockBuf);
    d.connect("clk2_net", "clk2_buf", "I");
    d.connect("clk2_global", "clk2_buf", "O");
    // One FF on the second clock.
    d.add_cell("ff2_lut", CellKind::Lut6 { init: 0xAAAAAAAAAAAAAAAA });
    d.add_cell("ff2", CellKind::Hff);
    d.connect("q3", "ff2_lut", "I0");
    d.connect("w2", "ff2_lut", "O");
    d.connect("w2", "ff2", "D");
    d.connect("q4", "ff2", "Q");
    d.connect("clk2_global", "ff2", "CLK");

    let backend = Ice40Backend::new("ice40-hx8k");
    let packed = backend.pack(&d).expect("pack");
    assert_eq!(packed.clks.len(), 2);
    let placed = backend.place(&packed).expect("place");
    let routed = backend.route(&placed).expect("route");

    assert_eq!(routed.glb_clocks.len(), 2);
    let glbs: Vec<u8> = routed.glb_clocks.iter().map(|g| g.glb).collect();
    assert_ne!(glbs[0], glbs[1], "two clocks must use distinct glb nets");
    assert!(routed.glb_clocks.iter().all(|g| !g.tiles.is_empty()));

    let asc = backend.bitstream(&routed).expect("bitstream");
    let tiles = asc_tiles(&asc);
    // Both clocks' column buffers are programmed.
    for gc in &routed.glb_clocks {
        let (x, y) = gc.tiles[0];
        let by = colbuf_src_y(y);
        let bt = &tiles[&(".logic_tile".to_string(), x, by)];
        let (bank, b) = colbuf_bit_logic(gc.glb);
        assert!(bit(bt, bank, b),
            "ColBufCtrl glb_netwk_{} not set", gc.glb);
    }
    let asc2 = round_trip(&asc, "gclk_two");
    let tiles2 = asc_tiles(&asc2);
    for gc in &routed.glb_clocks {
        let (x, y) = gc.tiles[0];
        let by = colbuf_src_y(y);
        let bt = &tiles2[&(".logic_tile".to_string(), x, by)];
        let (bank, b) = colbuf_bit_logic(gc.glb);
        assert!(bit(bt, bank, b), "ColBufCtrl lost in round-trip");
    }
}

#[test]
fn neigh_op_edges_are_used_by_the_router() {
    // A buffer chain long enough to spread over several tiles: inter-tile
    // hops between adjacent tiles should take the 1-hop neigh_op edges.
    let mut d = Design::new("chain");
    d.add_port("clk", PortDir::In);
    d.add_port("led0", PortDir::Out);
    d.add_cell("clk_ib", CellKind::IobIn);
    d.connect("clk_net", "clk_ib", "O");
    d.connect("clk", "clk_ib", "PAD");
    d.add_cell("clk_buf", CellKind::ClockBuf);
    d.connect("clk_net", "clk_buf", "I");
    d.connect("clk_global", "clk_buf", "O");
    let mut prev = "clk_net".to_string();
    for i in 0..16 {
        let lut = format!("buf{i}");
        let w = format!("w{i}");
        d.add_cell(&lut, CellKind::Lut6 { init: 0xAAAAAAAAAAAAAAAA });
        d.connect(&prev, &lut, "I0");
        d.connect("clk_net", &lut, "I1");
        d.connect(&w, &lut, "O");
        prev = w;
    }
    d.add_cell("ff0", CellKind::Hff);
    d.connect(&prev, "ff0", "D");
    d.connect("q", "ff0", "Q");
    d.connect("clk_global", "ff0", "CLK");
    d.add_cell("led_ob", CellKind::IobOut);
    d.connect("q", "led_ob", "I");
    d.connect("led0", "led_ob", "PAD");

    let backend = Ice40Backend::new("ice40-hx8k");
    let packed = backend.pack(&d).expect("pack");
    let placed = backend.place(&packed).expect("place");
    let routed = backend.route(&placed).expect("route");
    let router = Router::build();

    let mut neigh = 0;
    for net in &routed.nets {
        for e in &net.edges {
            let src = router.rel_src(e.kind_idx, e.rel_idx);
            if src.starts_with("neigh_op_") || src.starts_with("logic_op_") {
                // The edge must be programmed in a real tile with real bits.
                assert!(!router.edge_bits(e.kind_idx, e.rel_idx).is_empty());
                neigh += 1;
            }
        }
    }
    assert!(neigh > 0, "router never used a neigh_op edge");
    println!("neigh_op edges used: {neigh}");

    // The design must still produce a valid bitstream.
    let asc = backend.bitstream(&routed).expect("bitstream");
    assert!(asc.starts_with(".device 8k\n"));
    round_trip(&asc, "neigh_op_chain");
}
