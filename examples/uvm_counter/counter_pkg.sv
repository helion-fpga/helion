// UVM environment for examples/counter.sv (a free-running 4-bit counter
// whose `led` output is cnt[3]). The DUT has no data inputs, so sequence
// items describe how many clock cycles the driver lets the counter run;
// the monitor publishes one sample per clock and the scoreboard checks it
// against a cycle-accurate reference model.
package counter_pkg;
  import uvm_pkg::*;
  `include "uvm_macros.svh"

  // ---------------------------------------------------------------------
  // Transactions
  // ---------------------------------------------------------------------
  class counter_item extends uvm_sequence_item;
    rand int unsigned cycles;
    constraint c_cycles { cycles inside {[1:12]}; }

    `uvm_object_utils_begin(counter_item)
      `uvm_field_int(cycles, UVM_ALL_ON | UVM_DEC)
    `uvm_object_utils_end

    function new(string name = "counter_item");
      super.new(name);
    endfunction

    virtual function string convert2string();
      return $sformatf("run %0d cycle(s)", cycles);
    endfunction
  endclass

  class led_sample extends uvm_sequence_item;
    int unsigned cycle;
    bit led;
    `uvm_object_utils(led_sample)
    function new(string name = "led_sample");
      super.new(name);
    endfunction
  endclass

  // ---------------------------------------------------------------------
  // Sequences
  // ---------------------------------------------------------------------
  class counter_seq extends uvm_sequence #(counter_item);
    `uvm_object_utils(counter_seq)
    int unsigned n_items = 6;

    function new(string name = "counter_seq");
      super.new(name);
    endfunction

    virtual task body();
      repeat (n_items) begin
        req = counter_item::type_id::create("req");
        start_item(req);
        if (!req.randomize()) `uvm_error("RAND", "counter_item randomization failed")
        finish_item(req);
      end
    endtask
  endclass

  // Longer run with a fixed cycle count per item (used via a factory override).
  class counter_long_seq extends counter_seq;
    `uvm_object_utils(counter_long_seq)
    function new(string name = "counter_long_seq");
      super.new(name);
      n_items = 4;
    endfunction
    virtual task body();
      repeat (n_items) `uvm_do_with(req, { cycles == 10; })
    endtask
  endclass

  typedef uvm_sequencer #(counter_item) counter_sequencer;

  // ---------------------------------------------------------------------
  // Agent components
  // ---------------------------------------------------------------------
  class counter_driver extends uvm_driver #(counter_item);
    `uvm_component_utils(counter_driver)
    virtual counter_if vif;
    int unsigned total_cycles;

    function new(string name, uvm_component parent);
      super.new(name, parent);
    endfunction

    virtual function void build_phase(uvm_phase phase);
      super.build_phase(phase);
      if (!uvm_config_db #(virtual counter_if)::get(this, "", "vif", vif))
        `uvm_fatal("NOVIF", "virtual interface `vif` was not set in uvm_config_db")
    endfunction

    virtual task run_phase(uvm_phase phase);
      forever begin
        seq_item_port.get_next_item(req);
        `uvm_info("DRV", req.convert2string(), UVM_HIGH)
        repeat (req.cycles) @(vif.cb);
        total_cycles += req.cycles;
        seq_item_port.item_done();
      end
    endtask
  endclass

  class counter_monitor extends uvm_monitor;
    `uvm_component_utils(counter_monitor)
    virtual counter_if vif;
    uvm_analysis_port #(led_sample) ap;

    function new(string name, uvm_component parent);
      super.new(name, parent);
      ap = new("ap", this);
    endfunction

    virtual function void build_phase(uvm_phase phase);
      if (!uvm_config_db #(virtual counter_if)::get(this, "", "vif", vif))
        `uvm_fatal("NOVIF", "virtual interface `vif` was not set in uvm_config_db")
    endfunction

    virtual task run_phase(uvm_phase phase);
      int unsigned n;
      forever begin
        led_sample s;
        @(vif.cb);
        n++;
        s = led_sample::type_id::create("s");
        s.cycle = n;
        s.led = vif.cb.led;
        ap.write(s);
      end
    endtask
  endclass

  class counter_agent extends uvm_agent;
    `uvm_component_utils(counter_agent)
    counter_sequencer sqr;
    counter_driver drv;
    counter_monitor mon;

    function new(string name, uvm_component parent);
      super.new(name, parent);
    endfunction

    virtual function void build_phase(uvm_phase phase);
      mon = counter_monitor::type_id::create("mon", this);
      if (get_is_active() == UVM_ACTIVE) begin
        sqr = counter_sequencer::type_id::create("sqr", this);
        drv = counter_driver::type_id::create("drv", this);
      end
    endfunction

    virtual function void connect_phase(uvm_phase phase);
      if (get_is_active() == UVM_ACTIVE) drv.seq_item_port.connect(sqr.seq_item_export);
    endfunction
  endclass

  // ---------------------------------------------------------------------
  // Scoreboard: reference model of the 4-bit counter.
  // ---------------------------------------------------------------------
  class counter_scoreboard extends uvm_scoreboard;
    `uvm_component_utils(counter_scoreboard)
    uvm_analysis_imp #(led_sample, counter_scoreboard) analysis_export;
    bit [3:0] model_cnt;
    int unsigned matches, mismatches;

    function new(string name, uvm_component parent);
      super.new(name, parent);
      analysis_export = new("analysis_export", this);
    endfunction

    // Sample N is taken just before rising edge N, i.e. after N-1 edges.
    virtual function void write(led_sample s);
      bit expected = model_cnt[3];
      if (s.led !== expected) begin
        mismatches++;
        `uvm_error("SCB", $sformatf("cycle %0d: led=%0b, expected %0b (model count %0d)", s.cycle, s.led, expected, model_cnt))
      end else begin
        matches++;
        `uvm_info("SCB", $sformatf("cycle %0d: led=%0b ok", s.cycle, s.led), UVM_HIGH)
      end
      model_cnt++;
    endfunction

    virtual function void check_phase(uvm_phase phase);
      if (matches + mismatches == 0) `uvm_error("SCB", "no samples were checked")
    endfunction

    virtual function void report_phase(uvm_phase phase);
      `uvm_info("SCB", $sformatf("%0d sample(s) checked, %0d mismatch(es)", matches + mismatches, mismatches), UVM_LOW)
    endfunction
  endclass

  class counter_env extends uvm_env;
    `uvm_component_utils(counter_env)
    counter_agent agent;
    counter_scoreboard scb;

    function new(string name, uvm_component parent);
      super.new(name, parent);
    endfunction

    virtual function void build_phase(uvm_phase phase);
      agent = counter_agent::type_id::create("agent", this);
      scb = counter_scoreboard::type_id::create("scb", this);
    endfunction

    virtual function void connect_phase(uvm_phase phase);
      agent.mon.ap.connect(scb.analysis_export);
    endfunction
  endclass

  // ---------------------------------------------------------------------
  // Tests
  // ---------------------------------------------------------------------
  class counter_test extends uvm_test;
    `uvm_component_utils(counter_test)
    counter_env env;

    function new(string name, uvm_component parent);
      super.new(name, parent);
    endfunction

    virtual function void build_phase(uvm_phase phase);
      env = counter_env::type_id::create("env", this);
    endfunction

    virtual function void end_of_elaboration_phase(uvm_phase phase);
      uvm_top.print_topology();
    endfunction

    virtual task run_phase(uvm_phase phase);
      counter_seq seq;
      phase.raise_objection(this, "running counter_seq");
      seq = counter_seq::type_id::create("seq");
      seq.start(env.agent.sqr);
      phase.drop_objection(this);
    endtask

    virtual function void report_phase(uvm_phase phase);
      `uvm_info("TEST", $sformatf("driver ran %0d cycle(s)", env.agent.drv.total_cycles), UVM_LOW)
    endfunction
  endclass

  // Same environment; the factory swaps in the long sequence.
  class counter_long_test extends counter_test;
    `uvm_component_utils(counter_long_test)
    function new(string name, uvm_component parent);
      super.new(name, parent);
    endfunction
    virtual function void build_phase(uvm_phase phase);
      counter_seq::type_id::set_type_override(counter_long_seq::get_type());
      super.build_phase(phase);
    endfunction
  endclass
endpackage
