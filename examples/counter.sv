// Helion 1.0 SV counter — 4-bit incrementer, LED = cnt[3]
module counter (
    input  logic clk,
    output logic led
);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assign led = cnt[3];
endmodule
