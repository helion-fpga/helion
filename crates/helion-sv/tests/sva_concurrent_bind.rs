use helion_ir::CellKind;
use helion_sv::sva::synth_sv_with_sva;
use helion_sv::synth_sv;

// ============================================================================
// 1. Sampled-Value System Functions: `$past(expr, N)`, `$rose`, `$fell`, `$stable`
// ============================================================================

#[test]
fn sva_sampled_value_functions_synthesize_shift_register_delay_chains() {
    let sv = r#"
module sampled_rtl_top (
    input logic clk,
    input logic req,
    input logic [3:0] data_in,
    output logic rose_q,
    output logic fell_q,
    output logic stable_q,
    output logic [3:0] past3_q
);
    always_ff @(posedge clk) begin
        rose_q   <= $rose(req);
        fell_q   <= $fell(req);
        stable_q <= $stable(data_in);
        past3_q  <= $past(data_in, 3);
    end
endmodule
"#;
    let d = synth_sv(sv, "sampled_rtl.sv").expect("synth sampled_rtl_top");
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
    // Output FFs: rose_q(1) + fell_q(1) + stable_q(1) + past3_q(4) = 7
    // Delay-chain FFs: $rose(1) + $fell(1) + $stable(4) + $past(data_in, 3)(3*4=12) = 18 -> 25 total FFs
    assert!(
        ffs >= 20,
        "$past(data_in, 3), $rose, $fell, and $stable must synthesize multi-stage HFF delay chains, got {ffs}"
    );
    assert!(
        luts >= 4,
        "$rose / $fell / $stable must synthesize transition/equality LUTs, got {luts}"
    );
}

// ============================================================================
// 2. Concurrent SVA `sequence`, `property`, `disable iff`, `##[m:n]`, `assert`/`cover`
// ============================================================================

#[test]
fn sva_sequence_property_disable_iff_and_bounded_range_delay_elaborate_and_synthesize() {
    let sv = r#"
module sva_pipeline_mon (
    input logic clk,
    input logic rst,
    input logic req,
    input logic ack,
    input logic [3:0] d,
    output logic [3:0] q
);
    always_ff @(posedge clk) begin
        if (rst)
            q <= 4'd0;
        else
            q <= d;
    end

    sequence s_req;
        $rose(req);
    endsequence

    property p_handshake;
        @(posedge clk) disable iff (rst)
        s_req |-> ##[1:3] (ack && (q == $past(d, 2)));
    endproperty

    u_assert_hs: assert property (p_handshake);
    u_cover_hs:  cover property (@(posedge clk) disable iff (rst) req |=> ##[1:2] ack);
endmodule
"#;
    let out = synth_sv_with_sva(sv, "sva_pipeline_mon.sv").expect("synth_sv_with_sva");
    assert_eq!(
        out.properties.len(),
        1,
        "assert property (p_handshake) must elaborate into SvaSynthOut::properties"
    );
    let prop = &out.properties[0];
    assert_eq!(prop.label.as_deref(), Some("u_assert_hs"));
    assert_eq!(prop.delay, 1);
    assert!(
        prop.slots >= 3,
        "$rose(req) (1 slot) + $past(d, 2) (2 slots) must allocate >=3 history slots, got {}",
        prop.slots
    );

    let ffs = out
        .design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    // 4 FFs for q[3:0] + monitor pipeline & $past shift registers
    assert!(
        ffs > 4,
        "concurrent SVA monitor FSMs must synthesize shift-register HFFs beyond q[3:0], got {ffs}"
    );
}

// ============================================================================
// 3. Module `bind` Instantiation (`bind <target_mod> <checker_mod> <inst_name>(.*);`)
// ============================================================================

#[test]
fn sva_bind_directive_instantiates_checker_into_target_modules() {
    let sv = r#"
module handshake_checker (
    input logic clk,
    input logic rst,
    input logic valid,
    input logic ready
);
    property p_valid_hold;
        @(posedge clk) disable iff (rst)
        (valid && !ready) |=> valid;
    endproperty

    chk_hold: assert property (p_valid_hold);
    cov_xfer: cover property (@(posedge clk) valid && ready);
endmodule

module axi_channel (
    input logic clk,
    input logic rst,
    input logic valid_i,
    input logic ready_i,
    output logic valid,
    output logic ready
);
    always_ff @(posedge clk) begin
        valid <= valid_i;
        ready <= ready_i;
    end
endmodule

bind axi_channel handshake_checker u_hs_chk (.*);

module soc_top (
    input logic clk,
    input logic rst,
    input logic v0,
    input logic r0,
    input logic v1,
    input logic r1,
    output logic v0_o,
    output logic r0_o,
    output logic v1_o,
    output logic r1_o
);
    axi_channel u_ch0 (
        .clk(clk),
        .rst(rst),
        .valid_i(v0),
        .ready_i(r0),
        .valid(v0_o),
        .ready(r0_o)
    );

    axi_channel u_ch1 (
        .clk(clk),
        .rst(rst),
        .valid_i(v1),
        .ready_i(r1),
        .valid(v1_o),
        .ready(r1_o)
    );
endmodule
"#;
    let d = synth_sv(sv, "bind_soc.sv").expect("synth bind_soc.sv");
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    // Without bind: u_ch0 (2 FFs) + u_ch1 (2 FFs) = 4 FFs.
    // With `bind axi_channel handshake_checker u_hs_chk (.*);`, each of the 2 axi_channel
    // instances gets its own bound `u_hs_chk` monitor FSM (pipeline + status FFs).
    assert!(
        ffs >= 8,
        "bound handshake_checker must be elaborated into both axi_channel instances (expected >=8 HFFs, got {ffs})"
    );
    let has_bound_cells = d
        .cells
        .iter()
        .any(|c| helion_ir::resolve(c.name).contains("u_hs_chk"));
    assert!(
        has_bound_cells,
        "synthesized netlist must contain cells from bound instance `u_hs_chk`"
    );
}
