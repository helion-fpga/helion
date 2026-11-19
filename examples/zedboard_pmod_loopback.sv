// ZedBoard Pmod loopback check for the Helion 7-series backend.
// Top: zedboard_pmod_loopback. Pins are in examples/zedboard_pmod_loopback.xdc.
//
// Hardware setup: eight jumper wires from Pmod JA to Pmod JB, pin for pin
// (JA1 to JB1, JA2 to JB2, JA3 to JB3, JA4 to JB4, JA7 to JB7, JA8 to JB8,
// JA9 to JB9, JA10 to JB10). The design drives a repeating test pattern
// out of JA, waits for the signals to settle, samples JB, and compares.
// A mismatch latches the error flag and increments the error counter.
//
// Pattern sequence (one step about every 1 ms at 100 MHz, slow enough for
// jumper wires and for a scope to follow): walking 1, walking 0, 0x55,
// 0xAA, then an incrementing counter value. LED report:
//   led[0] pass so far (1 while no error has ever been seen)
//   led[1] error latched (1 once any mismatch was seen)
//   led[7:2] low six bits of the mismatch count
//
// No simulator was installed on this box (no verilator, no iverilog),
// so no testbench is shipped with this file; see docs/zedboard/rung5-summary.json.
module zedboard_pmod_loopback (
    input  logic       clk,
    output logic [7:0] ja,
    input  logic [7:0] jb,
    output logic [7:0] led
);
    localparam int unsigned SETTLE = 100_000; // 1 ms at 100 MHz

    logic [17:0] settle_cnt;
    logic [2:0]  pat_step;
    logic [7:0]  pattern;
    logic [7:0]  counter_pat;
    logic [5:0]  err_cnt;
    logic        err_seen;

    // Pattern generator: step through the five test patterns.
    always_comb begin
        case (pat_step)
            3'd0:    pattern = 8'b0000_0001; // walking 1, bit 0
            3'd1:    pattern = 8'b1000_0000; // walking 1, bit 7
            3'd2:    pattern = 8'b1111_1110; // walking 0, bit 0
            3'd3:    pattern = 8'b0111_1111; // walking 0, bit 7
            3'd4:    pattern = 8'h55;
            3'd5:    pattern = 8'haa;
            default: pattern = counter_pat;  // incrementing counter
        endcase
    end

    assign ja = pattern;

    always_ff @(posedge clk) begin
        if (settle_cnt == SETTLE[17:0]) begin
            settle_cnt <= 18'd0;
            // Sample JB after the settle time and compare with JA.
            if (jb != pattern) begin
                err_seen <= 1'b1;
                err_cnt  <= err_cnt + 6'd1;
            end
            if (pat_step == 3'd7) begin
                pat_step    <= 3'd0;
                counter_pat <= counter_pat + 8'd1;
            end else begin
                pat_step <= pat_step + 3'd1;
            end
        end else begin
            settle_cnt <= settle_cnt + 18'd1;
        end
    end

    assign led[0]   = ~err_seen;
    assign led[1]   = err_seen;
    assign led[7:2] = err_cnt;
endmodule
