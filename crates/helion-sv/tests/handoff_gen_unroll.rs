use helion_ir::{intern, resolve, CellKind};
use helion_sv::synth_sv;

#[test]
fn nested_2d_generate_for_uniquifies_signals_and_instances_outer_inner() {
    let sv = r#"
module leaf_and #(parameter W = 4) (
    input  wire [W-1:0] a,
    input  wire         en,
    output wire [W-1:0] y
);
    assign y = en ? a : {W{1'b0}};
endmodule

module grid_unroll #(
    parameter M = 2,
    parameter S = 3,
    parameter W = 4
) (
    input  wire [S*W-1:0] s_data,
    input  wire [M*S-1:0] mask,
    output wire [M*W-1:0] m_data
);
    wire req [0:M-1][0:S-1];
    wire [W-1:0] cell_out [0:M-1][0:S-1];

    genvar i, j;
    generate
        for (i = 0; i < M; i = i + 1) begin : gen_m
            wire [W-1:0] row_acc;
            for (j = 0; j < S; j = j + 1) begin : gen_s
                localparam int FLAT_IDX = i * S + j;
                localparam int SLICE_LO = j * W;
                wire [W-1:0] cell_in;
                assign req[i][j] = mask[FLAT_IDX];
                assign cell_in = s_data[SLICE_LO +: W];
                leaf_and #(.W(W)) u_leaf (
                    .a(cell_in),
                    .en(req[i][j]),
                    .y(cell_out[i][j])
                );
            end : gen_s
            assign row_acc = cell_out[i][0] ^ cell_out[i][1] ^ cell_out[i][2];
            assign m_data[i*W +: W] = row_acc;
        end : gen_m
    endgenerate
endmodule
"#;
    let d = synth_sv(sv, "grid_unroll.sv").expect("synth grid_unroll");

    // Verify inner instances get `_{i}_{j}` and connect to uniquified `cell_in_{i}_{j}`
    for i in 0..2 {
        let row_bit0 = intern(&format!("row_acc_{i}_0"));
        assert!(
            d.nets.iter().any(|n| n.name == row_bit0),
            "expected outer signal net row_acc_{i}_0"
        );
        for j in 0..3 {
            let inst_name = intern(&format!("u_leaf_{i}_{j}"));
            let inst = d
                .instances
                .iter()
                .find(|inst| inst.name == inst_name)
                .unwrap_or_else(|| {
                    panic!(
                        "expected inner instance u_leaf_{i}_{j}, got {:?}",
                        d.instances
                            .iter()
                            .map(|x| resolve(x.name))
                            .collect::<Vec<_>>()
                    )
                });
            let expected_sig = intern(&format!("cell_in_{i}_{j}"));
            assert!(
                inst.conns.iter().any(|&(p, n)| p == intern("a") && n == expected_sig),
                "expected u_leaf_{i}_{j}.a connected to cell_in_{i}_{j}"
            );
        }
    }
    assert_ne!(d.attrs.get("NO_BODY"), Some("1"), "grid_unroll must not be NO_BODY");
    assert_ne!(
        d.attrs.get("GENERATE_NOT_LOWERED"),
        Some("1"),
        "grid_unroll must not set GENERATE_NOT_LOWERED"
    );
    let luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    assert!(luts >= 8, "expected real LUTs for 2x3x4 crossbar, got {luts}");
}

#[test]
fn braceless_nested_generate_for_and_clocked_2d_regs() {
    let sv = r#"
module mat_pipe #(
    parameter M = 2,
    parameter S = 2
) (
    input  wire           clk,
    input  wire [M*S-1:0] d_in,
    output wire [M-1:0]   row_or
);
    reg q_mat [0:M-1][0:S-1];
    genvar i, j;
    generate
        for (i = 0; i < M; i = i + 1)
            for (j = 0; j < S; j = j + 1)
                always @(posedge clk) begin
                    q_mat[i][j] <= d_in[i * S + j];
                end
        for (i = 0; i < M; i = i + 1) begin
            assign row_or[i] = q_mat[i][0] | q_mat[i][1];
        end
    endgenerate
endmodule
"#;
    let d = synth_sv(sv, "mat_pipe.sv").expect("synth mat_pipe");
    assert_ne!(d.attrs.get("NO_BODY"), Some("1"));
    assert_ne!(d.attrs.get("GENERATE_NOT_LOWERED"), Some("1"));
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
    assert_eq!(ffs, 4, "expected 2x2=4 FFs from clocked 2D generate-for, got {ffs}");
    assert!(luts >= 2, "expected row OR LUTs, got {luts}");
}

#[test]
fn axi_interconnect_2d_generate_for_crossbar_synthesizes_cells() {
    let sv = r#"
module axi_interconnect #(
    parameter S_COUNT = 2,
    parameter M_COUNT = 2,
    parameter DATA_WIDTH = 8,
    parameter ADDR_WIDTH = 16,
    parameter STRB_WIDTH = (DATA_WIDTH/8),
    parameter M_REGIONS = 1,
    parameter M_BASE_ADDR = 32'h8000_0000,
    parameter M_ADDR_WIDTH = {M_COUNT{{M_REGIONS{32'd12}}}},
    parameter M_CONNECT_READ = {M_COUNT{{S_COUNT{1'b1}}}},
    parameter M_CONNECT_WRITE = {M_COUNT{{S_COUNT{1'b1}}}},
    parameter M_SECURE = {M_COUNT{1'b0}}
) (
    input  wire                              clk,
    input  wire                              rst,
    input  wire [S_COUNT*ADDR_WIDTH-1:0]     s_axi_awaddr,
    input  wire [S_COUNT-1:0]                s_axi_awvalid,
    output wire [S_COUNT-1:0]                s_axi_awready,
    input  wire [S_COUNT*DATA_WIDTH-1:0]     s_axi_wdata,
    input  wire [S_COUNT*STRB_WIDTH-1:0]     s_axi_wstrb,
    input  wire [S_COUNT-1:0]                s_axi_wvalid,
    output wire [S_COUNT-1:0]                s_axi_wready,
    output wire [M_COUNT*ADDR_WIDTH-1:0]     m_axi_awaddr,
    output wire [M_COUNT-1:0]                m_axi_awvalid,
    input  wire [M_COUNT-1:0]                m_axi_awready,
    output wire [M_COUNT*DATA_WIDTH-1:0]     m_axi_wdata,
    output wire [M_COUNT*STRB_WIDTH-1:0]     m_axi_wstrb,
    output wire [M_COUNT-1:0]                m_axi_wvalid,
    input  wire [M_COUNT-1:0]                m_axi_wready
);
    wire req [0:M_COUNT-1][0:S_COUNT-1];
    reg  gnt_q [0:M_COUNT-1][0:S_COUNT-1];
    wire [DATA_WIDTH-1:0] cell_wdata [0:M_COUNT-1][0:S_COUNT-1];
    wire [ADDR_WIDTH-1:0] cell_awaddr [0:M_COUNT-1][0:S_COUNT-1];

    genvar i, j;
    generate
        for (i = 0; i < M_COUNT; i = i + 1) begin : gen_m
            localparam [ADDR_WIDTH-1:0] REGION_BASE = M_BASE_ADDR[i*ADDR_WIDTH +: ADDR_WIDTH];
            localparam integer REGION_W = M_ADDR_WIDTH[i*32 +: 32];
            for (j = 0; j < S_COUNT; j = j + 1) begin : gen_s
                localparam integer CONN_BIT = i * S_COUNT + j;
                wire addr_hit;
                assign addr_hit = (s_axi_awaddr[j*ADDR_WIDTH + ADDR_WIDTH - 1 : j*ADDR_WIDTH + REGION_W] ==
                                   REGION_BASE[ADDR_WIDTH - 1 : REGION_W]);
                assign req[i][j] = s_axi_awvalid[j] & addr_hit & M_CONNECT_WRITE[CONN_BIT] & ~M_SECURE[i];

                always @(posedge clk) begin
                    if (rst) begin
                        gnt_q[i][j] <= 1'b0;
                    end else begin
                        gnt_q[i][j] <= req[i][j] & m_axi_awready[i];
                    end
                end

                assign cell_wdata[i][j] = gnt_q[i][j] ? s_axi_wdata[j*DATA_WIDTH +: DATA_WIDTH] : {DATA_WIDTH{1'b0}};
                assign cell_awaddr[i][j] = gnt_q[i][j] ? s_axi_awaddr[j*ADDR_WIDTH +: ADDR_WIDTH] : {ADDR_WIDTH{1'b0}};
            end
            assign m_axi_awvalid[i] = req[i][0] | req[i][1];
            assign m_axi_wvalid[i]  = (gnt_q[i][0] & s_axi_wvalid[0]) | (gnt_q[i][1] & s_axi_wvalid[1]);
            assign m_axi_wdata[i*DATA_WIDTH +: DATA_WIDTH] = cell_wdata[i][0] | cell_wdata[i][1];
            assign m_axi_awaddr[i*ADDR_WIDTH +: ADDR_WIDTH] = cell_awaddr[i][0] | cell_awaddr[i][1];
            assign m_axi_wstrb[i*STRB_WIDTH +: STRB_WIDTH] =
                (gnt_q[i][0] ? s_axi_wstrb[0*STRB_WIDTH +: STRB_WIDTH] : {STRB_WIDTH{1'b0}}) |
                (gnt_q[i][1] ? s_axi_wstrb[1*STRB_WIDTH +: STRB_WIDTH] : {STRB_WIDTH{1'b0}});
        end
        for (j = 0; j < S_COUNT; j = j + 1) begin : gen_s_ack
            assign s_axi_awready[j] = gnt_q[0][j] | gnt_q[1][j];
            assign s_axi_wready[j]  = (gnt_q[0][j] & m_axi_wready[0]) | (gnt_q[1][j] & m_axi_wready[1]);
        end
    endgenerate
endmodule
"#;
    let d = synth_sv(sv, "axi_interconnect.v").expect("synth axi_interconnect");
    assert_ne!(
        d.attrs.get("NO_BODY"),
        Some("1"),
        "axi_interconnect must not produce NO_BODY"
    );
    assert_ne!(
        d.attrs.get("GENERATE_NOT_LOWERED"),
        Some("1"),
        "axi_interconnect must not set GENERATE_NOT_LOWERED"
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
    assert_eq!(ffs, 4, "expected 4 grant FFs, got {ffs}");
    assert!(luts >= 20, "expected real decode and crossbar mux LUTs, got {luts}");
}

#[test]
fn priority_encoder_arbiter_and_full_axi_interconnect_synthesize_without_no_body() {
    // Tests:
    // 1. `priority_encoder` nested `for (l = 1; l < LEVELS; l = l + 1)` ->
    //    `for (n = 0; n < W/(2*2**l); n = n + 1)` with `2*2**l` precedence and
    //    2D `stage_valid[l][n]` + `stage_enc[l][(n+1)*(l+1)-1:n*(l+1)]`.
    // 2. `arbiter` dynamic barrel right-shift `{PORTS{1'b1}} >> (PORTS - masked_request_index)`.
    // 3. `axi_interconnect` with `initial begin $display(...); end` sanity check,
    //    nested `M_ADDR_WIDTH = {M_COUNT{{M_REGIONS{32'd24}}}}` replication,
    //    `calcBaseAddrs(0)`, and `generate for` request/acknowledge loops.
    let sv = r#"
module priority_encoder #(
    parameter WIDTH = 4,
    parameter LSB_HIGH_PRIORITY = 0
) (
    input  wire [WIDTH-1:0]         input_unencoded,
    output wire                     output_valid,
    output wire [$clog2(WIDTH)-1:0] output_encoded,
    output wire [WIDTH-1:0]         output_unencoded
);
    parameter LEVELS = WIDTH > 2 ? $clog2(WIDTH) : 1;
    parameter W = 2**LEVELS;

    wire [W-1:0] input_padded = {{W-WIDTH{1'b0}}, input_unencoded};

    wire [W/2-1:0] stage_valid[LEVELS-1:0];
    wire [W/2-1:0] stage_enc[LEVELS-1:0];

    generate
        genvar l, n;

        for (n = 0; n < W/2; n = n + 1) begin : loop_in
            assign stage_valid[0][n] = |input_padded[n*2+1:n*2];
            if (LSB_HIGH_PRIORITY) begin
                assign stage_enc[0][n] = !input_padded[n*2+0];
            end else begin
                assign stage_enc[0][n] = input_padded[n*2+1];
            end
        end

        for (l = 1; l < LEVELS; l = l + 1) begin : loop_levels
            for (n = 0; n < W/(2*2**l); n = n + 1) begin : loop_compress
                assign stage_valid[l][n] = |stage_valid[l-1][n*2+1:n*2];
                if (LSB_HIGH_PRIORITY) begin
                    assign stage_enc[l][(n+1)*(l+1)-1:n*(l+1)] =
                        stage_valid[l-1][n*2+0]
                        ? {1'b0, stage_enc[l-1][(n*2+1)*l-1:(n*2+0)*l]}
                        : {1'b1, stage_enc[l-1][(n*2+2)*l-1:(n*2+1)*l]};
                end else begin
                    assign stage_enc[l][(n+1)*(l+1)-1:n*(l+1)] =
                        stage_valid[l-1][n*2+1]
                        ? {1'b1, stage_enc[l-1][(n*2+2)*l-1:(n*2+1)*l]}
                        : {1'b0, stage_enc[l-1][(n*2+1)*l-1:(n*2+0)*l]};
                end
            end
        end
    endgenerate

    assign output_valid = stage_valid[LEVELS-1];
    assign output_encoded = stage_enc[LEVELS-1];
    assign output_unencoded = 1 << output_encoded;
endmodule

module arbiter #(
    parameter PORTS = 4,
    parameter ARB_TYPE_ROUND_ROBIN = 0,
    parameter ARB_BLOCK = 0,
    parameter ARB_BLOCK_ACK = 1,
    parameter ARB_LSB_HIGH_PRIORITY = 0
) (
    input  wire                     clk,
    input  wire                     rst,
    input  wire [PORTS-1:0]         request,
    input  wire [PORTS-1:0]         acknowledge,
    output wire [PORTS-1:0]         grant,
    output wire                     grant_valid,
    output wire [$clog2(PORTS)-1:0] grant_encoded
);
    reg [PORTS-1:0] grant_reg = 0, grant_next;
    reg grant_valid_reg = 0, grant_valid_next;
    reg [$clog2(PORTS)-1:0] grant_encoded_reg = 0, grant_encoded_next;

    assign grant_valid = grant_valid_reg;
    assign grant = grant_reg;
    assign grant_encoded = grant_encoded_reg;

    wire request_valid;
    wire [$clog2(PORTS)-1:0] request_index;
    wire [PORTS-1:0] request_mask;

    priority_encoder #(
        .WIDTH(PORTS),
        .LSB_HIGH_PRIORITY(ARB_LSB_HIGH_PRIORITY)
    ) priority_encoder_inst (
        .input_unencoded(request),
        .output_valid(request_valid),
        .output_encoded(request_index),
        .output_unencoded(request_mask)
    );

    reg [PORTS-1:0] mask_reg = 0, mask_next;

    wire masked_request_valid;
    wire [$clog2(PORTS)-1:0] masked_request_index;
    wire [PORTS-1:0] masked_request_mask;

    priority_encoder #(
        .WIDTH(PORTS),
        .LSB_HIGH_PRIORITY(ARB_LSB_HIGH_PRIORITY)
    ) priority_encoder_masked (
        .input_unencoded(request & mask_reg),
        .output_valid(masked_request_valid),
        .output_encoded(masked_request_index),
        .output_unencoded(masked_request_mask)
    );

    always @* begin
        grant_next = 0;
        grant_valid_next = 0;
        grant_encoded_next = 0;
        mask_next = mask_reg;

        if (ARB_BLOCK && !ARB_BLOCK_ACK && (grant_reg & request)) begin
            grant_valid_next = grant_valid_reg;
            grant_next = grant_reg;
            grant_encoded_next = grant_encoded_reg;
        end else if (ARB_BLOCK && ARB_BLOCK_ACK && grant_valid && !(grant_reg & acknowledge)) begin
            grant_valid_next = grant_valid_reg;
            grant_next = grant_reg;
            grant_encoded_next = grant_encoded_reg;
        end else if (request_valid) begin
            if (ARB_TYPE_ROUND_ROBIN) begin
                if (masked_request_valid) begin
                    grant_valid_next = 1;
                    grant_next = masked_request_mask;
                    grant_encoded_next = masked_request_index;
                    if (ARB_LSB_HIGH_PRIORITY) begin
                        mask_next = {PORTS{1'b1}} << (masked_request_index + 1);
                    end else begin
                        mask_next = {PORTS{1'b1}} >> (PORTS - masked_request_index);
                    end
                end else begin
                    grant_valid_next = 1;
                    grant_next = request_mask;
                    grant_encoded_next = request_index;
                    if (ARB_LSB_HIGH_PRIORITY) begin
                        mask_next = {PORTS{1'b1}} << (request_index + 1);
                    end else begin
                        mask_next = {PORTS{1'b1}} >> (PORTS - request_index);
                    end
                end
            end else begin
                grant_valid_next = 1;
                grant_next = request_mask;
                grant_encoded_next = request_index;
            end
        end
    end

    always @(posedge clk) begin
        if (rst) begin
            grant_reg <= 0;
            grant_valid_reg <= 0;
            grant_encoded_reg <= 0;
            mask_reg <= 0;
        end else begin
            grant_reg <= grant_next;
            grant_valid_reg <= grant_valid_next;
            grant_encoded_reg <= grant_encoded_next;
            mask_reg <= mask_next;
        end
    end
endmodule

module axi_interconnect #(
    parameter S_COUNT = 2,
    parameter M_COUNT = 2,
    parameter DATA_WIDTH = 8,
    parameter ADDR_WIDTH = 16,
    parameter STRB_WIDTH = (DATA_WIDTH/8),
    parameter ID_WIDTH = 2,
    parameter M_REGIONS = 1,
    parameter M_BASE_ADDR = 0,
    parameter M_ADDR_WIDTH = {M_COUNT{{M_REGIONS{32'd12}}}},
    parameter M_CONNECT_READ = {M_COUNT{{S_COUNT{1'b1}}}},
    parameter M_CONNECT_WRITE = {M_COUNT{{S_COUNT{1'b1}}}},
    parameter M_SECURE = {M_COUNT{1'b0}}
) (
    input  wire                          clk,
    input  wire                          rst,
    input  wire [S_COUNT*ADDR_WIDTH-1:0] s_axi_awaddr,
    input  wire [S_COUNT*3-1:0]          s_axi_awprot,
    input  wire [S_COUNT-1:0]            s_axi_awvalid,
    output wire [S_COUNT-1:0]            s_axi_awready,
    input  wire [S_COUNT-1:0]            s_axi_bvalid,
    input  wire [S_COUNT-1:0]            s_axi_bready,
    input  wire [S_COUNT*ADDR_WIDTH-1:0] s_axi_araddr,
    input  wire [S_COUNT*3-1:0]          s_axi_arprot,
    input  wire [S_COUNT-1:0]            s_axi_arvalid,
    output wire [S_COUNT-1:0]            s_axi_arready,
    input  wire [S_COUNT-1:0]            s_axi_rvalid,
    input  wire [S_COUNT-1:0]            s_axi_rready,
    input  wire [S_COUNT-1:0]            s_axi_rlast,
    output wire [M_COUNT-1:0]            m_axi_awvalid,
    input  wire [M_COUNT-1:0]            m_axi_awready,
    output wire [M_COUNT-1:0]            m_axi_arvalid,
    input  wire [M_COUNT-1:0]            m_axi_arready
);
    parameter CL_S_COUNT = $clog2(S_COUNT);
    parameter CL_M_COUNT = $clog2(M_COUNT);

    function [M_COUNT*M_REGIONS*ADDR_WIDTH-1:0] calcBaseAddrs(input [31:0] dummy);
        integer i;
        begin
            calcBaseAddrs = 0;
        end
    endfunction

    parameter M_BASE_ADDR_INT = M_BASE_ADDR ? M_BASE_ADDR : calcBaseAddrs(0);

    integer i, j;

    initial begin
        $display("Addressing configuration for axi_interconnect instance %m");
    end

    localparam [1:0]
        STATE_IDLE = 2'd0,
        STATE_DECODE = 2'd1,
        STATE_WRITE = 2'd2,
        STATE_READ = 2'd3;

    reg [1:0] state_reg = STATE_IDLE, state_next;
    reg match;
    reg [CL_M_COUNT-1:0] m_select_reg = 0, m_select_next;
    reg [ADDR_WIDTH-1:0] axi_addr_reg = 0, axi_addr_next;
    reg [2:0] axi_prot_reg = 0, axi_prot_next;
    reg [S_COUNT-1:0] s_axi_awready_reg = 0, s_axi_awready_next;
    reg [S_COUNT-1:0] s_axi_arready_reg = 0, s_axi_arready_next;
    reg [M_COUNT-1:0] m_axi_awvalid_reg = 0, m_axi_awvalid_next;
    reg [M_COUNT-1:0] m_axi_arvalid_reg = 0, m_axi_arvalid_next;

    assign s_axi_awready = s_axi_awready_reg;
    assign s_axi_arready = s_axi_arready_reg;
    assign m_axi_awvalid = m_axi_awvalid_reg;
    assign m_axi_arvalid = m_axi_arvalid_reg;

    wire [S_COUNT*2-1:0] request;
    wire [S_COUNT*2-1:0] acknowledge;
    wire [S_COUNT*2-1:0] grant;
    wire grant_valid;
    wire [CL_S_COUNT:0] grant_encoded;

    wire read = grant_encoded[0];
    wire [CL_S_COUNT-1:0] s_select = grant_encoded >> 1;

    arbiter #(
        .PORTS(S_COUNT*2),
        .ARB_TYPE_ROUND_ROBIN(1),
        .ARB_BLOCK(1),
        .ARB_BLOCK_ACK(1),
        .ARB_LSB_HIGH_PRIORITY(1)
    ) arb_inst (
        .clk(clk),
        .rst(rst),
        .request(request),
        .acknowledge(acknowledge),
        .grant(grant),
        .grant_valid(grant_valid),
        .grant_encoded(grant_encoded)
    );

    genvar n;
    generate
        for (n = 0; n < S_COUNT; n = n + 1) begin
            assign request[2*n]   = s_axi_awvalid[n];
            assign request[2*n+1] = s_axi_arvalid[n];
        end
    endgenerate

    generate
        for (n = 0; n < S_COUNT; n = n + 1) begin
            assign acknowledge[2*n]   = grant[2*n]   && s_axi_bvalid[n] && s_axi_bready[n];
            assign acknowledge[2*n+1] = grant[2*n+1] && s_axi_rvalid[n] && s_axi_rready[n] && s_axi_rlast[n];
        end
    endgenerate

    always @* begin
        state_next = state_reg;
        match = 1'b0;
        m_select_next = m_select_reg;
        axi_addr_next = axi_addr_reg;
        axi_prot_next = axi_prot_reg;
        s_axi_awready_next = 0;
        s_axi_arready_next = 0;
        m_axi_awvalid_next = m_axi_awvalid_reg & ~m_axi_awready;
        m_axi_arvalid_next = m_axi_arvalid_reg & ~m_axi_arready;

        case (state_reg)
            STATE_IDLE: begin
                if (grant_valid) begin
                    if (read) begin
                        axi_addr_next = s_axi_araddr[s_select*ADDR_WIDTH +: ADDR_WIDTH];
                        axi_prot_next = s_axi_arprot[s_select*3 +: 3];
                        s_axi_arready_next[s_select] = 1'b1;
                    end else begin
                        axi_addr_next = s_axi_awaddr[s_select*ADDR_WIDTH +: ADDR_WIDTH];
                        axi_prot_next = s_axi_awprot[s_select*3 +: 3];
                        s_axi_awready_next[s_select] = 1'b1;
                    end
                    state_next = STATE_DECODE;
                end
            end
            STATE_DECODE: begin
                match = 1'b0;
                for (i = 0; i < M_COUNT; i = i + 1) begin
                    for (j = 0; j < M_REGIONS; j = j + 1) begin
                        if (M_ADDR_WIDTH[(i*M_REGIONS+j)*32 +: 32] &&
                            (!M_SECURE[i] || !axi_prot_reg[1]) &&
                            (axi_addr_reg[ADDR_WIDTH-1:12] ==
                             M_BASE_ADDR_INT[(i*M_REGIONS+j)*ADDR_WIDTH + 12 +: (ADDR_WIDTH-12)])) begin
                            m_select_next = i;
                            match = 1'b1;
                        end
                    end
                end
                if (match) begin
                    if (read) begin
                        m_axi_arvalid_next[m_select_next] = 1'b1;
                        state_next = STATE_READ;
                    end else begin
                        m_axi_awvalid_next[m_select_next] = 1'b1;
                        state_next = STATE_WRITE;
                    end
                end else begin
                    state_next = STATE_IDLE;
                end
            end
            default: begin
                state_next = STATE_IDLE;
            end
        endcase
    end

    always @(posedge clk) begin
        if (rst) begin
            state_reg <= STATE_IDLE;
            s_axi_awready_reg <= 0;
            s_axi_arready_reg <= 0;
            m_axi_awvalid_reg <= 0;
            m_axi_arvalid_reg <= 0;
        end else begin
            state_reg <= state_next;
            s_axi_awready_reg <= s_axi_awready_next;
            s_axi_arready_reg <= s_axi_arready_next;
            m_axi_awvalid_reg <= m_axi_awvalid_next;
            m_axi_arvalid_reg <= m_axi_arvalid_next;
        end
        m_select_reg <= m_select_next;
        axi_addr_reg <= axi_addr_next;
        axi_prot_reg <= axi_prot_next;
    end
endmodule
"#;
    let d = synth_sv(sv, "axi_interconnect_full.v").expect("synth full axi_interconnect");
    assert_ne!(
        d.attrs.get("NO_BODY"),
        Some("1"),
        "full axi_interconnect must not be NO_BODY"
    );
    assert_ne!(
        d.attrs.get("SIM_ONLY"),
        Some("1"),
        "initial-only $display must not force SIM_ONLY on axi_interconnect"
    );
    assert_ne!(
        d.attrs.get("GENERATE_NOT_LOWERED"),
        Some("1"),
        "full axi_interconnect must not set GENERATE_NOT_LOWERED"
    );
    assert!(
        d.cells.len() > 20,
        "expected substantial synthesized cells for axi_interconnect+arbiter+priority_encoder, got {}",
        d.cells.len()
    );
}

#[test]
fn multidim_3d_slices_descending_bounds_and_tree_localparams() {
    let sv = r#"
module tree_and_3d #(
    parameter M = 2,
    parameter S = 2,
    parameter W = 8,
    parameter NUM_LEVELS = 2
) (
    input  wire [M*S*W-1:0] flat_in,
    input  wire [3:0]       oh_i,
    output wire [M*S*4-1:0] low_nibbles,
    output wire             tree_or
);
    wire [W-1:0] cube [0:M-1][0:S-1];
    wire [2**(NUM_LEVELS+1)-2:0] or_tree;

    genvar i, j, level, offset;
    generate
        for (i = M - 1; i >= 0; i = i - 1) begin : gen_row
            for (j = 0; j != S; j += 1) begin : gen_col
                localparam int BASE = (i * S + j) * W;
                assign cube[i][j] = flat_in[BASE +: W];
                assign low_nibbles[(i * S + j) * 4 +: 4] =
                    cube[i][j][3:0] ^ cube[i][j][4 +: 4];
            end : gen_col
        end : gen_row

        for (level = 0; level < NUM_LEVELS + 1; level++) begin : gen_tree
            localparam int Base0 = (2**level) - 1;
            localparam int Base1 = (2**(level+1)) - 1;
            for (offset = 0; offset < 2**level; offset++) begin : gen_level
                localparam int Pa = Base0 + offset;
                localparam int C0 = Base1 + 2 * offset;
                localparam int C1 = Base1 + 2 * offset + 1;
                if (level == NUM_LEVELS) begin : gen_leaf
                    assign or_tree[Pa] = oh_i[offset];
                end else begin : gen_node
                    assign or_tree[Pa] = or_tree[C0] | or_tree[C1];
                end
            end : gen_level
        end : gen_tree
    endgenerate

    assign tree_or = or_tree[0];
endmodule
"#;
    let d = synth_sv(sv, "tree_and_3d.sv").expect("synth tree_and_3d");
    assert_ne!(d.attrs.get("NO_BODY"), Some("1"));
    assert_ne!(d.attrs.get("GENERATE_NOT_LOWERED"), Some("1"));
    let luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    assert!(
        luts >= 16,
        "expected 16 nibble XOR LUTs + tree OR LUT, got {luts}"
    );
}
