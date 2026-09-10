// Helion-ST byte-serial CRC32 — init/enable/data_valid (not AXI).
// Ethernet poly 0x04C11DB7, bit-serial MSB-first, one bit per enable tick
// after a byte is accepted on Helion-ST. Real FF+LUT fabric for catalog proof.
module h_crc32 (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST data byte ingress
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire        init,        // load CRC to all-ones
    input  wire        enable,      // advance one bit when shift pending
    output reg  [31:0] crc,
    output wire        busy,
    output wire        st_out_valid
);
    localparam [31:0] POLY = 32'h04C11DB7;

    reg [7:0]  byte_r;
    reg [3:0]  bit_left; // 0 = idle; 1..8 = bits remaining
    reg        out_v;

    assign st_ready     = (bit_left == 4'h0);
    assign busy         = (bit_left != 4'h0);
    assign st_out_valid = out_v;

    wire msb = crc[31];
    wire din = byte_r[7];

    always @(posedge clk) begin
        if (!resetn) begin
            crc      <= 32'hFFFF_FFFF;
            byte_r   <= 8'h00;
            bit_left <= 4'h0;
            out_v    <= 1'b0;
        end else begin
            out_v <= 1'b0;

            if (init) begin
                crc      <= 32'hFFFF_FFFF;
                bit_left <= 4'h0;
            end else if (st_valid && (bit_left == 4'h0)) begin
                byte_r   <= st_data;
                bit_left <= 4'h8;
            end else if (enable && (bit_left != 4'h0)) begin
                // One bit of CRC32 update
                if (msb ^ din)
                    crc <= {crc[30:0], 1'b0} ^ POLY;
                else
                    crc <= {crc[30:0], 1'b0};
                byte_r   <= {byte_r[6:0], 1'b0};
                bit_left <= bit_left - 4'h1;
                if (bit_left == 4'h1) begin
                    out_v <= 1'b1;
                end
            end
        end
    end
endmodule
