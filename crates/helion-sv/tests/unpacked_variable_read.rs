//! Variable-index READ of a small unpacked array.
//!
//! The write side of a small array lowers to one flop per bit
//! (`<mem>_w<word>_<bit>`), but a read like
//!
//! ```text
//! always_ff @(posedge clk) if (we) mem[addr] <= din;
//! assign dout = mem[addr];            // <- variable index, combinational
//! ```
//!
//! was skipped outright: the assign loop bails on anything
//! `rhs_unpacked_index` reports, so no read logic existed at all.
//!
//! That was not a cosmetic gap. The array itself lowered fine -- the flops
//! existed and writes landed -- so the design looked almost built and then
//! failed at packing with
//!
//! ```text
//! run: 8 output pad(s) have no driver after packing and cannot be placed:
//!      dout_0 .. dout_7
//! ```
//!
//! which points at the IO and not at the memory. A perfectly ordinary
//! register file (depth 16, width 8 -- inside the documented depth<=16,
//! width<=32, depth*width<=128 caps) could not be built at all.
//!
//! The clocked form (`dout <= mem[addr]`) already worked, which is why this
//! went unnoticed: only the combinational read was dead.

use helion_ir::CellKind;

fn count_ffs(d: &helion_ir::Design) -> usize {
    d.cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count()
}

/// True when the top-level output PORT bit `port` reaches a net that a
/// LUT/FF actually drives.
///
/// Neither obvious test works on its own, and both traps were hit here:
///
///  - `endpoints.is_empty()` is always false for a top-level output, because
///    PORT BINDING puts an endpoint on it even when nothing drives it. That
///    is how the defect hid: the net reported two endpoints while the design
///    had no read logic at all.
///  - The net carrying `dout_i` is not always named `dout_i`. A constant word
///    index ALIASES the output straight onto `<mem>_w3_0`, so looking only for
///    a net called `dout_i` finds the pad side and nothing else.
///
/// So follow the wire: find the IOB whose PAD endpoint sits on `port`, then
/// ask whether the net on that IOB's `I` pin has an LUT/FF endpoint. This
/// mirrors the packer, whose test is membership in the packed LUT/FF Q set
/// (`crates/helion-cli/src/main.rs`).
fn is_driven(d: &helion_ir::Design, port: &str) -> bool {
    // The IOB cell that drives this port's pad.
    let pad_cell = d.nets.iter().find(|n| n.name == port).and_then(|n| {
        n.endpoints
            .iter()
            .find(|e| e.pin == "PAD")
            .map(|e| e.cell.clone())
    });
    let Some(pad_cell) = pad_cell else {
        return false;
    };
    // The net feeding that IOB's data input.
    let driver_net = d.nets.iter().find(|n| {
        n.endpoints
            .iter()
            .any(|e| e.cell == pad_cell && e.pin == "I")
    });
    let Some(driver_net) = driver_net else {
        return false;
    };
    driver_net.endpoints.iter().any(|e| {
        d.cells
            .iter()
            .find(|c| c.name == e.cell)
            .map(|c| matches!(c.kind, CellKind::Lut6 { .. } | CellKind::Hff))
            .unwrap_or(false)
    })
}

/// The exact design that failed: a synthesizable register file with a
/// combinational read port.
#[test]
fn variable_index_read_lowers() {
    let src = r#"
module regmem (input logic clk, input logic we, input logic [3:0] addr,
               input logic [7:0] din, output logic [7:0] dout);
    logic [7:0] mem [0:15];
    always_ff @(posedge clk) if (we) mem[addr] <= din;
    assign dout = mem[addr];
endmodule
"#;
    let d = helion_sv::synth_sv(src, "regmem.sv").expect("variable-index read must lower");
    // 16 words x 8 bits of storage.
    assert!(
        count_ffs(&d) >= 128,
        "expected >=128 Hff for a 16x8 array, got {}",
        count_ffs(&d)
    );
    // Every dout bit must be driven. Before the fix these were all absent,
    // which is what made packing report "output pad(s) have no driver".
    for i in 0..8 {
        let net = format!("dout_{i}");
        assert!(
            is_driven(&d, &net),
            "net {net} has no driving LUT/FF; the read port did not lower"
        );
    }
    assert_eq!(d.attrs.get("BRAM_NOT_LOWERED"), None);
}

/// The depth-1 array is the degenerate case: one word, no address bits.
#[test]
fn single_word_read_lowers() {
    let src = r#"
module one (input logic clk, input logic we, input logic [0:0] addr,
            input logic [3:0] din, output logic [3:0] dout);
    logic [3:0] mem [0:0];
    always_ff @(posedge clk) if (we) mem[0] <= din;
    assign dout = mem[addr];
endmodule
"#;
    let d = helion_sv::synth_sv(src, "one.sv").expect("depth-1 read must lower");
    for i in 0..4 {
        let net = format!("dout_{i}");
        assert!(is_driven(&d, &net), "net {net} has no driving LUT/FF");
    }
}

/// The clocked read must keep working -- this is the case that already did,
/// and it is the reason the gap survived.
#[test]
fn clocked_read_still_lowers() {
    let src = r#"
module regmem2 (input logic clk, input logic we, input logic [3:0] addr,
                input logic [7:0] din, output logic [7:0] dout);
    logic [7:0] mem [0:15];
    always_ff @(posedge clk) begin
        if (we) mem[addr] <= din;
        dout <= mem[addr];
    end
endmodule
"#;
    let d = helion_sv::synth_sv(src, "regmem2.sv").expect("clocked read must lower");
    assert!(count_ffs(&d) >= 128, "cells={}", d.cells.len());
}

/// A constant index must keep working: it aliases a word, it is not a mux.
#[test]
fn constant_index_still_aliases() {
    let src = r#"
module cm (input logic clk, input logic we, input logic [3:0] addr,
           input logic [7:0] din, output logic [7:0] dout);
    logic [7:0] mem [0:15];
    always_ff @(posedge clk) if (we) mem[addr] <= din;
    assign dout = mem[3];
endmodule
"#;
    let d = helion_sv::synth_sv(src, "cm.sv").expect("constant read must lower");
    for i in 0..8 {
        let net = format!("dout_{i}");
        assert!(is_driven(&d, &net), "net {net} has no driving LUT/FF");
    }
}

/// A memory beyond the register-file caps must still be REFUSED loudly.
/// Lowering the read must not become a way to accept an array that has no
/// storage at all.
#[test]
fn oversized_memory_is_still_refused() {
    let src = r#"
module big (input logic clk, input logic we, input logic [5:0] addr,
            input logic [7:0] din, output logic [7:0] dout);
    logic [7:0] mem [0:32];
    always_ff @(posedge clk) if (we) mem[addr] <= din;
    assign dout = mem[addr];
endmodule
"#;
    let err = helion_sv::synth_sv(src, "big.sv")
        .expect_err("depth=33 exceeds the register-file path and has no BRAM lowering");
    assert!(
        err.contains("too large for the register-file path"),
        "unexpected error: {err}"
    );
}

// ---------------------------------------------------------------------------
// The read must return the RIGHT WORD, not merely some driver.
//
// `variable_index_read_lowers` above only proves a LUT/FF reaches `dout`. It
// passed while the mux was returning the wrong value, because a mux built with
// a missing address guard is still a perfectly well-driven net.
//
// The defect: `unpacked_read_mux_expr` emitted the TOP word bare, with no
// `addr == depth-1` selector, so it became an extra OR term of every other
// word. For depth 2 that is
//
//     mem1 | (mem0 & ~addr)      instead of      addr ? mem1 : mem0
//
// which is wrong exactly when the top word holds a 1 and a lower word is
// selected. Every word needs its guard.
// ---------------------------------------------------------------------------

/// Evaluate a LUT6 INIT over the given pin values.
fn lut6(init: u64, pins: &[bool]) -> bool {
    let mut idx = 0usize;
    for (i, v) in pins.iter().enumerate().take(6) {
        if *v {
            idx |= 1 << i;
        }
    }
    (init >> idx) & 1 == 1
}

/// Settle the combinational cells to a fixpoint, forcing the storage flops and
/// the address, then read the value on each output register's `D`.
///
/// Combinational only: a flop's Q never changes here, which is why the D net is
/// read rather than the output port.
fn read_word(design: &helion_ir::Design, table: &[u8], addr: usize) -> u8 {
    use std::collections::HashMap;
    let mut val: HashMap<String, bool> = HashMap::new();
    for (w, byte) in table.iter().enumerate() {
        for b in 0..8 {
            val.insert(format!("mem_w{w}_{b}"), (byte >> b) & 1 == 1);
        }
    }
    // A one-bit address is the scalar net `addr`; wider ones are `addr_<b>`.
    if design.nets.iter().any(|n| n.name == "addr") {
        val.insert("addr".to_string(), addr & 1 == 1);
    } else {
        for b in 0..6 {
            val.insert(format!("addr_{b}"), (addr >> b) & 1 == 1);
        }
    }

    let ins = |c: &str| -> Vec<(usize, String)> {
        let mut v: Vec<(usize, String)> = design
            .nets
            .iter()
            .flat_map(|n| {
                n.endpoints.iter().filter_map(move |e| {
                    if e.cell != c || e.pin == "O" {
                        return None;
                    }
                    let i = e.pin.strip_prefix('I')?.parse::<usize>().ok()?;
                    Some((i, n.name.clone()))
                })
            })
            .collect();
        v.sort();
        v
    };
    let out = |c: &str| -> Option<String> {
        design.nets.iter().find_map(|n| {
            n.endpoints
                .iter()
                .any(|e| e.cell == c && e.pin == "O")
                .then(|| n.name.clone())
        })
    };

    let luts: Vec<&helion_ir::Cell> = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .collect();
    for _ in 0..900 {
        let mut changed = false;
        for c in &luts {
            let CellKind::Lut6 { init } = c.kind else {
                continue;
            };
            let iv: Vec<bool> = ins(&c.name)
                .into_iter()
                .map(|(_, n)| *val.get(&n).unwrap_or(&false))
                .collect();
            let o = lut6(init, &iv);
            if let Some(net) = out(&c.name) {
                if val.get(&net).copied() != Some(o) {
                    val.insert(net, o);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut word = 0u8;
    for b in 0..8 {
        let q = format!("led_{b}");
        let Some(ff) = design.nets.iter().find(|n| n.name == q).and_then(|n| {
            n.endpoints
                .iter()
                .find(|e| e.pin == "Q")
                .map(|e| e.cell.clone())
        }) else {
            continue;
        };
        let d = design.nets.iter().find(|n| {
            n.endpoints
                .iter()
                .any(|e| e.cell == ff && e.pin == "D")
        });
        if d.is_some_and(|d| *val.get(&d.name).unwrap_or(&false)) {
            word |= 1 << b;
        }
    }
    word
}

fn check_read_table(depth: usize, addr_w: usize) {
    let src = format!(
        r#"
module regmem (input logic clk, input logic we, input logic [{}:0] addr,
               input logic [7:0] din, output logic [7:0] led);
    logic [7:0] mem [0:{}];
    always_ff @(posedge clk) if (we) mem[addr] <= din;
    assign led = mem[addr];
endmodule
"#,
        addr_w - 1,
        depth - 1
    );
    let d = helion_sv::synth_sv(&src, "regmem.sv").expect("read must lower");

    // The missing top-word guard makes the TOP word an unconditional OR term
    // of every other word, so it only shows when the top word holds a 1 that
    // a lower-addressed word must mask off. Set the top word to all ones and
    // word 0 to all zeros: a leaking top word then makes addr=0 read 0xFF.
    let mut table: Vec<u8> = (0..depth)
        .map(|w| ((w * 37 + 11) & 0xFF) as u8)
        .collect();
    table[0] = 0x00;
    table[depth - 1] = 0xFF;
    assert_ne!(table[0], table[depth - 1]);

    let wrong: Vec<(usize, u8, u8)> = (0..depth)
        .map(|a| (a, table[a], read_word(&d, &table, a)))
        .filter(|(_, want, got)| want != got)
        .collect();
    assert!(
        wrong.is_empty(),
        "depth {depth}: wrong words (addr, expect, got) = {wrong:?}"
    );
}

/// Depth 2 is the minimal case that shows the missing top-word guard:
/// `addr=1, mem0=1, mem1=0` used to read 1.
#[test]
fn read_returns_the_right_word_depth_2() {
    check_read_table(2, 1);
}

#[test]
fn read_returns_the_right_word_depth_4() {
    check_read_table(4, 2);
}

#[test]
fn read_returns_the_right_word_depth_8() {
    check_read_table(8, 3);
}

#[test]
fn read_returns_the_right_word_depth_16() {
    check_read_table(16, 4);
}

