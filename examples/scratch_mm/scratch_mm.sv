// Helion lab: 4×4 Helion-MM scratch. Internal host writes; LED = selected bit.
// Helion-MM (mm_valid / mm_write / mm_addr / mm_wdata / mm_rdata). Not AXI.
module scratch_bank (
    input  logic       clk,
    input  logic       mm_valid,
    input  logic       mm_write,
    input  logic [1:0] mm_addr,
    input  logic [3:0] mm_wdata,
    output logic [3:0] mm_rdata
);
    logic [3:0] b0;
    logic [3:0] b1;
    logic [3:0] b2;
    logic [3:0] b3;
    always_ff @(posedge clk) begin
        if (mm_valid && mm_write) begin
            if (mm_addr == 2'b00) b0 <= mm_wdata;
            if (mm_addr == 2'b01) b1 <= mm_wdata;
            if (mm_addr == 2'b10) b2 <= mm_wdata;
            if (mm_addr == 2'b11) b3 <= mm_wdata;
        end
    end
    assign mm_rdata =
        (mm_addr == 2'b00) ? b0 :
        (mm_addr == 2'b01) ? b1 :
        (mm_addr == 2'b10) ? b2 : b3;
endmodule

module scratch_mm (
    input  logic clk,
    output logic led
);
    logic [1:0] phase;
    logic       mm_valid;
    logic       mm_write;
    logic [1:0] mm_addr;
    logic [3:0] mm_wdata;
    logic [3:0] mm_rdata;

    always_ff @(posedge clk) begin
        phase <= phase + 1;
        mm_valid <= 1'b1;
        mm_write <= 1'b1;
        if (phase == 2'd0) begin
            mm_addr  <= 2'b00;
            mm_wdata <= 4'hA;
        end
        if (phase == 2'd1) begin
            mm_addr  <= 2'b01;
            mm_wdata <= 4'h5;
        end
        if (phase == 2'd2) begin
            mm_addr  <= 2'b10;
            mm_wdata <= 4'h3;
        end
        if (phase == 2'd3) begin
            mm_addr  <= 2'b11;
            mm_wdata <= 4'hC;
        end
    end

    scratch_bank u_bank (
        .clk(clk),
        .mm_valid(mm_valid),
        .mm_write(mm_write),
        .mm_addr(mm_addr),
        .mm_wdata(mm_wdata),
        .mm_rdata(mm_rdata)
    );
    assign led = mm_rdata[1];
endmodule
