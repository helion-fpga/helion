// UVM testbench top for examples/counter.sv (found in the parent folder).
// Run in the IDE's testbench runner, or pick a test with
// +UVM_TESTNAME=counter_long_test.
`include "uvm_macros.svh"
`include "counter_if.sv"
`include "counter_pkg.sv"

module uvm_counter_tb;
  import uvm_pkg::*;
  import counter_pkg::*;

  logic clk = 0;
  always #5 clk = ~clk;

  counter_if cif (clk);
  counter dut (.clk(clk), .led(cif.led));

  initial begin
    uvm_config_db #(virtual counter_if)::set(null, "uvm_test_top.*", "vif", cif);
    run_test("counter_test");
  end
endmodule
