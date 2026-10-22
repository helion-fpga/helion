// ZedBoard blinky for the Helion 7-series backend.
// Top: zedboard_blinky. Pins are in examples/zedboard.xdc.
// 100 MHz GCLK drives a free-running counter. LED 0 blinks at about 1 Hz
// via counter bit select; LED 7..1 tap higher counter bits so every LED
// shows activity. Any push button press resets the counter synchronously.
module zedboard_blinky (
    input  logic       clk,
    input  logic [4:0] btn,
    output logic [7:0] led
);
    logic [26:0] cnt;

    always_ff @(posedge clk) begin
        if (|btn)
            cnt <= 27'b0;
        else
            cnt <= cnt + 27'b1;
    end

    // cnt[26] toggles every 2^26 cycles: 100 MHz / 2^27 is 0.745 Hz.
    assign led[0] = cnt[26];
    assign led[1] = cnt[25];
    assign led[2] = cnt[24];
    assign led[3] = cnt[23];
    assign led[4] = cnt[22];
    assign led[5] = cnt[21];
    assign led[6] = cnt[20];
    assign led[7] = cnt[19];
endmodule
