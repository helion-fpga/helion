// Interface between the UVM environment and examples/counter.sv.
// The clocking block samples `led` just before each rising clock edge, so
// the monitor sees the value the DUT held during the previous cycle.
interface counter_if (input logic clk);
  logic led;

  clocking cb @(posedge clk);
    input led;
  endclocking

  modport mon (clocking cb);
endinterface
