// Helion lab: minimal 8N1 UART TX. Idle framing, auto-sends 0xA5.
// Helion-legal serial. Not AXI UART. Not a vendor primitive.
module uart (
    input  logic clk,
    output logic tx
);
    logic [3:0] idx;
    always_ff @(posedge clk) begin
        if (idx == 4'd9)
            idx <= 4'd0;
        else
            idx <= idx + 1;
    end
    // 8N1, LSB first: start, d0..d7 of 0xA5 (8'hA5 = 1010_0101), stop.
    assign tx =
        (idx == 4'd0) ? 1'b0 :
        (idx == 4'd1) ? 1'b1 :
        (idx == 4'd2) ? 1'b0 :
        (idx == 4'd3) ? 1'b1 :
        (idx == 4'd4) ? 1'b0 :
        (idx == 4'd5) ? 1'b0 :
        (idx == 4'd6) ? 1'b1 :
        (idx == 4'd7) ? 1'b0 :
        (idx == 4'd8) ? 1'b1 :
                        1'b1;
endmodule
