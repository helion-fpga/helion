// Helion-MM timer — small loadable down-counter (not AXI timer).
// Kept narrow (4-bit) so Helion maps real LUT/FF fabric.
module h_timer (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    output wire        irq_zero
);
    reg [3:0] count_r;
    reg [3:0] load_r;
    reg       en_r;
    reg       zero_r;

    assign mm_ready = 1'b1;
    assign irq_zero = zero_r;

    always @(posedge clk) begin
        if (!resetn) begin
            count_r  <= 4'h0;
            load_r   <= 4'h0;
            en_r     <= 1'b0;
            zero_r   <= 1'b0;
            mm_rdata <= 32'h0;
        end else begin
            if (mm_valid && mm_write && (mm_addr[3:0] == 4'h0)) begin
                load_r  <= mm_wdata[3:0];
                count_r <= mm_wdata[3:0];
                zero_r  <= 1'b0;
            end else if (mm_valid && mm_write && (mm_addr[3:0] == 4'h4)) begin
                en_r <= mm_wdata[0];
            end else if (mm_valid && mm_write && (mm_addr[3:0] == 4'h8)) begin
                if (mm_wdata[0]) zero_r <= 1'b0;
            end else if (en_r) begin
                if (count_r == 4'h0) begin
                    zero_r  <= 1'b1;
                    count_r <= load_r;
                end else begin
                    count_r <= count_r - 4'h1;
                end
            end

            if (mm_valid && !mm_write) begin
                if (mm_addr[3:0] == 4'h0)
                    mm_rdata <= {28'h0, count_r};
                else if (mm_addr[3:0] == 4'h4)
                    mm_rdata <= {31'h0, en_r};
                else if (mm_addr[3:0] == 4'h8)
                    mm_rdata <= {31'h0, zero_r};
                else
                    mm_rdata <= 32'h0;
            end
        end
    end
endmodule
