// Helion-MM scratch register file — 4×32b with write/read strobes (not AXI).
// Addr[3:2] selects bank 0..3; mm_write + mm_valid stores wdata; read returns
// registered rdata. Real FF+LUT fabric for catalog proof.
module h_scratch_mm (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    output wire        wr_strobe,
    output wire        rd_strobe
);
    reg [31:0] bank0;
    reg [31:0] bank1;
    reg [31:0] bank2;
    reg [31:0] bank3;
    reg        wr_r;
    reg        rd_r;

    assign mm_ready  = 1'b1;
    assign wr_strobe = wr_r;
    assign rd_strobe = rd_r;

    wire [1:0] sel = mm_addr[3:2];

    always @(posedge clk) begin
        if (!resetn) begin
            bank0    <= 32'h0;
            bank1    <= 32'h0;
            bank2    <= 32'h0;
            bank3    <= 32'h0;
            mm_rdata <= 32'h0;
            wr_r     <= 1'b0;
            rd_r     <= 1'b0;
        end else begin
            wr_r <= 1'b0;
            rd_r <= 1'b0;

            if (mm_valid && mm_write) begin
                wr_r <= 1'b1;
                case (sel)
                    2'b00: bank0 <= mm_wdata;
                    2'b01: bank1 <= mm_wdata;
                    2'b10: bank2 <= mm_wdata;
                    2'b11: bank3 <= mm_wdata;
                endcase
            end

            if (mm_valid && !mm_write) begin
                rd_r <= 1'b1;
                case (sel)
                    2'b00: mm_rdata <= bank0;
                    2'b01: mm_rdata <= bank1;
                    2'b10: mm_rdata <= bank2;
                    2'b11: mm_rdata <= bank3;
                endcase
            end
        end
    end
endmodule
