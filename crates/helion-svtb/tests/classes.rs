//! Stage a: classes, objects, handles, inheritance, virtual methods,
//! static members, parameterized classes.

mod common;
use common::*;

#[test]
fn classes_inheritance_virtual_static_and_super() {
    let o = run(
        "classes",
        r#"
class Animal;
  string name;
  static int count = 0;
  int legs;
  function new(string n, int l = 4);
    name = n;
    legs = l;
    count++;
  endfunction
  virtual function string speak();
    return "...";
  endfunction
  function string kind();
    return "animal";
  endfunction
  function string describe();
    return $sformatf("%s says %s (%0d legs)", name, speak(), legs);
  endfunction
endclass

class Dog extends Animal;
  function new(string n);
    super.new(n);
  endfunction
  virtual function string speak();
    return "woof";
  endfunction
  function string kind();
    return "dog";
  endfunction
endclass

class Puppy extends Dog;
  function new(string n);
    super.new(n);
    this.legs = 4;
  endfunction
  function string speak();
    return {super.speak(), "!"};
  endfunction
endclass

class Bird extends Animal;
  function new(string n);
    super.new(n, 2);
  endfunction
  virtual function string speak();
    return "tweet";
  endfunction
endclass

module tb;
  initial begin
    Animal zoo[$];
    Animal a;
    Dog d;
    Puppy p;
    Bird b;
    d = new("rex");
    p = new("bit");
    b = new("tweety");
    zoo.push_back(d);
    zoo.push_back(p);
    zoo.push_back(b);
    foreach (zoo[i]) $display("%s", zoo[i].describe());
    a = d;
    $display("kind static=%s", a.kind());
    $display("count=%0d", Animal::count);
    if (a == d) $display("same handle");
    a = null;
    if (a == null) $display("null ok");
    if ($cast(d, zoo[1])) $display("cast ok %s", d.name);
    if (!$cast(d, zoo[2])) $display("cast rejected");
  end
endmodule
"#,
    );
    let t = o.text();
    assert!(o.has("rex says woof (4 legs)"), "{t}");
    assert!(o.has("bit says woof! (4 legs)"), "{t}");
    assert!(o.has("tweety says tweet (2 legs)"), "{t}");
    assert!(o.has("kind static=animal"), "non-virtual dispatch uses the handle type: {t}");
    assert!(o.has("count=3"), "{t}");
    assert!(o.has("same handle") && o.has("null ok") && o.has("cast ok bit") && o.has("cast rejected"), "{t}");
    assert!(o.res.failures.is_empty(), "{:?}", o.res.failures);
}

#[test]
fn parameterized_classes_have_per_specialization_statics() {
    let o = run(
        "param",
        r#"
class Stack #(type T = int, int DEPTH = 4);
  static int made;
  T items[$];
  function new();
    made++;
  endfunction
  function bit push(T x);
    if (items.size() >= DEPTH) return 0;
    items.push_back(x);
    return 1;
  endfunction
  function T pop();
    return items.pop_back();
  endfunction
endclass

class Pair #(type A = int, type B = string);
  A a;
  B b;
endclass

module tb;
  Stack #(int, 2) si;
  Stack #(string) ss;
  Stack #(int, 2) si2;
  Pair #(int) p;
  initial begin
    si = new;
    si2 = new;
    ss = new;
    p = new;
    void'(si.push(1));
    void'(si.push(2));
    if (!si.push(3)) $display("full at %0d", 2);
    $display("pop=%0d", si.pop());
    void'(ss.push("hello"));
    $display("str=%s", ss.pop());
    $display("made int2=%0d str=%0d", Stack#(int,2)::made, Stack#(string)::made);
    p.b = "bee";
    $display("pair=%0d,%s", p.a, p.b);
  end
endmodule
"#,
    );
    let t = o.text();
    assert!(o.has("full at 2") && o.has("pop=2") && o.has("str=hello"), "{t}");
    assert!(o.has("made int2=2 str=1"), "{t}");
    assert!(o.has("pair=0,bee"), "{t}");
}

#[test]
fn abstract_class_and_pure_virtual_are_enforced() {
    let d = tmp("abstract");
    let e = run_in(
        &d,
        "tb.sv",
        r#"
virtual class Shape;
  pure virtual function int area();
endclass
module tb;
  Shape s;
  initial s = new;
endmodule
"#,
        &[],
    );
    let out = e.unwrap();
    assert_eq!(out.res.failures.len(), 1, "{}", out.text());
    assert!(out.res.failures[0].msg.contains("abstract"), "{:?}", out.res.failures);
    assert_eq!(out.res.failures[0].line, 7);
}

#[test]
fn null_handle_access_is_a_located_failure() {
    let o = run(
        "nullh",
        r#"
class C; int x; endclass
module tb;
  C c;
  initial begin
    $display("before");
    c.x = 1;
    $display("after");
  end
endmodule
"#,
    );
    assert!(o.has("before") && !o.has("after"));
    assert_eq!(o.res.failures.len(), 1);
    assert_eq!(o.res.failures[0].line, 7);
    assert!(o.res.failures[0].msg.contains("null"), "{:?}", o.res.failures);
}

#[test]
fn shallow_copy_static_methods_and_class_scope_resolution() {
    let o = run(
        "copy",
        r#"
class Cfg;
  int n = 3;
  int q[$];
  static function Cfg make(int n);
    Cfg c = new;
    c.n = n;
    return c;
  endfunction
endclass
module tb;
  initial begin
    Cfg a, b;
    a = Cfg::make(7);
    a.q.push_back(1);
    b = new a;
    b.n = 9;
    b.q.push_back(2);
    $display("a=%0d/%0d b=%0d/%0d", a.n, a.q.size(), b.n, b.q.size());
  end
endmodule
"#,
    );
    assert!(o.has("a=7/1 b=9/2"), "{}", o.text());
}
