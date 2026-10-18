use std::collections::HashMap;

use helion_ir::{
    extract_carry4_chains_from_luts, intern, pack_muxf7_muxf8_trees, share_resource_muxed_alu,
    CellKind, Design, PortDir, Spur, FA_CARRY_INIT, FA_SUM_INIT, MUX2_INIT, SUB_CARRY_INIT,
    XOR2_INIT,
};

/// Evaluate a `Design` containing `Lut6` (including `Muxf7`/`Muxf8`) and `Carry4` cells.
fn eval_datapath(d: &Design, inputs: &HashMap<Spur, bool>) -> HashMap<Spur, bool> {
    let mut vals = inputs.clone();
    vals.insert(intern("0"), false);
    vals.insert(intern("1"), true);
    let i_pins = [
        intern("I0"),
        intern("I1"),
        intern("I2"),
        intern("I3"),
        intern("I4"),
        intern("I5"),
    ];
    let o_pin = intern("O");
    let max_rounds = d.cells.len() + 4;

    for _ in 0..max_rounds {
        let mut changed = false;
        for c in &d.cells {
            match c.kind {
                CellKind::Lut6 { init } => {
                    let Some(out_net) = d.net_on(c.name, o_pin) else {
                        continue;
                    };
                    let mut addr = 0u64;
                    for (idx, &p_sp) in i_pins.iter().enumerate() {
                        if let Some(in_net) = d.net_on(c.name, p_sp) {
                            if vals.get(&in_net).copied().unwrap_or(false) {
                                addr |= 1u64 << idx;
                            }
                        }
                    }
                    let v = ((init >> addr) & 1) != 0;
                    if vals.insert(out_net, v) != Some(v) {
                        changed = true;
                    }
                }
                CellKind::Carry4 => {
                    let ci = d
                        .net_on(c.name, intern("CI"))
                        .and_then(|n| vals.get(&n).copied())
                        .unwrap_or(false);
                    let cyinit = d
                        .net_on(c.name, intern("CYINIT"))
                        .and_then(|n| vals.get(&n).copied())
                        .unwrap_or(false);
                    let mut cin = ci || cyinit;
                    for lane in 0..4 {
                        let s = d
                            .net_on(c.name, intern(&format!("S{lane}")))
                            .and_then(|n| vals.get(&n).copied())
                            .unwrap_or(false);
                        let di = d
                            .net_on(c.name, intern(&format!("DI{lane}")))
                            .and_then(|n| vals.get(&n).copied())
                            .unwrap_or(false);
                        let sum = s ^ cin;
                        let co = if s { cin } else { di };
                        if let Some(o_net) = d.net_on(c.name, intern(&format!("O{lane}"))) {
                            if vals.insert(o_net, sum) != Some(sum) {
                                changed = true;
                            }
                        }
                        if let Some(co_net) = d.net_on(c.name, intern(&format!("CO{lane}"))) {
                            if vals.insert(co_net, co) != Some(co) {
                                changed = true;
                            }
                        }
                        cin = co;
                    }
                }
                _ => {}
            }
        }
        if !changed {
            break;
        }
    }
    vals
}

#[test]
fn extract_carry4_chains_from_luts_promotes_8bit_ripple_adder_and_4bit_subtractor() {
    // 1. 8-bit ripple-carry adder built from 8 full-adder sum LUTs (`0x96`) + 8 carry LUTs (`0xE8`).
    let mut d = Design::new("ripple8");
    d.add_port("cin", PortDir::In);
    for i in 0..8 {
        d.add_port(&format!("a{i}"), PortDir::In);
        d.add_port(&format!("b{i}"), PortDir::In);
        d.add_port(&format!("sum{i}"), PortDir::Out);
    }
    d.add_port("cout", PortDir::Out);

    for i in 0..8 {
        let cin_net = if i == 0 {
            "cin".to_string()
        } else {
            format!("c{i}")
        };
        let cout_net = if i == 7 {
            "cout".to_string()
        } else {
            format!("c{}", i + 1)
        };
        let sum_lut = format!("u_sum{i}");
        let car_lut = format!("u_car{i}");

        d.add_cell(&sum_lut, CellKind::Lut6 { init: FA_SUM_INIT });
        d.connect(&format!("a{i}"), &sum_lut, "I0");
        d.connect(&format!("b{i}"), &sum_lut, "I1");
        d.connect(&cin_net, &sum_lut, "I2");
        d.connect(&format!("sum{i}"), &sum_lut, "O");

        d.add_cell(&car_lut, CellKind::Lut6 { init: FA_CARRY_INIT });
        d.connect(&format!("a{i}"), &car_lut, "I0");
        d.connect(&format!("b{i}"), &car_lut, "I1");
        d.connect(&cin_net, &car_lut, "I2");
        d.connect(&cout_net, &car_lut, "O");
    }

    let st = extract_carry4_chains_from_luts(&mut d);
    assert_eq!(st.chains_extracted, 1);
    assert_eq!(st.bits_promoted, 8);
    assert_eq!(st.carry4_cells_created, 2, "8 bits require 2 chained Carry4 cells");
    assert_eq!(
        d.cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Carry4))
            .count(),
        2
    );

    // Verify exact 8-bit arithmetic `a + b + cin` across test vectors.
    for (a_val, b_val, cin_val) in [
        (0u16, 0u16, false),
        (255, 1, false),
        (173, 94, true),
        (0x55, 0xAA, false),
        (0x55, 0xAA, true),
        (127, 128, true),
    ] {
        let mut env = HashMap::new();
        env.insert(intern("cin"), cin_val);
        for i in 0..8 {
            env.insert(intern(&format!("a{i}")), ((a_val >> i) & 1) != 0);
            env.insert(intern(&format!("b{i}")), ((b_val >> i) & 1) != 0);
        }
        let out = eval_datapath(&d, &env);
        let expected = a_val + b_val + u16::from(cin_val);
        for i in 0..8 {
            assert_eq!(
                out[&intern(&format!("sum{i}"))],
                ((expected >> i) & 1) != 0,
                "sum{i} mismatch for {a_val} + {b_val} + {cin_val}"
            );
        }
        assert_eq!(out[&intern("cout")], ((expected >> 8) & 1) != 0);
    }

    // 2. 4-bit ripple-carry subtractor (`a + ~b + 1` with `SUB_CARRY_INIT` and `~FA_SUM_INIT`).
    let mut d_sub = Design::new("sub4");
    d_sub.add_port("cin", PortDir::In);
    for i in 0..4 {
        d_sub.add_port(&format!("a{i}"), PortDir::In);
        d_sub.add_port(&format!("b{i}"), PortDir::In);
        d_sub.add_port(&format!("diff{i}"), PortDir::Out);
    }
    for i in 0..4 {
        let cin_net = if i == 0 {
            "cin".to_string()
        } else {
            format!("c{i}")
        };
        let cout_net = format!("c{}", i + 1);
        d_sub.add_cell(
            &format!("u_sum{i}"),
            CellKind::Lut6 {
                init: !FA_SUM_INIT,
            },
        );
        d_sub.connect(&format!("a{i}"), &format!("u_sum{i}"), "I0");
        d_sub.connect(&format!("b{i}"), &format!("u_sum{i}"), "I1");
        d_sub.connect(&cin_net, &format!("u_sum{i}"), "I2");
        d_sub.connect(&format!("diff{i}"), &format!("u_sum{i}"), "O");

        d_sub.add_cell(
            &format!("u_car{i}"),
            CellKind::Lut6 {
                init: SUB_CARRY_INIT,
            },
        );
        d_sub.connect(&format!("a{i}"), &format!("u_car{i}"), "I0");
        d_sub.connect(&format!("b{i}"), &format!("u_car{i}"), "I1");
        d_sub.connect(&cin_net, &format!("u_car{i}"), "I2");
        d_sub.connect(&cout_net, &format!("u_car{i}"), "O");
    }

    let st_sub = d_sub.extract_carry4_chains_from_luts();
    assert_eq!(st_sub.chains_extracted, 1);
    assert_eq!(st_sub.carry4_cells_created, 1);
    for a_val in 0..16u8 {
        for b_val in 0..16u8 {
            let mut env = HashMap::new();
            env.insert(intern("cin"), true); // +1 for two's complement `a - b`
            for i in 0..4 {
                env.insert(intern(&format!("a{i}")), ((a_val >> i) & 1) != 0);
                env.insert(intern(&format!("b{i}")), ((b_val >> i) & 1) != 0);
            }
            let out = eval_datapath(&d_sub, &env);
            let diff = a_val.wrapping_sub(b_val) & 0xF;
            for i in 0..4 {
                assert_eq!(out[&intern(&format!("diff{i}"))], ((diff >> i) & 1) != 0);
            }
        }
    }
}

#[test]
fn pack_muxf7_muxf8_trees_promotes_2to1_cascade_and_4to1_lut6() {
    let mut d = Design::new("muxpack");
    for p in ["x0", "x1", "s0", "s1"] {
        d.add_port(p, PortDir::In);
    }
    d.add_port("y_cascade", PortDir::Out);
    d.add_port("y_mux4", PortDir::Out);

    // 4 leaf LUT6 cones:
    let inits = [
        0x6666_6666_6666_6666u64, // x0 ^ x1
        0x8888_8888_8888_8888u64, // x0 & x1
        0xEEEE_EEEE_EEEE_EEEEu64, // x0 | x1
        0x9999_9999_9999_9999u64, // ~(x0 ^ x1)
    ];
    for (k, &init) in inits.iter().enumerate() {
        let cname = format!("leaf{k}");
        d.add_cell(&cname, CellKind::Lut6 { init });
        d.connect("x0", &cname, "I0");
        d.connect("x1", &cname, "I1");
        d.connect(&format!("n_leaf{k}"), &cname, "O");
    }

    // Tree 1: two 2:1 LUT6 muxes feeding a root 2:1 LUT6 mux -> should become 2 MUXF7 + 1 MUXF8!
    d.add_cell("m_lo", CellKind::Lut6 { init: MUX2_INIT });
    d.connect("n_leaf0", "m_lo", "I0");
    d.connect("n_leaf1", "m_lo", "I1");
    d.connect("s0", "m_lo", "I2");
    d.connect("n_m_lo", "m_lo", "O");

    d.add_cell("m_hi", CellKind::Lut6 { init: MUX2_INIT });
    d.connect("n_leaf2", "m_hi", "I0");
    d.connect("n_leaf3", "m_hi", "I1");
    d.connect("s0", "m_hi", "I2");
    d.connect("n_m_hi", "m_hi", "O");

    d.add_cell("m_root", CellKind::Lut6 { init: MUX2_INIT });
    d.connect("n_m_lo", "m_root", "I0");
    d.connect("n_m_hi", "m_root", "I1");
    d.connect("s1", "m_root", "I2");
    d.connect("y_cascade", "m_root", "O");

    // Tree 2: single 6-input LUT6 implementing a 4:1 mux (`I4=s0`, `I5=s1`, `I0..I3 = n_leaf0..3`)
    let mut mux4_init = 0u64;
    for addr in 0..64u64 {
        let sel = ((addr >> 4) & 3) as usize;
        let bit = (addr >> sel) & 1;
        mux4_init |= bit << addr;
    }
    d.add_cell("u_mux4", CellKind::Lut6 { init: mux4_init });
    for k in 0..4 {
        d.connect(&format!("n_leaf{k}"), "u_mux4", &format!("I{k}"));
    }
    d.connect("s0", "u_mux4", "I4");
    d.connect("s1", "u_mux4", "I5");
    d.connect("y_mux4", "u_mux4", "O");

    // Record truth table before packing:
    let mut before = Vec::new();
    for mask in 0..16usize {
        let mut env = HashMap::new();
        env.insert(intern("x0"), (mask & 1) != 0);
        env.insert(intern("x1"), ((mask >> 1) & 1) != 0);
        env.insert(intern("s0"), ((mask >> 2) & 1) != 0);
        env.insert(intern("s1"), ((mask >> 3) & 1) != 0);
        let res = eval_datapath(&d, &env);
        before.push((res[&intern("y_cascade")], res[&intern("y_mux4")]));
    }

    let st = pack_muxf7_muxf8_trees(&mut d);
    assert_eq!(st.muxf7_packed, 4, "2 from cascade + 2 from 4:1 LUT6 decomposition");
    assert_eq!(st.muxf8_packed, 2, "1 from cascade + 1 from 4:1 LUT6 decomposition");
    assert_eq!(d.cell("m_lo").unwrap().attrs.get("PRIM"), Some("MUXF7"));
    assert_eq!(d.cell("m_hi").unwrap().attrs.get("PRIM"), Some("MUXF7"));
    assert_eq!(d.cell("m_root").unwrap().attrs.get("PRIM"), Some("MUXF8"));

    for (mask, expected) in before.into_iter().enumerate() {
        let mut env = HashMap::new();
        env.insert(intern("x0"), (mask & 1) != 0);
        env.insert(intern("x1"), ((mask >> 1) & 1) != 0);
        env.insert(intern("s0"), ((mask >> 2) & 1) != 0);
        env.insert(intern("s1"), ((mask >> 3) & 1) != 0);
        let res = eval_datapath(&d, &env);
        assert_eq!((res[&intern("y_cascade")], res[&intern("y_mux4")]), expected);
    }
}

#[test]
fn share_resource_muxed_alu_folds_dual_carry4_branches_into_single_shared_carry4() {
    // `y = sel ? (a + b) : (c + d)` with two 4-bit `Carry4` adders (`c4_0` for `c + d`, `c4_1` for `a + b`)
    let mut d = Design::new("muxed_alu");
    d.add_port("sel", PortDir::In);
    for i in 0..4 {
        for p in ["a", "b", "c", "d"] {
            d.add_port(&format!("{p}{i}"), PortDir::In);
        }
        d.add_port(&format!("y{i}"), PortDir::Out);
    }

    d.add_cell("c4_0", CellKind::Carry4);
    d.connect("0", "c4_0", "CYINIT");
    d.add_cell("c4_1", CellKind::Carry4);
    d.connect("0", "c4_1", "CYINIT");

    for i in 0..4 {
        // Branch 0 (`c + d`):
        let p0 = format!("prop0_{i}");
        let s0 = format!("s0_{i}");
        let sum0 = format!("sum0_{i}");
        d.add_cell(&p0, CellKind::Lut6 { init: XOR2_INIT });
        d.connect(&format!("c{i}"), &p0, "I0");
        d.connect(&format!("d{i}"), &p0, "I1");
        d.connect(&s0, &p0, "O");
        d.connect(&s0, "c4_0", &format!("S{i}"));
        d.connect(&format!("c{i}"), "c4_0", &format!("DI{i}"));
        d.connect(&sum0, "c4_0", &format!("O{i}"));

        // Branch 1 (`a + b`):
        let p1 = format!("prop1_{i}");
        let s1 = format!("s1_{i}");
        let sum1 = format!("sum1_{i}");
        d.add_cell(&p1, CellKind::Lut6 { init: XOR2_INIT });
        d.connect(&format!("a{i}"), &p1, "I0");
        d.connect(&format!("b{i}"), &p1, "I1");
        d.connect(&s1, &p1, "O");
        d.connect(&s1, "c4_1", &format!("S{i}"));
        d.connect(&format!("a{i}"), "c4_1", &format!("DI{i}"));
        d.connect(&sum1, "c4_1", &format!("O{i}"));

        // Output 2:1 mux `y{i} = sel ? sum1_{i} : sum0_{i}`:
        let omux = format!("out_mux_{i}");
        d.add_cell(&omux, CellKind::Lut6 { init: MUX2_INIT });
        d.connect(&sum0, &omux, "I0");
        d.connect(&sum1, &omux, "I1");
        d.connect("sel", &omux, "I2");
        d.connect(&format!("y{i}"), &omux, "O");
    }

    assert_eq!(
        d.cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Carry4))
            .count(),
        2
    );

    let st = share_resource_muxed_alu(&mut d);
    assert_eq!(st.alus_shared, 1);
    assert_eq!(st.carry4_eliminated, 1);
    assert_eq!(
        d.cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Carry4))
            .count(),
        1,
        "one of the two Carry4 chains must be eliminated by resource sharing"
    );

    // Exhaustively verify `sel ? (a + b) : (c + d)` across representative 4-bit operand combinations.
    for sel in [false, true] {
        for a_val in [0u8, 3, 7, 12, 15] {
            for b_val in [0u8, 5, 9, 14] {
                for c_val in [1u8, 6, 11, 15] {
                    for d_val in [2u8, 4, 8, 13] {
                        let mut env = HashMap::new();
                        env.insert(intern("sel"), sel);
                        for i in 0..4 {
                            env.insert(intern(&format!("a{i}")), ((a_val >> i) & 1) != 0);
                            env.insert(intern(&format!("b{i}")), ((b_val >> i) & 1) != 0);
                            env.insert(intern(&format!("c{i}")), ((c_val >> i) & 1) != 0);
                            env.insert(intern(&format!("d{i}")), ((d_val >> i) & 1) != 0);
                        }
                        let res = eval_datapath(&d, &env);
                        let expected = if sel {
                            a_val.wrapping_add(b_val) & 0xF
                        } else {
                            c_val.wrapping_add(d_val) & 0xF
                        };
                        for i in 0..4 {
                            assert_eq!(
                                res[&intern(&format!("y{i}"))],
                                ((expected >> i) & 1) != 0
                            );
                        }
                    }
                }
            }
        }
    }
}
