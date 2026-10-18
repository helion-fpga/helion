use helion_ir::{CellKind, PortDir};
use helion_sv::synth_sv;

fn synth(sv: &str) -> helion_ir::Design {
    synth_sv(sv, "sv2017.sv").expect("SV2017 synthesis must succeed")
}

// ============================================================================
// 1. `interface` and `modport` port bundle flattening (`axi_if.master`, `axi_if.slave`)
// ============================================================================

#[test]
fn sv2017_interface_modport_master_slave_hierarchy_flattens_and_synthesizes() {
    let sv = r#"
interface axi_stream_if #(parameter DW = 4);
    logic valid;
    logic ready;
    logic [DW-1:0] data;

    modport master (output valid, data, input ready);
    modport slave  (input valid, data, output ready);
endinterface

module producer (
    input logic clk,
    input logic [3:0] in_data,
    input logic in_en,
    axi_stream_if.master m_axis
);
    always_ff @(posedge clk) begin
        if (m_axis.ready) begin
            m_axis.valid <= in_en;
            m_axis.data  <= in_data ^ 4'b1010;
        end
    end
endmodule

module consumer (
    input logic clk,
    input logic [3:0] mask,
    axi_stream_if.slave s_axis,
    output logic [3:0] out_data
);
    assign s_axis.ready = 1'b1;
    always_ff @(posedge clk) begin
        if (s_axis.valid) begin
            out_data <= s_axis.data & mask;
        end
    end
endmodule

module top (
    input logic clk,
    input logic [3:0] in_data,
    input logic [3:0] mask,
    input logic in_en,
    output logic [3:0] out_data
);
    axi_stream_if #(.DW(4)) u_bus();

    producer u_prod (
        .clk(clk),
        .in_data(in_data),
        .in_en(in_en),
        .m_axis(u_bus)
    );

    consumer u_cons (
        .clk(clk),
        .mask(mask),
        .s_axis(u_bus),
        .out_data(out_data)
    );
endmodule
"#;
    let d = synth(sv);
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    // producer has valid (1 FF) + data (4 FFs) = 5 FFs; consumer has out_data (4 FFs) = 4 FFs -> 9 FFs total.
    assert!(
        ffs >= 8,
        "producer + consumer connected via axi_stream_if must synthesize >=8 HFFs, got {ffs}"
    );
    assert!(
        luts >= 4,
        "producer + consumer must synthesize combinational LUTs, got {luts}"
    );
}

#[test]
fn sv2017_interface_port_on_top_module_flattens_directions_from_modport() {
    let sv = r#"
interface bus_if;
    logic req;
    logic gnt;
    logic [3:0] addr;
    logic [3:0] rdata;
    modport master (output req, addr, input gnt, rdata);
endinterface

module master_top (
    input logic clk,
    input logic [3:0] seed,
    bus_if.master bus,
    output logic [3:0] captured
);
    assign bus.req = bus.gnt;
    assign bus.addr = seed ^ 4'h5;
    always_ff @(posedge clk) begin
        if (bus.gnt) begin
            captured <= bus.rdata + seed;
        end
    end
endmodule
"#;
    let d = synth(sv);
    let port_dir = |name: &str| -> Option<PortDir> {
        d.ports
            .iter()
            .find(|p| helion_ir::resolve(p.name) == name)
            .map(|p| p.dir)
    };
    assert_eq!(port_dir("bus_req"), Some(PortDir::Out));
    assert_eq!(port_dir("bus_addr"), Some(PortDir::Out));
    assert_eq!(port_dir("bus_gnt"), Some(PortDir::In));
    assert_eq!(port_dir("bus_rdata"), Some(PortDir::In));
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(ffs, 4, "captured[3:0] must infer 4 HFFs");
}

// ============================================================================
// 2. Nested `typedef struct packed` and `typedef union packed` member slicing
// ============================================================================

#[test]
fn sv2017_nested_packed_struct_and_union_lhs_and_rhs_slicing() {
    let sv = r#"
typedef struct packed {
    logic [3:0] opcode;
    logic [3:0] flags;
} hdr_t;

typedef union packed {
    logic [7:0] raw;
    hdr_t       hdr;
} word_u;

typedef struct packed {
    word_u      u;
    logic [3:0] tag;
} packet_t;

module struct_union_top (
    input logic clk,
    input logic [7:0] raw_in,
    input logic [3:0] tag_in,
    output logic [3:0] op_out,
    output logic [3:0] flg_out,
    output logic [3:0] tag_out
);
    packet_t pkt_d, pkt_q;

    always_comb begin
        pkt_d.u.raw = raw_in;
        pkt_d.u.hdr.flags[0] = tag_in[0];
        pkt_d.tag = tag_in;
    end

    always_ff @(posedge clk) begin
        pkt_q <= pkt_d;
    end

    assign op_out  = pkt_q.u.hdr.opcode;
    assign flg_out = pkt_q.u.hdr.flags;
    assign tag_out = pkt_q.tag;
endmodule
"#;
    let d = synth(sv);
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(
        ffs, 12,
        "12-bit packet_t (8-bit union word_u + 4-bit tag) must synthesize 12 HFFs, got {ffs}"
    );
}

// ============================================================================
// 3. `unique0 case` / `priority case` and `inside` set-membership with ranges & wildcards
// ============================================================================

#[test]
fn sv2017_unique0_case_and_inside_ranges_and_wildcards() {
    let sv = r#"
module inside_unique0_top (
    input logic clk,
    input logic [7:0] x,
    input logic [1:0] sel,
    input logic [3:0] a,
    input logic [3:0] b,
    output logic hit_q,
    output logic [3:0] mux_q
);
    logic hit_d;
    logic [3:0] mux_d;

    always_comb begin
        hit_d = (x inside {[8'd0:8'd7], 8'b1???_????, 8'h4?});
        mux_d = 4'd0;
        unique0 case (sel)
            2'd0: mux_d = a & b;
            2'd1: mux_d = a | b;
            2'd2: mux_d = a ^ b;
        endcase
    end

    always_ff @(posedge clk) begin
        hit_q <= hit_d;
        mux_q <= mux_d;
    end
endmodule
"#;
    let d = synth(sv);
    assert_ne!(
        d.attrs.get("ASSIGN_NOT_LOWERED"),
        Some("1"),
        "inside with [0:7] range and 8'b1???_???? / 8'h4? wildcards must lower cleanly"
    );
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    assert_eq!(ffs, 5, "hit_q (1) + mux_q (4) must synthesize 5 HFFs");
    assert!(luts >= 5, "inside + unique0 case must map LUT6 cones, got {luts}");
}

// ============================================================================
// 4. `{<<{}}` / `{>>{}}` / `{<< N {expr}}` streaming concatenation bit/byte reversal
// ============================================================================

#[test]
fn sv2017_streaming_concat_bit_and_byte_reversal() {
    let sv = r#"
module stream_top (
    input logic clk,
    input logic [7:0] in_byte,
    input logic [15:0] in_half,
    output logic [7:0] bit_rev_q,
    output logic [15:0] byte_rev_q,
    output logic [7:0] pack_lr_q
);
    // Const streaming bit-reversal check in localparam / elaboration:
    // {<< {4'b1100}} == 4'b0011 (3)
    // {<< 4 {8'hA5}} == 8'h5A
    always_ff @(posedge clk) begin
        bit_rev_q  <= {<< {in_byte}} ^ {<< {8'b1011_0001}};
        byte_rev_q <= {<< byte {in_half}} ^ {<< 8 {16'h1234}};
        pack_lr_q  <= {>> {in_byte[7:4], in_byte[3:0]}};
    end
endmodule
"#;
    let d = synth(sv);
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(
        ffs,
        8 + 16 + 8,
        "bit_rev_q (8) + byte_rev_q (16) + pack_lr_q (8) must infer 32 HFFs, got {ffs}"
    );
}
