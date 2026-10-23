// t05_shift_chain: a register pipeline with real inter-FF routing.
// Isolates: FF-to-FF routing across several tiles and the clock spine feeding
// more than one FF. blinky has 27 FFs but they form one counter; a shift chain
// forces distinct, non-adjacent sites and exercises the routing graph.
// Pass: the lit LED walks LD0 -> LD7 and wraps, at about 100 MHz / 2^23.
module t05_shift_chain (
    input  logic clk,
    output logic [7:0] led
);
    logic [22:0] cnt;
    logic [7:0]  sr;

    always_ff @(posedge clk) begin
        cnt <= cnt + 23'b1;
        if (cnt == 23'b0) sr <= 8'b1;
        else              sr <= {sr[6:0], sr[7]};
    end

    assign led = sr;
endmodule
