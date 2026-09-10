// Helion-ST registered CRC-8 — poly fixed 0x07 (CRC-8-ATM), byte-serial (not AXI).
// Accepts Helion-ST data bytes; advances one bit per enable when shift pending.
// Real FF+LUT fabric for catalog proof.
module h_crc8 (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST data byte ingress
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire       init,        // load CRC to 0x00
    input  wire       enable,      // advance one bit when shift pending
    output reg  [7:0] crc,
    output wire       busy,
    output wire       st_out_valid
);
    localparam [7:0] POLY = 8'h07;

    reg [7:0] byte_r;
    reg [3:0] bit_left; // 0 = idle; 1..8 = bits remaining
    reg       out_v;

    assign st_ready     = (bit_left == 4'h0);
    assign busy         = (bit_left != 4'h0);
    assign st_out_valid = out_v;

    wire msb = crc[7];
    wire din = byte_r[7];

    always @(posedge clk) begin
        if (!resetn) begin
            crc      <= 8'h00;
            byte_r   <= 8'h00;
            bit_left <= 4'h0;
            out_v    <= 1'b0;
        end else begin
            out_v <= 1'b0;

            if (init) begin
                crc      <= 8'h00;
                bit_left <= 4'h0;
            end else if (st_valid && (bit_left == 4'h0)) begin
                byte_r   <= st_data;
                bit_left <= 4'h8;
            end else if (enable && (bit_left != 4'h0)) begin
                if (msb ^ din)
                    crc <= {crc[6:0], 1'b0} ^ POLY;
                else
                    crc <= {crc[6:0], 1'b0};
                byte_r   <= {byte_r[6:0], 1'b0};
                bit_left <= bit_left - 4'h1;
                if (bit_left == 4'h1)
                    out_v <= 1'b1;
            end
        end
    end
endmodule
