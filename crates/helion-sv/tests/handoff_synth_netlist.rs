use std::path::PathBuf;

use helion_device::Device;
use helion_ir::{intern, resolve, CellKind, Design};
use helion_pack::pack;
use helion_sv::{synth_sv, synth_sv_path, synth_sv_sources};

const BUF_LUT6_INIT: u64 = 0xaaaa_aaaa_aaaa_aaaa;

fn is_driver_pin(pin: &str) -> bool {
    matches!(
        pin,
        "O" | "Q"
            | "PAD"
            | "O0"
            | "O1"
            | "O2"
            | "O3"
            | "CO0"
            | "CO1"
            | "CO2"
            | "CO3"
            | "DOADO"
            | "DOBDO"
            | "P"
    )
}

/// Count non-IOB-passthrough drivers on each net.
/// An `IobOut` whose `I` and `PAD` are both connected to the same net is a pad
/// buffer attached to that net; an `IobOut` whose `I` is on a different net
/// drives `PAD` from that other net and counts as a distinct driver on `PAD`.
fn multi_driven_nets(d: &Design) -> Vec<(String, Vec<String>)> {
    let mut violations = Vec::new();
    for net in &d.nets {
        let net_name = resolve(net.name);
        let mut fabric_drivers = Vec::new();
        let mut cross_pad_drivers = Vec::new();
        for ep in &net.endpoints {
            let pin = resolve(ep.pin);
            if !is_driver_pin(pin) {
                continue;
            }
            if pin == "PAD" {
                let same_net_input = net
                    .endpoints
                    .iter()
                    .any(|other| other.cell == ep.cell && resolve(other.pin) == "I");
                if !same_net_input {
                    cross_pad_drivers.push(format!("{}/{}", resolve(ep.cell), pin));
                }
            } else {
                fabric_drivers.push(format!("{}/{}", resolve(ep.cell), pin));
            }
        }
        if fabric_drivers.len() > 1
            || (!fabric_drivers.is_empty() && !cross_pad_drivers.is_empty())
        {
            let mut all = fabric_drivers;
            all.extend(cross_pad_drivers);
            violations.push((net_name.to_string(), all));
        }
    }
    violations
}

/// Count combinational identity buffer LUT6s (`init == 0xaaaaaaaaaaaaaaaa`).
/// Sequential FF feeder LUTs (`u_lut*` driving `Hff/D` for `PackedLutFf`) are
/// excluded; combinational LUTs (`u_clut*`, etc.) must never be identity buffers.
fn count_identity_buf_luts(d: &Design) -> usize {
    d.cells
        .iter()
        .filter(|c| {
            matches!(c.kind, CellKind::Lut6 { init: BUF_LUT6_INIT })
                && !resolve(c.name).starts_with("u_lut")
        })
        .count()
}

#[test]
fn wire_copy_assigns_alias_nets_without_identity_lut6() {
    for (name, sv) in [
        (
            "copy_from_input",
            r#"
module m(input clk, input a, output y);
  wire pc = a;
  assign y = pc;
endmodule
"#,
        ),
        (
            "copy_chain",
            r#"
module m(input clk, input a, output y);
  wire w = a;
  wire pc = w;
  assign y = pc;
endmodule
"#,
        ),
        (
            "copy_from_reg_bit",
            r#"
module m(input clk, input d, output reg [31:0] r, output y);
  wire pc = r[0];
  assign y = pc;
  always @(posedge clk) r <= {d, r[31:1]};
endmodule
"#,
        ),
        (
            "range_copy_from_reg",
            r#"
module m(input clk, input d, output reg [31:0] r, output y);
  localparam B = 0;
  wire [B:0] pc = r[B:0];
  assign y = pc;
  always @(posedge clk) r <= {d, r[31:1]};
endmodule
"#,
        ),
    ] {
        let d = synth_sv(sv, "wire_copy.sv").expect(name);
        assert!(
            !d.cells.is_empty(),
            "{name}: synthesis must not drop wire copy"
        );
        assert_eq!(
            count_identity_buf_luts(&d),
            0,
            "{name}: wire copy must not emit 0xaaaaaaaaaaaaaaaa buffer LUTs, got {:?}",
            d.lut_inits()
        );
        assert!(
            multi_driven_nets(&d).is_empty(),
            "{name}: must have 0 multi-driven nets, got {:?}",
            multi_driven_nets(&d)
        );
    }
}

#[test]
fn wire_slice_copy_feeds_logic_without_buffer_luts() {
    let sv = r#"
module slice_alu(
    input  wire        clk,
    input  wire [7:0]  a,
    input  wire [7:0]  b,
    output wire [3:0]  y
);
    reg [7:0] r;
    always @(posedge clk) r <= a ^ b;
    wire [3:0] hi = r[7:4];
    wire [3:0] alias_hi = hi;
    assign y = alias_hi & b[3:0];
endmodule
"#;
    let d = synth_sv(sv, "slice_alu.sv").expect("synth slice_alu");
    assert_eq!(
        count_identity_buf_luts(&d),
        0,
        "slice copy must not emit identity LUT6s: {:?}",
        d.lut_inits()
    );
    for bit in 0..4 {
        let r_net = format!("r_{}", 4 + bit);
        let net = d.net(&r_net).unwrap_or_else(|| panic!("missing {r_net}"));
        assert!(
            net.endpoints.iter().any(|e| resolve(e.pin) == "Q"),
            "{r_net} must be driven by FF Q"
        );
        assert!(
            net.endpoints
                .iter()
                .any(|e| resolve(e.pin).starts_with('I')),
            "{r_net} must directly feed downstream AND LUT input via net aliasing"
        );
    }
    assert!(multi_driven_nets(&d).is_empty());
}

#[test]
fn serv_decode_has_no_multi_driven_nets() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/serv/serv_decode.v");
    let d = synth_sv_path(&path).expect("synth serv_decode.v");
    // Only co_shift_op (simplified after MDU=0 const-prop) is a 1-input comb LUT;
    // none of the 35 wire-copy output assigns (o_sh_right = co_sh_right, etc.) emit buffer LUTs.
    assert!(
        count_identity_buf_luts(&d) <= 1,
        "serv_decode wire-copy outputs must not emit buffer LUTs, got {}",
        count_identity_buf_luts(&d)
    );
    let f2 = multi_driven_nets(&d);
    assert!(
        f2.is_empty(),
        "serv_decode must have 0 F2 multi-driven nets, got {f2:?}"
    );
}

#[test]
fn serv_ctrl_and_serv_aligner_have_no_f2_or_wire_copy_buffer_luts() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/serv");
    let d_ctrl = synth_sv_path(&root.join("serv_ctrl.v")).expect("serv_ctrl.v");
    assert_eq!(
        count_identity_buf_luts(&d_ctrl),
        0,
        "serv_ctrl.v wire copies (pc = o_ibus_adr[B:0], o_bad_pc = pc_plus_offset_aligned) must not emit 0xaaaaaaaaaaaaaaaa LUTs"
    );
    let f2_ctrl = multi_driven_nets(&d_ctrl);
    assert!(
        f2_ctrl.is_empty(),
        "serv_ctrl.v must have 0 F2 multi-driven nets, got {f2_ctrl:?}"
    );

    let d_align = synth_sv_path(&root.join("serv_aligner.v")).expect("serv_aligner.v");
    let f2_align = multi_driven_nets(&d_align);
    assert!(
        f2_align.is_empty(),
        "serv_aligner.v must have 0 F2 multi-driven nets (o_wb_ibus_cyc not double-driven), got {f2_align:?}"
    );
}

#[test]
fn submodule_instantiation_has_zero_f2_and_zero_fallback_iobs() {
    let sub_sv = r#"
module sub_decode(
    input  wire       clk,
    input  wire [3:0] i_op,
    input  wire       i_en,
    output reg        o_sh_right,
    output reg        o_last
);
    reg [3:0] op_r;
    wire co_sh_right = op_r[2] ^ op_r[0];
    always @(posedge clk) begin
        if (i_en) begin
            op_r       <= i_op;
            o_sh_right <= co_sh_right;
            o_last     <= op_r[3] & op_r[1];
        end
    end
endmodule
"#;
    let top_sv = r#"
module top_wrap(
    input  wire       clk,
    input  wire [3:0] i_op,
    input  wire       i_en,
    output wire       led
);
    wire sh_right;
    wire last_b;
    sub_decode u_dec(
        .clk(clk),
        .i_op(i_op),
        .i_en(i_en),
        .o_sh_right(sh_right),
        .o_last(last_b)
    );
    assign led = sh_right ^ last_b;
endmodule
"#;
    let d = synth_sv_sources(&[("sub_decode.v", sub_sv), ("top_wrap.v", top_sv)])
        .expect("synth hierarchical sub_decode");
    assert_eq!(d.name, intern("top_wrap"));
    let sub_iobs: Vec<_> = d
        .cells
        .iter()
        .filter(|c| {
            matches!(c.kind, CellKind::IobOut | CellKind::IobIn)
                && resolve(c.name).starts_with("u_dec_")
        })
        .map(|c| resolve(c.name).to_string())
        .collect();
    assert!(
        sub_iobs.is_empty(),
        "submodule u_dec must not emit fallback IOB cells, got {sub_iobs:?}"
    );
    let f2 = multi_driven_nets(&d);
    assert!(
        f2.is_empty(),
        "hierarchical design must have 0 F2 multi-driven nets, got {f2:?}"
    );
    let dev = Device::load_part("HL10T-C32-1").expect("load HL10T");
    pack(&d, &dev).expect("HL10T pack must succeed without unsupported IobIn");
}

#[test]
fn force_list_input_ports_do_not_emit_unsupported_iob_in() {
    let sv = r#"
module axi_pcpi_stub(
    input  wire clk,
    input  wire pcpi_wait,
    input  wire pcpi_ready,
    input  wire s_axil_awvalid,
    input  wire s_axil_wvalid,
    input  wire s_axil_bready,
    input  wire d,
    output wire y
);
    reg q;
    always @(posedge clk) q <= d;
    assign y = q;
endmodule
"#;
    let d = synth_sv(sv, "axi_pcpi_stub.v").expect("synth stub");
    assert!(
        !d.cells.iter().any(|c| matches!(c.kind, CellKind::IobIn)),
        "force-list input ports must never emit IobIn cells: {:?}",
        d.cells
    );
    let dev = Device::load_part("HL10T-C32-1").expect("load HL10T");
    pack(&d, &dev).expect("HL10T pack must accept design with unused force-list inputs");
}
