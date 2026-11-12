//! Language semantics of the v2 front end, checked without outside tools:
//! each design is synthesized and its netlist is evaluated against the
//! function the SystemVerilog standard defines, over every input value (or
//! many random ones) and, for sequential designs, cycle by cycle.
//!
//! `tools/svcheck` does the same against a reference simulator for whole
//! designs; these tests pin the individual rules.

use helion_ir::{resolve, CellKind, Design, PortDir};
use std::collections::HashMap;

struct Sim {
    luts: Vec<(u64, [Option<String>; 6], String)>,
    ffs: Vec<(String, String)>,
    outs: Vec<(String, u32, String)>,
    val: HashMap<String, bool>,
}

impl Sim {
    fn new(d: &Design) -> Self {
        let pins = d.pin_index();
        let net = |cell: &str, pin: &str| pins.net_on(cell, pin).map(|n| resolve(n).to_string());
        let mut s = Sim { luts: vec![], ffs: vec![], outs: vec![], val: HashMap::new() };
        for c in &d.cells {
            let name = resolve(c.name);
            match c.kind {
                CellKind::Lut6 { init } => {
                    let ins: [Option<String>; 6] = std::array::from_fn(|i| net(name, &format!("I{i}")));
                    s.luts.push((init, ins, net(name, "O").expect("LUT output")));
                }
                CellKind::Hff => s.ffs.push((net(name, "D").expect("D"), net(name, "Q").expect("Q"))),
                CellKind::IobOut => {
                    let port = c.attrs.get("PORT").expect("PORT").to_string();
                    let bit: u32 = c.attrs.get("BIT").expect("BIT").parse().unwrap();
                    s.outs.push((port, bit, net(name, "I").expect("pad source")));
                }
                _ => panic!("unexpected cell kind in {name}"),
            }
        }
        s
    }
    fn set(&mut self, port: &str, w: u32, v: u64) {
        for i in 0..w {
            let name = if w == 1 { port.to_string() } else { format!("{port}_{i}") };
            self.val.insert(name, (v >> i) & 1 == 1);
        }
    }
    fn settle(&mut self) {
        for _ in 0..1000 {
            let mut changed = false;
            for (init, ins, o) in &self.luts {
                let mut a = 0u64;
                for (i, n) in ins.iter().enumerate() {
                    if n.as_ref().map(|n| self.val.get(n).copied().unwrap_or(false)).unwrap_or(false) {
                        a |= 1 << i;
                    }
                }
                let v = (init >> a) & 1 == 1;
                if self.val.insert(o.clone(), v) != Some(v) {
                    changed = true;
                }
            }
            if !changed {
                return;
            }
        }
        panic!("netlist does not settle");
    }
    fn clock(&mut self) {
        let next: Vec<(String, bool)> = self.ffs.iter().map(|(d, q)| (q.clone(), self.val.get(d).copied().unwrap_or(false))).collect();
        for (q, v) in next {
            self.val.insert(q, v);
        }
    }
    fn get(&self, port: &str) -> u64 {
        let mut v = 0;
        for (p, bit, net) in &self.outs {
            if p == port && self.val.get(net).copied().unwrap_or(false) {
                v |= 1 << bit;
            }
        }
        v
    }
}

fn synth(src: &str) -> (Design, Vec<String>) {
    let s = helion_sv::synth_sv_v2(src, "t.sv", None).unwrap_or_else(|e| panic!("synthesis failed: {e}"));
    (s.design, s.warnings)
}

fn widths(d: &Design, src: &str) -> Vec<(String, u32)> {
    // Input widths from the header text: `input [signed] [N:0] name`.
    let mut out = Vec::new();
    for p in d.ports.iter().filter(|p| p.dir == PortDir::In) {
        let name = resolve(p.name).to_string();
        let w = src
            .split(|c| c == ',' || c == '(' || c == ')')
            .find(|seg| seg.split_whitespace().last() == Some(name.as_str()) && seg.contains("input"))
            .and_then(|seg| seg.split('[').nth(1).and_then(|r| r.split(':').next()).and_then(|n| n.trim().parse::<u32>().ok()))
            .map(|hi| hi + 1)
            .unwrap_or(1);
        out.push((name, w));
    }
    out
}

/// Check a combinational design against `f(inputs) -> outputs` over all
/// input values (inputs total at most 16 bits).
fn comb(src: &str, f: impl Fn(&HashMap<String, u64>) -> Vec<(&'static str, u64)>) {
    let (d, _) = synth(src);
    let ins = widths(&d, src);
    let total: u32 = ins.iter().map(|i| i.1).sum();
    assert!(total <= 16, "too many input bits for exhaustive check");
    let mut sim = Sim::new(&d);
    for x in 0..(1u64 << total) {
        let mut vals = HashMap::new();
        let mut sh = 0;
        for (name, w) in &ins {
            let v = (x >> sh) & ((1 << w) - 1);
            sh += w;
            sim.set(name, *w, v);
            vals.insert(name.clone(), v);
        }
        sim.settle();
        for (port, want) in f(&vals) {
            assert_eq!(sim.get(port), want, "{port} for inputs {vals:?}");
        }
    }
}

fn sx(v: u64, w: u32) -> i64 {
    ((v << (64 - w)) as i64) >> (64 - w)
}

#[test]
fn addition_is_sized_by_its_context() {
    // The sum is 5 bits wide because the target is: the carry is kept.
    comb("module t(input [3:0] a, input [3:0] b, output [4:0] s, output [3:0] n); assign s = a + b; assign n = a + b; endmodule", |v| {
        vec![("s", v["a"] + v["b"]), ("n", (v["a"] + v["b"]) & 15)]
    });
}

#[test]
fn comparison_operands_are_self_determined() {
    // (a + b) is 4 bits inside the comparison: the carry is lost there.
    comb("module t(input [3:0] a, input [3:0] b, output y, output z); assign y = (a + b) == 4'd0; assign z = (a + b + 5'd0) == 5'd16; endmodule", |v| {
        vec![("y", (((v["a"] + v["b"]) & 15) == 0) as u64), ("z", ((v["a"] + v["b"]) == 16) as u64)]
    });
}

#[test]
fn signedness_needs_every_operand_signed() {
    comb(
        "module t(input signed [3:0] a, input signed [3:0] b, input [3:0] u, output lt, output mix, output [7:0] ext, output [7:0] uext, output [3:0] sra, output [3:0] srl);
           assign lt = a < b;
           assign mix = a < u;          // unsigned comparison: u is unsigned
           assign ext = a;              // sign-extended
           assign uext = a + u;         // zero-extended: one operand is unsigned
           assign sra = a >>> 1;
           assign srl = u >>> 1;
         endmodule",
        |v| {
            let (a, b, u) = (v["a"], v["b"], v["u"]);
            vec![
                ("lt", (sx(a, 4) < sx(b, 4)) as u64),
                ("mix", (a < u) as u64),
                ("ext", (sx(a, 4) as u64) & 0xff),
                ("uext", (a + u) & 0xff),
                ("sra", ((sx(a, 4) >> 1) as u64) & 15),
                ("srl", u >> 1),
            ]
        },
    );
}

#[test]
fn casts_and_system_functions() {
    comb(
        "module t(input [3:0] a, input [3:0] b, output [7:0] s, output [7:0] w, output lt, output [2:0] ones, output oh);
           assign s = $signed(a) * $signed(b);
           assign w = 8'(a) << 4;
           assign lt = signed'(a) < signed'(b);
           assign ones = $countones(a);
           assign oh = $onehot(b);
         endmodule",
        |v| {
            let (a, b) = (v["a"], v["b"]);
            vec![
                ("s", ((sx(a, 4) * sx(b, 4)) as u64) & 0xff),
                ("w", (a << 4) & 0xff),
                ("lt", (sx(a, 4) < sx(b, 4)) as u64),
                ("ones", a.count_ones() as u64),
                ("oh", (b.count_ones() == 1) as u64),
            ]
        },
    );
}

#[test]
fn division_and_modulo() {
    comb(
        "module t(input [4:0] a, input [2:0] b, input signed [3:0] p, input signed [2:0] q, output [4:0] d, output [4:0] m, output signed [3:0] sd, output signed [3:0] sm);
           assign d = a / b; assign m = a % b; assign sd = p / q; assign sm = p % q;
         endmodule",
        |v| {
            let (a, b, p, q) = (v["a"], v["b"], sx(v["p"], 4), sx(v["q"], 3));
            let mut out = vec![];
            if b != 0 {
                out.push(("d", a / b));
                out.push(("m", a % b));
            }
            if q != 0 {
                out.push(("sd", (p.wrapping_div(q) as u64) & 15));
                out.push(("sm", (p.wrapping_rem(q) as u64) & 15));
            }
            out
        },
    );
}

#[test]
fn shifts_selects_and_concatenation() {
    comb(
        "module t(input [7:0] a, input [2:0] n, output [7:0] l, output [7:0] r, output b, output [3:0] up, output [3:0] dn, output [15:0] rep);
           assign l = a << n; assign r = a >> n; assign b = a[n];
           assign up = a[n +: 4]; assign dn = a[n -: 4];
           assign rep = {2{a[3:0], ~a[7:4]}};
         endmodule",
        |v| {
            let (a, n) = (v["a"], v["n"]);
            let part = |lo: i64| -> u64 { (0..4).map(|i| if lo + i >= 0 && lo + i < 8 { ((a >> (lo + i)) & 1) << i } else { 0 }).sum() };
            let unit = ((a & 15) << 4) | (!(a >> 4) & 15);
            let mut out = vec![("l", (a << n) & 0xff), ("r", a >> n), ("b", (a >> n) & 1), ("rep", (unit << 8) | unit)];
            if n <= 4 {
                out.push(("up", part(n as i64)));
            }
            if n >= 3 {
                out.push(("dn", part(n as i64 - 3)));
            }
            out
        },
    );
}

#[test]
fn case_variants_and_priority() {
    comb(
        "module t(input [3:0] a, input [1:0] s, output reg [1:0] z, output reg [3:0] m, output reg p);
           always @* begin
             casez (a)
               4'b1???: z = 2'd3;
               4'b01??: z = 2'd2;
               4'b001?: z = 2'd1;
               default: z = 2'd0;
             endcase
           end
           always_comb begin
             m = 4'd0;
             unique case (s)
               2'd0: m = a;
               2'd1: m = ~a;
               2'd2: m = {a[0], a[3:1]};
               2'd3: m = a + 4'd1;
             endcase
           end
           always_comb begin
             p = 1'b0;
             case (1'b1)
               a[0]: p = s[0];
               a[1]: p = s[1];
               a[2]: p = 1'b1;
             endcase
           end
         endmodule",
        |v| {
            let (a, s) = (v["a"], v["s"]);
            let z = if a & 8 != 0 { 3 } else if a & 4 != 0 { 2 } else if a & 2 != 0 { 1 } else { 0 };
            let m = match s {
                0 => a,
                1 => !a & 15,
                2 => ((a & 1) << 3) | (a >> 1),
                _ => (a + 1) & 15,
            };
            let p = if a & 1 != 0 { s & 1 } else if a & 2 != 0 { (s >> 1) & 1 } else if a & 4 != 0 { 1 } else { 0 };
            vec![("z", z), ("m", m), ("p", p)]
        },
    );
}

#[test]
fn loops_functions_and_early_return() {
    comb(
        "module t(input [7:0] a, output [3:0] first, output [3:0] cnt, output [7:0] rev);
           function automatic logic [3:0] lowest(input logic [7:0] x);
             for (int i = 0; i < 8; i++) if (x[i]) return 4'(i);
             return 4'd15;
           endfunction
           function automatic logic [3:0] count(input logic [7:0] x);
             logic [3:0] n;
             n = 0;
             for (int i = 0; i < 8; i++) begin
               if (!x[i]) continue;
               n++;
             end
             return n;
           endfunction
           logic [7:0] r;
           always_comb for (int i = 0; i < 8; i++) r[i] = a[7 - i];
           assign first = lowest(a);
           assign cnt = count(a);
           assign rev = r;
         endmodule",
        |v| {
            let a = v["a"];
            vec![
                ("first", if a == 0 { 15 } else { a.trailing_zeros() as u64 }),
                ("cnt", a.count_ones() as u64),
                ("rev", (a as u8).reverse_bits() as u64),
            ]
        },
    );
}

#[test]
fn structs_enums_packages_and_parameters() {
    comb(
        "package p;
           typedef enum logic [1:0] {ADD, SUB, AND_, XOR_} op_t;
           typedef struct packed { op_t op; logic [3:0] x; logic [3:0] y; } req_t;
           localparam int W = 4;
           function automatic logic [W-1:0] run(req_t r);
             unique case (r.op)
               ADD: return r.x + r.y;
               SUB: return r.x - r.y;
               AND_: return r.x & r.y;
               default: return r.x ^ r.y;
             endcase
           endfunction
         endpackage
         module child #(parameter int N = 2, parameter logic [3:0] K = 4'h5) (input logic [N-1:0] d, output logic [N-1:0] q);
           assign q = d ^ N'(K);
         endmodule
         module t import p::*; (input [9:0] bits, output [3:0] y, output [3:0] f, output [2:0] g);
           req_t r;
           assign r = bits;
           assign y = run(r);
           child #(.N(4), .K(4'hA)) u4 (.d(r.x), .q(f));
           child #(.N(3)) u3 (.d(r.y[2:0]), .q(g));
         endmodule",
        |v| {
            let b = v["bits"];
            let (op, x, y) = (b >> 8, (b >> 4) & 15, b & 15);
            let r = match op {
                0 => (x + y) & 15,
                1 => x.wrapping_sub(y) & 15,
                2 => x & y,
                _ => x ^ y,
            };
            vec![("y", r), ("f", x ^ 0xA), ("g", (y & 7) ^ 5)]
        },
    );
}

#[test]
fn arrays_generate_and_hierarchical_names() {
    comb(
        "module leaf(input [1:0] a, output [1:0] y); logic [1:0] t; assign t = a + 2'd1; assign y = t; endmodule
         module t(input [7:0] a, input [1:0] s, output [1:0] pick, output [1:0] deep, output [7:0] all);
           logic [3:0][1:0] words;
           logic [1:0] outs [4];
           assign words = a;
           for (genvar i = 0; i < 4; i++) begin : g
             leaf u (.a(words[i]), .y(outs[i]));
           end
           assign pick = outs[s];
           assign deep = g[2].u.t;
           assign all = {outs[3], outs[2], outs[1], outs[0]};
         endmodule",
        |v| {
            let (a, s) = (v["a"], v["s"]);
            let w = |i: u64| ((a >> (2 * i)) & 3) + 1 & 3;
            vec![("pick", w(s)), ("deep", w(2)), ("all", w(0) | (w(1) << 2) | (w(2) << 4) | (w(3) << 6))]
        },
    );
}

#[test]
fn registers_hold_enable_reset_and_initial_values() {
    let src = "module t(input clk, input rst, input en, input [3:0] d, output [3:0] q, output [3:0] c, output one);
                 logic [3:0] r, cnt;
                 logic init1 = 1'b1;
                 always_ff @(posedge clk) begin
                   if (rst) r <= 4'h9;
                   else if (en) r <= d;
                 end
                 always_ff @(posedge clk) cnt <= cnt + 4'd3;
                 always_ff @(posedge clk) if (en) init1 <= d[0];
                 assign q = r; assign c = cnt; assign one = init1;
               endmodule";
    let (d, _) = synth(src);
    let mut sim = Sim::new(&d);
    let (mut r, mut cnt, mut one) = (0u64, 0u64, 1u64);
    let mut x = 0x1234_5678_9abc_def1u64;
    for cycle in 0..400 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let (rst, en, dv) = (((x >> 5) & 15 == 0) as u64, (x >> 9) & 1, (x >> 12) & 15);
        sim.set("rst", 1, rst);
        sim.set("en", 1, en);
        sim.set("d", 4, dv);
        sim.settle();
        assert_eq!((sim.get("q"), sim.get("c"), sim.get("one")), (r, cnt, one), "cycle {cycle}");
        sim.clock();
        if rst == 1 {
            r = 9;
        } else if en == 1 {
            r = dv;
        }
        cnt = (cnt + 3) & 15;
        if en == 1 {
            one = dv & 1;
        }
    }
}

#[test]
fn memory_with_variable_index_and_blocking_temporaries() {
    let src = "module t(input clk, input we, input [1:0] wa, input [1:0] ra, input [3:0] wd, output [3:0] rd, output [4:0] acc);
                 logic [3:0] mem [0:3];
                 logic [4:0] sum;
                 always_ff @(posedge clk) begin
                   logic [4:0] tmp;
                   tmp = {1'b0, wd} + 5'd1;
                   if (we) mem[wa] <= wd;
                   sum <= sum + tmp;
                 end
                 assign rd = mem[ra]; assign acc = sum;
               endmodule";
    let (d, _) = synth(src);
    let mut sim = Sim::new(&d);
    let mut mem = [0u64; 4];
    let mut sum = 0u64;
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    for cycle in 0..400 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let (we, wa, ra, wd) = (x & 1, (x >> 1) & 3, (x >> 3) & 3, (x >> 5) & 15);
        sim.set("we", 1, we);
        sim.set("wa", 2, wa);
        sim.set("ra", 2, ra);
        sim.set("wd", 4, wd);
        sim.settle();
        assert_eq!((sim.get("rd"), sim.get("acc")), (mem[ra as usize], sum), "cycle {cycle}");
        sim.clock();
        if we == 1 {
            mem[wa as usize] = wd;
        }
        sum = (sum + wd + 1) & 31;
    }
}

#[test]
fn what_cannot_be_built_is_an_error_with_a_line() {
    let cases = [
        ("module t(input a, input en, output reg q);\n always_comb begin\n  if (en) q = a;\n end\nendmodule", "latch"),
        ("module t(input a, output y);\n assign y = a;\n assign y = ~a;\nendmodule", "more than one driver"),
        ("module t(input a, output y);\n wire w;\n assign w = ~w | a;\n assign y = w;\nendmodule", "combinational loop"),
        ("module t(input [3:0] a, output y);\n assign y = a[undefined_name];\nendmodule", "not declared"),
        ("module t(input a, output y);\n missing u (.x(a), .y(y));\nendmodule", "not defined"),
        ("module t(input [3:0] n, output reg [3:0] y);\n always_comb begin y = 0; for (int i = 0; i < n; i++) y = y + 1; end\nendmodule", "not constant"),
    ];
    for (src, want) in cases {
        let e = match helion_sv::synth_sv_v2(src, "t.sv", None) {
            Ok(_) => panic!("accepted: {src}"),
            Err(e) => e,
        };
        assert!(e.contains(want), "expected '{want}' in: {e}");
    }
    // Line numbers survive the preprocessor.
    let e = helion_sv::synth_sv_v2("`define W 4\nmodule t(input [`W-1:0] a, output y);\n\n assign y = nope;\nendmodule", "t.sv", None).err().expect("an undeclared name is an error");
    assert!(e.contains("line 4"), "{e}");
}

#[test]
fn macros_conditionals_and_predefined_synthesis() {
    comb(
        "`define OP(x, y) ((x) ^ (y))
         `ifdef SYNTHESIS
           `define K 4'h6
         `else
           `define K 4'h0
         `endif
         module t(input [3:0] a, input [3:0] b, output [3:0] y);
           assign y = `OP(a,
                          b) + `K;
         endmodule",
        |v| vec![("y", ((v["a"] ^ v["b"]) + 6) & 15)],
    );
}
