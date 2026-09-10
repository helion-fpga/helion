// Helion-MM GPIO — registered data + direction (not AXI GPIO).
module h_gpio (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    input  wire [7:0]  gpio_in,
    output wire [7:0]  gpio_out
);
    // addr 0x00: data out; 0x04: direction (1=out); 0x08: read pin sample
    reg [7:0] data_r;
    reg [7:0] dir_r;
    reg [7:0] in_r;

    assign mm_ready = 1'b1;
    assign gpio_out = data_r & dir_r;

    always @(posedge clk) begin
        if (!resetn) begin
            data_r   <= 8'h00;
            dir_r    <= 8'h00;
            in_r     <= 8'h00;
            mm_rdata <= 32'h0;
        end else begin
            in_r <= gpio_in;
            if (mm_valid && mm_write) begin
                case (mm_addr[3:0])
                    4'h0: data_r <= mm_wdata[7:0];
                    4'h4: dir_r  <= mm_wdata[7:0];
                    default: ;
                endcase
            end
            if (mm_valid && !mm_write) begin
                case (mm_addr[3:0])
                    4'h0: mm_rdata <= {24'h0, data_r};
                    4'h4: mm_rdata <= {24'h0, dir_r};
                    4'h8: mm_rdata <= {24'h0, in_r};
                    default: mm_rdata <= 32'h0;
                endcase
            end
        end
    end
endmodule
