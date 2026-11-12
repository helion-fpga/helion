//! Stage c: fork/join variants, events, mailboxes, semaphores, delays in
//! class tasks, virtual interfaces + clocking blocks bound to a DUT.

mod common;
use common::*;

#[test]
fn fork_join_any_none_and_disable() {
    let o = run(
        "fork",
        r#"
class Worker;
  string name;
  function new(string n); name = n; endfunction
  task work(int d);
    #(d);
    $display("%s done at %0t", name, $time);
  endtask
endclass
module tb;
  initial begin
    Worker a = new("a"), b = new("b"), c = new("c");
    fork
      a.work(10);
      b.work(5);
    join
    $display("join at %0t", $time);
    fork
      a.work(30);
      b.work(7);
    join_any
    $display("join_any at %0t", $time);
    disable fork;
    fork
      c.work(1);
    join_none
    $display("join_none at %0t", $time);
    wait fork;
    $display("wait fork at %0t", $time);
    for (int i = 0; i < 3; i++) begin
      automatic int k = i;
      fork
        #(k) $display("loop fork k=%0d at %0t", k, $time);
      join_none
    end
    #10 $display("end at %0t", $time);
  end
endmodule
"#,
    );
    let t = o.text();
    let expect = [
        "b done at 5",
        "a done at 10",
        "join at 10",
        "b done at 17",
        "join_any at 17",
        "join_none at 17",
        "c done at 18",
        "wait fork at 18",
        "loop fork k=0 at 18",
        "loop fork k=1 at 19",
        "loop fork k=2 at 20",
        "end at 28",
    ];
    let got: Vec<&str> = o.lines.iter().map(String::as_str).filter(|l| !l.starts_with("$finish")).collect();
    assert_eq!(got, expect, "{t}");
    assert!(!o.has("a done at 40"), "disable fork must kill a.work(30): {t}");
}

#[test]
fn events_mailboxes_and_semaphores() {
    let o = run(
        "mbx",
        r#"
class Packet;
  int id;
  function new(int i); id = i; endfunction
endclass
module tb;
  mailbox #(Packet) mb = new(2);
  semaphore sem = new(1);
  event go;
  initial begin
    Packet p;
    @(go);
    $display("got go at %0t", $time);
    repeat (4) begin
      mb.get(p);
      $display("rx %0d at %0t", p.id, $time);
      #10;
    end
  end
  initial begin
    Packet p;
    #3 -> go;
    for (int i = 0; i < 4; i++) begin
      p = new(i);
      mb.put(p);
      $display("tx %0d at %0t (num=%0d)", i, $time, mb.num());
    end
  end
  initial begin
    #1;
    fork
      begin sem.get(); $display("A has key at %0t", $time); #20; sem.put(); end
      begin #1 sem.get(); $display("B has key at %0t", $time); sem.put(); end
    join
    if (sem.try_get()) $display("try_get ok");
  end
endmodule
"#,
    );
    let t = o.text();
    for s in [
        "got go at 3",
        "tx 0 at 3",
        "tx 1 at 3",
        "rx 0 at 3",
        "tx 2 at 3",
        "rx 1 at 13",
        "tx 3 at 13",
        "rx 2 at 23",
        "rx 3 at 33",
        "A has key at 1",
        "B has key at 21",
        "try_get ok",
    ] {
        assert!(o.has(s), "missing `{s}`:\n{t}");
    }
}

const COUNTER: &str = r#"
module counter (
    input logic clk,
    output logic led
);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assign led = cnt[3];
endmodule
"#;

#[test]
fn virtual_interface_clocking_block_drives_and_samples_the_dut() {
    let d = tmp("vif");
    std::fs::write(d.join("counter.sv"), COUNTER).unwrap();
    let o = run_in(
        &d,
        "tb.sv",
        r#"
interface cnt_if(input logic clk);
  logic led;
  clocking cb @(posedge clk);
    input led;
  endclocking
endinterface

class Monitor;
  virtual cnt_if vif;
  int rises;
  function new(virtual cnt_if v); vif = v; endfunction
  task run(int cycles);
    bit last;
    repeat (cycles) begin
      @(vif.cb);
      if (vif.cb.led && !last) rises++;
      last = vif.cb.led;
    end
  endtask
endclass

module tb;
  logic clk = 0;
  always #5 clk = ~clk;
  cnt_if vif(clk);
  counter dut(.clk(clk), .led(vif.led));
  initial begin
    Monitor m = new(vif);
    @(posedge vif.led);
    $display("led rose at %0t", $time);
    m.run(40);
    $display("rises=%0d", m.rises);
    $finish;
  end
endmodule
"#,
        &[],
    )
    .unwrap();
    let t = o.text();
    // 8 rising clock edges (5, 15, …, 75 ns) make cnt[3] = 1.
    assert!(o.has("led rose at 75"), "{t}");
    assert!(o.has("rises=3"), "{t}");
    assert!(o.res.finished && o.res.failures.is_empty(), "{t}");
    assert_eq!(o.res.duts.len(), 1);
    assert!(o.res.duts[0].cov.toggle_bin_coverage_pct() > 0.0);
}

#[test]
fn zero_delay_loop_and_timeout_are_reported() {
    let d = tmp("loop");
    let o = run_in(&d, "tb.sv", "module tb; initial forever begin end endmodule", &[]).unwrap();
    assert_eq!(o.res.failures.len(), 1, "{}", o.text());
    assert!(o.res.failures[0].msg.contains("without suspending"));
}
