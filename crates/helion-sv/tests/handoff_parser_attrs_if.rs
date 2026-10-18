use helion_ir::CellKind;
use helion_sv::synth_sv;
use std::path::Path;

fn synth(sv: &str) -> helion_ir::Design {
    synth_sv(sv, "test.sv").expect("synthesis must succeed")
}

// ============================================================================
// Issue 1: (* full_case *), (* parallel_case *), and comma-separated attributes
// ============================================================================

#[test]
fn issue1_full_case_attribute_before_case_is_not_dropped() {
    let sv_with_attr = r#"
module fc_test (
    input wire [1:0] sel,
    input wire [3:0] a,
    input wire [3:0] b,
    input wire [3:0] c,
    output reg [3:0] y
);
    always @* begin
        (* full_case *)
        case (sel)
            2'd0: y = a & b;
            2'd1: y = a | b;
            2'd2: y = a ^ c;
        endcase
    end
endmodule
"#;
    let sv_without_attr = r#"
module fc_test (
    input wire [1:0] sel,
    input wire [3:0] a,
    input wire [3:0] b,
    input wire [3:0] c,
    output reg [3:0] y
);
    always @* begin
        case (sel)
            2'd0: y = a & b;
            2'd1: y = a | b;
            2'd2: y = a ^ c;
        endcase
    end
endmodule
"#;
    let d_attr = synth(sv_with_attr);
    let d_no_attr = synth(sv_without_attr);
    assert!(
        !d_attr.cells.is_empty(),
        "(* full_case *) before case must not drop the always block"
    );
    assert_eq!(
        d_attr.cells.len(),
        d_no_attr.cells.len(),
        "(* full_case *) must synthesize identical cell count to unattributed case"
    );
}

#[test]
fn issue1_comma_separated_parallel_full_case_in_clocked_and_comb_blocks() {
    let sv = r#"
module pfc_test (
    input wire clk,
    input wire [1:0] state,
    input wire [3:0] x,
    input wire [3:0] y,
    output reg [3:0] out_q,
    output reg [3:0] comb_o
);
    always @* begin
        comb_o = 4'd0;
        (* parallel_case, full_case *)
        case (state)
            2'd0: comb_o = x + y;
            2'd1: comb_o = x ^ y;
            default: comb_o = x & y;
        endcase
    end

    always @(posedge clk) begin
        if (state == 2'd3) begin
            out_q <= 4'd0;
        end else
        (* parallel_case, full_case *)
        case (state)
            2'd0: out_q <= x & y;
            2'd1: out_q <= x | y;
            2'd2: out_q <= x ^ y;
        endcase
    end
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
    assert_eq!(ffs, 4, "clocked case under (* parallel_case, full_case *) must emit 4 HFFs");
    assert!(luts >= 4, "comb + sequential case blocks must emit LUTs, got {luts}");
}

#[test]
fn issue1_picorv32_mem_la_wstrb_driven_and_cell_count_restored() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let picorv32_path = manifest_dir.join("../../third_party/picorv32/picorv32.v");
    let d = helion_sv::synth_sv_path(&picorv32_path).expect("picorv32 must synthesize");

    assert!(
        d.cells.len() >= 2100,
        "picorv32 cell count should be >= 2100, got {}",
        d.cells.len()
    );

    // Verify mem_la_wstrb bits have real drivers in the synthesized netlist
    for bit in 0..4 {
        let net_name = format!("mem_la_wstrb_{bit}");
        let net = d
            .nets
            .iter()
            .find(|n| helion_ir::resolve(n.name) == net_name)
            .unwrap_or_else(|| panic!("missing net {net_name} in picorv32"));
        let has_driver = net.endpoints.iter().any(|ep| {
            let pin = helion_ir::resolve(ep.pin);
            pin == "O" || pin == "Q"
        });
        assert!(
            has_driver,
            "picorv32 net {net_name} must be driven by a cell (not dropped by (* full_case *))"
        );
    }
}

// ============================================================================
// Issue 2: Instance parameter override #(.N(2)) elaboration in procedural if
// ============================================================================

#[test]
fn issue2_instance_parameter_override_selects_distinct_procedural_if_branch() {
    let sv_n1 = r#"
module worker #(parameter N = 1) (
    input wire clk,
    input wire [7:0] a,
    input wire [7:0] b,
    output reg [7:0] r,
    output reg [7:0] extra
);
    always @(posedge clk) begin
        if (N == 1) begin
            r <= a & b;
        end else begin
            r <= a ^ b;
            extra <= a | b;
        end
    end
endmodule

module top (
    input wire clk,
    input wire [7:0] a,
    input wire [7:0] b,
    output wire [7:0] r,
    output wire [7:0] extra
);
    worker #(.N(1)) u_w (.clk(clk), .a(a), .b(b), .r(r), .extra(extra));
endmodule
"#;

    let sv_n2 = r#"
module worker #(parameter N = 1) (
    input wire clk,
    input wire [7:0] a,
    input wire [7:0] b,
    output reg [7:0] r,
    output reg [7:0] extra
);
    always @(posedge clk) begin
        if (N == 1) begin
            r <= a & b;
        end else begin
            r <= a ^ b;
            extra <= a | b;
        end
    end
endmodule

module top (
    input wire clk,
    input wire [7:0] a,
    input wire [7:0] b,
    output wire [7:0] r,
    output wire [7:0] extra
);
    worker #(.N(2)) u_w (.clk(clk), .a(a), .b(b), .r(r), .extra(extra));
endmodule
"#;

    let d1 = synth(sv_n1);
    let d2 = synth(sv_n2);

    let ffs1 = d1
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let ffs2 = d2
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();

    assert_eq!(ffs1, 8, "N=1 branch only writes 8-bit r (8 HFFs)");
    assert_eq!(ffs2, 16, "N=2 override must select else branch writing r and extra (16 HFFs)");
    assert_ne!(
        d1.cells.len(),
        d2.cells.len(),
        "#(.N(1)) and #(.N(2)) must produce distinct cell counts"
    );
}

#[test]
fn issue2_same_param_name_across_modules_and_localparam_recomputation() {
    // Verifies that `parameter N = 1` in child does not pollute `parameter N = 2` in top,
    // and that `localparam M = N + 10` in child recomputes with the overridden N=2.
    let sv = r#"
module child #(parameter N = 1) (
    input wire clk,
    input wire [3:0] a,
    input wire [3:0] b,
    output reg [3:0] y
);
    localparam M = N + 10;
    always @(posedge clk) begin
        if (M == 12) begin
            y <= a ^ b;
        end
    end
endmodule

module top #(parameter N = 2) (
    input wire clk,
    input wire [3:0] a,
    input wire [3:0] b,
    output wire [3:0] y
);
    child #(.N(N)) u_child (.clk(clk), .a(a), .b(b), .y(y));
endmodule
"#;
    let d = synth(sv);
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(
        ffs, 4,
        "top's N=2 must not be polluted by child's default N=1, and child's localparam M=N+10 must evaluate to 12"
    );
}

// ============================================================================
// Issue 6: const_atom width masking for `&` (AND-reduce) and `~` (bitwise NOT)
// ============================================================================

#[test]
fn issue6_const_atom_and_reduction_respects_operand_width() {
    let sv = r#"
module and_red_test #(parameter ONE = 1, parameter ZERO = 0) (
    input wire clk,
    input wire [3:0] a,
    output reg [3:0] q1,
    output reg [3:0] q2,
    output reg [3:0] q3,
    output reg [3:0] q4
);
    always @(posedge clk) begin
        if (&1'b1) begin
            q1 <= a;
        end
        if (&4'b1111) begin
            q2 <= a;
        end
        if (&4'b0111) begin
            q3 <= a;
        end
        if (&ONE && !(&ZERO)) begin
            q4 <= a;
        end
    end
endmodule
"#;
    let d = synth(sv);
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    // q1 (4), q2 (4), and q4 (4) must be synthesized; q3 (&4'b0111 == 0) must not.
    assert_eq!(
        ffs, 12,
        "&1'b1, &4'b1111, and &ONE must evaluate to 1 while &4'b0111 evaluates to 0 (expected 12 HFFs, got {ffs})"
    );
}

#[test]
fn issue6_const_atom_bitwise_not_masks_to_operand_width() {
    let sv = r#"
module not_mask_test (
    input wire clk,
    input wire [3:0] a,
    output reg [3:0] q1,
    output reg [3:0] q2
);
    localparam INV4 = ~4'b1010;
    localparam INV1_ZERO = ~1'b0;
    localparam INV1_ONE = ~1'b1;
    always @(posedge clk) begin
        if (INV4 == 4'b0101) begin
            q1 <= a;
        end
        if (INV1_ZERO == 1 && INV1_ONE == 0) begin
            q2 <= a;
        end
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
        ffs, 8,
        "~4'b1010 must mask to 4'b0101 (5) and ~1'b0 / ~1'b1 must mask to 1 / 0 (expected 8 HFFs, got {ffs})"
    );
}
