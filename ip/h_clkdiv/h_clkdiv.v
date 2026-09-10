// Helion-ST clock divider — loadable divisor, enable, toggling out (not AXI).
// Divisor loaded via Helion-ST (st_valid + st_data). Counter counts to
// divisor then toggles clk_out. Real FF+LUT fabric for catalog proof.
module h_clkdiv (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST load path (ingress): write new divisor when st_valid
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire       enable,
    output reg        clk_out
);
    reg [7:0] div_r;
    reg [7:0] count_r;

    assign st_ready = 1'b1;

    always @(posedge clk) begin
        if (!resetn) begin
            div_r   <= 8'h0F;
            count_r <= 8'h00;
            clk_out <= 1'b0;
        end else begin
            if (st_valid) begin
                // Zero divisor treated as 1 to keep toggle well-defined
                div_r   <= (st_data == 8'h00) ? 8'h01 : st_data;
                count_r <= 8'h00;
            end else if (enable) begin
                if (count_r >= div_r) begin
                    count_r <= 8'h00;
                    clk_out <= ~clk_out;
                end else begin
                    count_r <= count_r + 8'h01;
                end
            end
        end
    end
endmodule
