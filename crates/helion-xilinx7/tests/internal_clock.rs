//! A clock produced inside the design must be buffered, not refused.
//!
//! `insert_clock_spine` only handled a clock that *is* a top-level input, so a
//! clock derived from logic was rejected with "clock net `gated` has no matching
//! top-level input port" -- a message describing Helion's internals rather than
//! the design. `examples/timing_lut_as_clock.sv` (a LUT driving an FF CLK) is a
//! legitimate, if poor-practice, design that Vivado accepts.
//!
//! An internal clock needs no IBUF: the net is already a fabric signal, so it
//! drives the BUFG directly.

fn db_root() -> std::path::PathBuf {
    std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../work/prjxray-db")
        })
}

const LUT_AS_CLOCK: &str = "module top(input logic clk, input logic en, \
     input logic d, output logic q);\n\
     logic gated;\n\
     assign gated = clk ^ en;\n\
     always_ff @(posedge gated) q <= d;\n\
   endmodule";

#[test]
fn internal_clock_gets_a_bufg() {
    let design = helion_sv::synth_sv(LUT_AS_CLOCK, "t.sv").unwrap();
    let packed = helion_xilinx7::pack::pack(&design).expect("internal clock must pack");
    // The XOR LUT produces `gated`; that net is the clock.
    let bufg = packed
        .bufgs
        .iter()
        .find(|b| b.in_net == "gated")
        .unwrap_or_else(|| {
            panic!(
                "no BUFG on the internal clock `gated`; buffers: {:?}",
                packed
                    .bufgs
                    .iter()
                    .map(|b| (&b.in_net, &b.out_net))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(bufg.out_net, "gated_bufg");
    // No IBUF: `gated` is not a port, so there is no pad to bring it in from.
    assert!(
        !packed.iobs.iter().any(|i| i.net == "gated"),
        "an internal clock must not get an input buffer"
    );
    // The FFs must have been moved onto the buffered net.
    assert!(
        packed.ffs.iter().any(|f| f.clk_net == "gated_bufg"),
        "FFs must clock off the BUFG output, got {:?}",
        packed.ffs.iter().map(|f| &f.clk_net).collect::<Vec<_>>()
    );
}

#[test]
fn internal_clock_reaches_a_bitstream() {
    // The whole flow, because packing alone does not prove the clock routes.
    let mut design = helion_sv::synth_sv(LUT_AS_CLOCK, "t.sv").unwrap();
    let xdc = "set_property PACKAGE_PIN Y9 [get_ports {clk}]\n\
               set_property PACKAGE_PIN P16 [get_ports {en}]\n\
               set_property PACKAGE_PIN R16 [get_ports {d}]\n\
               set_property PACKAGE_PIN T22 [get_ports {q}]\n";
    helion_xilinx7::build::PinConstraints::parse(xdc).apply(&mut design);
    let bits = helion_xilinx7::build::bitstream_for_part(&design, "xc7z020", Some(&db_root()))
        .expect("internal-clock design must produce a bitstream");
    assert_eq!(bits.part, "xc7z020clg484-1");
    assert_eq!(bits.idcode, 0x0372_7093);
    assert_eq!(bits.frame_count, 9996);
    assert!(bits.features_applied > 0);
}

#[test]
fn clock_port_still_works_alongside_an_internal_one() {
    // Two different clocks must get two different buffers and no collision.
    let design = helion_sv::synth_sv(
        "module top(input logic clk, input logic en, input logic d, output logic q, \
                    output logic r);\n\
           logic gated;\n\
           assign gated = clk ^ en;\n\
           always_ff @(posedge gated) q <= d;\n\
           always_ff @(posedge clk) r <= ~d;\n\
         endmodule",
        "t.sv",
    )
    .unwrap();
    let packed = helion_xilinx7::pack::pack(&design).expect("both clocks must pack");
    let ins: Vec<&str> = packed.bufgs.iter().map(|b| b.in_net.as_str()).collect();
    assert!(ins.contains(&"clk_ibuf"), "port clock needs an IBUF+BUFG: {ins:?}");
    assert!(ins.contains(&"gated"), "internal clock needs a BUFG: {ins:?}");
    assert_eq!(packed.bufgs.len(), 2, "two clocks, two buffers: {ins:?}");
}
