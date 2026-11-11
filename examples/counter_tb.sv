// Self-checking testbench for counter.sv (Helion testbench-runner subset).
// The 4-bit counter wraps every 16 clocks; led mirrors cnt[3], so it must
// stay low for the first 8 rising edges and be high after them.
module counter_tb;
    logic clk = 0;
    logic led;

    counter dut (.clk(clk), .led(led));

    always #5 clk = ~clk;

    initial begin
        $display("counter_tb: reset released, checking led");
        repeat (7) @(posedge clk);
        #1;
        assert (led == 0) else $error("led rose early: led=%0d", led);
        @(posedge clk);
        #1;
        assert (led == 1) else $error("led did not rise after 8 clocks: led=%0d", led);
        $display("led=%0d at %t", led, $time);
        $finish;
    end
endmodule
