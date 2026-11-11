//! Stage d (constrained randomization) and stage e (macros / include).

mod common;
use common::*;

#[test]
fn constraints_ranges_inside_relations_implication_dist_and_with() {
    let o = run(
        "rand",
        r#"
class Txn;
  rand bit [7:0] addr;
  rand bit [7:0] len;
  rand bit [1:0] kind;
  randc bit [2:0] tag;
  bit [7:0] limit = 100;
  int pre, post;
  constraint c_addr { addr inside {[10:20], 50}; }
  constraint c_len  { len < limit; len > addr; }
  constraint c_kind { kind dist { 0 := 1, 1 := 0, [2:3] :/ 2 }; }
  constraint c_imp  { (kind == 3) -> len == 99; }
  function void pre_randomize(); pre++; endfunction
  function void post_randomize(); post++; endfunction
endclass

module tb;
  initial begin
    Txn t = new;
    int bad = 0, k1 = 0, k3 = 0;
    bit [7:0] tags[$];
    for (int i = 0; i < 200; i++) begin
      if (!t.randomize()) bad++;
      if (!((t.addr >= 10 && t.addr <= 20) || t.addr == 50)) bad++;
      if (!(t.len < 100 && t.len > t.addr)) bad++;
      if (t.kind == 1) k1++;
      if (t.kind == 3) begin k3++; if (t.len != 99) bad++; end
      if (i < 8) tags.push_back(t.tag);
    end
    tags.sort();
    $display("bad=%0d k1=%0d k3_seen=%0d pre=%0d post=%0d", bad, k1, k3 > 0, t.pre, t.post);
    $display("randc=%p", tags);
    if (!t.randomize() with { addr == 50; len == 77; }) $display("with failed");
    $display("with addr=%0d len=%0d", t.addr, t.len);
    t.c_addr.constraint_mode(0);
    t.addr.rand_mode(0);
    t.addr = 200;
    t.limit = 255;
    if (t.randomize()) $display("mode addr=%0d len_ok=%0d", t.addr, t.len > 200);
    if (!t.randomize() with { len == 5; }) $display("unsat reported");
  end
endmodule
"#,
    );
    let t = o.text();
    assert!(o.has("bad=0 k1=0 k3_seen=1 pre=200 post=200"), "{t}");
    assert!(o.has("randc='{0, 1, 2, 3, 4, 5, 6, 7}"), "randc covers the cycle: {t}");
    assert!(o.has("with addr=50 len=77"), "{t}");
    assert!(o.has("mode addr=200 len_ok=1"), "{t}");
    assert!(o.has("unsat reported"), "{t}");
    assert!(o.res.log.iter().any(|l| l.text.contains("randomize() failed")), "{t}");
}

#[test]
fn std_randomize_solve_before_and_seed_reproducibility() {
    let src = r#"
module tb;
  initial begin
    int unsigned a, b;
    repeat (3) begin
      void'(std::randomize(a, b) with { a < 10; b == a + 5; solve a before b; });
      $display("a=%0d b=%0d", a, b);
    end
    $display("u=%0d r=%0d", $urandom_range(5, 1) <= 5, $urandom % 4 < 4);
  end
endmodule
"#;
    let a = run("stdrand1", src);
    let b = run("stdrand2", src);
    assert_eq!(a.lines, b.lines, "same seed, same stream");
    for l in a.lines.iter().filter(|l| l.starts_with("a=")) {
        let v: Vec<i64> = l.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| s.parse().unwrap()).collect();
        assert!(v[0] < 10 && v[1] == v[0] + 5, "{l}");
    }
}

#[test]
fn macros_with_args_include_and_ifdef() {
    let d = tmp("macros");
    std::fs::write(
        d.join("defs.svh"),
        r#"
`ifndef DEFS_SVH
`define DEFS_SVH
`define MAX(a, b) ((a) > (b) ? (a) : (b))
`define SHOW(x) $display(`"x = %0d`", x)
`define GREETING "hi"
`endif
"#,
    )
    .unwrap();
    let o = run_in(
        &d,
        "tb.sv",
        r#"
`include "defs.svh"
`include "defs.svh"
`define MODE_FAST
module tb;
  initial begin
    int v = `MAX(3, 9);
    `SHOW(v);
`ifdef MODE_FAST
    $display("fast %s", `GREETING);
`elsif MODE_SLOW
    $display("slow");
`else
    $display("default");
`endif
`ifndef NOT_DEFINED
    $display("line %0d in %s", `__LINE__, `__FILE__);
`endif
  end
endmodule
"#,
        &[],
    )
    .unwrap();
    let t = o.text();
    assert!(o.has("v = 9"), "{t}");
    assert!(o.has("fast hi") && !o.has("slow") && !o.has("default"), "{t}");
    assert!(o.lines.iter().any(|l| l.starts_with("line 17 in ") && l.ends_with("tb.sv")), "{t}");
}
