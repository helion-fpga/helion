//! A pre-existing defect surfaced while auditing the frontend: the blocking
//! assignment fold keys whole-word assignments as `(lhs, None)` but a later
//! read of one bit arrives as `RExpr::Bit(lhs, n)`, which the rewriter has no
//! arm for. The bit reference therefore survives unsubstituted and the emitted
//! logic reads the *port* rather than the value the block computed.
//!
//! This test states the correct function; it is expected to fail until the fold
//! handles bit-level reads of a whole-word assignment.

mod common;
use common::Netlist;
use helion_sv::synth_sv;

#[test]
fn blocking_assign_is_visible_to_a_later_bit_read() {
    // `t` is assigned as a whole word, then bit 0 of it is read. The result
    // must be `(a & b) & 1`, i.e. `a & b`.
    let d = synth_sv(
        "module m(input logic a, input logic b, output logic y);\n\
             logic t;\n\
             always_comb begin\n\
                 t = a & b;\n\
                 y = t[0];\n\
             end\n\
         endmodule",
        "t.sv",
    )
    .expect("synth");
    for a in 0u64..2 {
        for b in 0u64..2 {
            let got = Netlist::drive(&d, &[("a", a, 1), ("b", b, 1)]).out("y", 1);
            assert_eq!(got, a & b, "t = a & b; y = t[0]  (a={a} b={b})");
        }
    }
}

#[test]
fn blocking_assign_is_visible_to_a_later_vector_bit_read() {
    // Same shape with a multi-bit word and a non-zero bit index.
    let d = synth_sv(
        "module m(input logic [3:0] a, output logic y);\n\
             logic [3:0] t;\n\
             always_comb begin\n\
                 t = a ^ 4'hF;\n\
                 y = t[2];\n\
             end\n\
         endmodule",
        "t.sv",
    )
    .expect("synth");
    for a in 0u64..16 {
        let got = Netlist::drive(&d, &[("a", a, 4)]).out("y", 1);
        assert_eq!(got, ((a ^ 0xF) >> 2) & 1, "t = a ^ 4'hF; y = t[2]  (a={a})");
    }
}

/// Harder shapes: the fold must stay correct when the bit read is separated
/// from the whole-word assignment by another statement, and when a word is
/// built bit by bit.
#[test]
fn bit_read_survives_an_intervening_statement() {
    let d = synth_sv(
        "module m(input logic [1:0] a, input logic [1:0] b, output logic y);\n\
             logic [1:0] t; logic [1:0] u;\n\
             always_comb begin t = a & b; u = ~t; y = u[0]; end\n\
         endmodule",
        "t.sv",
    )
    .expect("synth");
    for a in 0u64..4 {
        for b in 0u64..4 {
            let got = Netlist::drive(&d, &[("a", a, 2), ("b", b, 2)]).out("y", 1);
            assert_eq!(got, (!(a & b)) & 1, "t=a&b; u=~t; y=u[0] (a={a} b={b})");
        }
    }
}

#[test]
fn word_built_from_bits_reads_back() {
    let d = synth_sv(
        "module m(input logic [1:0] a, input logic [1:0] b, output logic y);\n\
             logic [1:0] t;\n\
             always_comb begin t[0] = a[0]; t[1] = b[0]; y = t[1]; end\n\
         endmodule",
        "t.sv",
    )
    .expect("synth");
    for a in 0u64..2 {
        for b in 0u64..2 {
            let got = Netlist::drive(&d, &[("a", a, 1), ("b", b, 1)]).out("y", 1);
            assert_eq!(got, b & 1, "t[1]=b[0]; y=t[1] (b={b})");
        }
    }
}

/// Diagnostic: dump the emitted structure for the failing shape.
#[test]
fn dump_word_built_from_bits() {
    let d = synth_sv(
        "module m(input logic [1:0] a, input logic [1:0] b, output logic y);\n\
             logic [1:0] t;\n\
             always_comb begin t[0] = a[0]; t[1] = b[0]; y = t[1]; end\n\
         endmodule",
        "t.sv",
    )
    .expect("synth");
    println!("== cells ==");
    for c in &d.cells {
        println!("  {} {:?}", c.name, c.kind);
    }
    println!("== nets ==");
    for n in &d.nets {
        let eps: Vec<String> = n
            .endpoints
            .iter()
            .map(|e| format!("{}.{}", e.cell, e.pin))
            .collect();
        println!("  {} -> {}", n.name, eps.join(", "));
    }
}
