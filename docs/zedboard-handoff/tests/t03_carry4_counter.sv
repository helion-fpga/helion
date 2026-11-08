// t03_carry4_counter: CARRY4 carry-chain test. 26-bit incrementer.
// Isolates: CARRY4 chain configuration (DI/S muxes, CYINIT, CI chaining).
// A 26-bit +1 incrementer needs 7 chained CARRY4s. If any link in the
// chain is misconfigured, the count sequence breaks and the LED pattern
// will be wrong (stuck bits, wrong rates, or no blinking at all).
// PASS: LD0 blinks at ~1.5 Hz (100 MHz / 2^26), LD1..LD5 blink
// progressively faster, like the blinky cascade.
// FAIL: LEDs stuck, wrong blink rates, or no activity.
// (This is the same structure as zedboard_blinky; that design IS the
// carry-chain test. Kept as a named isolated test for the harness.)
module t03_carry4_counter (
    input  logic       clk,
    output logic [7:0] led
);
    logic [25:0] cnt;
    always_ff @(posedge clk) begin
        cnt <= cnt + 26'd1;
    end
    assign led[0] = cnt[25];
    assign led[1] = cnt[24];
    assign led[2] = cnt[23];
    assign led[3] = cnt[22];
    assign led[4] = cnt[21];
    assign led[5] = cnt[20];
    assign led[6] = cnt[19];
    assign led[7] = cnt[18];
endmodule
