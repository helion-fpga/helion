// Helion-MM watchdog — loadable countdown, enable, timeout sticky, pet/kick (not AXI).
// Reg map: 0x00 load/count, 0x04 ctrl (en), 0x08 status/clear timeout, 0x0C pet (any write).
// Width kept modest so Helion maps real LUT/FF fabric.
module h_watchdog (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    output wire        irq_timeout
);
    reg [7:0] count_r;
    reg [7:0] load_r;
    reg       en_r;
    reg       timeout_r;

    assign mm_ready    = 1'b1;
    assign irq_timeout = timeout_r;

    always @(posedge clk) begin
        if (!resetn) begin
            count_r    <= 8'hFF;
            load_r     <= 8'hFF;
            en_r       <= 1'b0;
            timeout_r  <= 1'b0;
            mm_rdata   <= 32'h0;
        end else begin
            if (mm_valid && mm_write) begin
                case (mm_addr[3:0])
                    4'h0: begin
                        load_r  <= (mm_wdata[7:0] == 8'h0) ? 8'h01 : mm_wdata[7:0];
                        count_r <= (mm_wdata[7:0] == 8'h0) ? 8'h01 : mm_wdata[7:0];
                    end
                    4'h4: en_r <= mm_wdata[0];
                    4'h8: if (mm_wdata[0]) timeout_r <= 1'b0;
                    4'hC: begin
                        // Pet / kick: reload countdown from load
                        count_r <= load_r;
                    end
                    default: ;
                endcase
            end

            if (mm_valid && !mm_write) begin
                case (mm_addr[3:0])
                    4'h0: mm_rdata <= {24'h0, count_r};
                    4'h4: mm_rdata <= {31'h0, en_r};
                    4'h8: mm_rdata <= {31'h0, timeout_r};
                    4'hC: mm_rdata <= {24'h0, load_r};
                    default: mm_rdata <= 32'h0;
                endcase
            end

            // Countdown when enabled and not already timed out
            if (en_r && !timeout_r) begin
                if (count_r == 8'h0) begin
                    timeout_r <= 1'b1;
                end else begin
                    count_r <= count_r - 8'h01;
                end
            end
        end
    end
endmodule
