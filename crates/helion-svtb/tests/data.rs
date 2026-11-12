//! Stage b: dynamic arrays, queues, associative arrays, foreach, strings.

mod common;
use common::*;

#[test]
fn queues_dynamic_and_fixed_arrays() {
    let o = run(
        "arrays",
        r#"
module tb;
  initial begin
    int q[$];
    int d[];
    int f[4];
    int sorted[$];
    int found[$];
    string names[$] = '{"b", "a", "c"};
    q.push_back(3);
    q.push_back(1);
    q.push_front(7);
    q.insert(1, 9);
    $display("q=%p size=%0d first=%0d last=%0d", q, q.size(), q[0], q[$]);
    $display("pop_front=%0d pop_back=%0d size=%0d", q.pop_front(), q.pop_back(), q.size());
    q = {q, 5, 6};
    $display("concat=%p", q);
    q.delete(0);
    $display("after delete(0)=%p", q);
    d = new[3];
    foreach (d[i]) d[i] = i * 10;
    d = new[5](d);
    $display("d=%p size=%0d", d, d.size());
    f = '{4, 2, 8, 6};
    sorted = f;
    sorted.sort();
    $display("sorted=%p sum=%0d", sorted, f.sum());
    found = f.find with (item > 4);
    $display("found=%p idx=%p", found, f.find_index with (item == 8));
    names.sort();
    $display("names=%p", names);
    q.delete();
    $display("empty=%0d", q.size());
    $display("slice=%p", sorted[1:2]);
  end
endmodule
"#,
    );
    let t = o.text();
    assert!(o.has("q='{7, 9, 3, 1} size=4 first=7 last=1"), "{t}");
    assert!(o.has("pop_front=7 pop_back=1 size=2"), "{t}");
    assert!(o.has("concat='{9, 3, 5, 6}"), "{t}");
    assert!(o.has("after delete(0)='{3, 5, 6}"), "{t}");
    assert!(o.has("d='{0, 10, 20, 0, 0} size=5"), "{t}");
    assert!(o.has("sorted='{2, 4, 6, 8} sum=20"), "{t}");
    assert!(o.has("found='{8, 6} idx='{2}"), "{t}");
    assert!(o.has("names='{\"a\", \"b\", \"c\"}"), "{t}");
    assert!(o.has("empty=0") && o.has("slice='{4, 6}"), "{t}");
}

#[test]
fn associative_arrays_and_iteration() {
    let o = run(
        "assoc",
        r#"
module tb;
  initial begin
    int score[string];
    int sparse[int];
    string k;
    int ik;
    score["bob"] = 3;
    score["amy"] = 5;
    score["cat"] += 2;
    $display("num=%0d exists=%0d/%0d", score.num(), score.exists("amy"), score.exists("zed"));
    foreach (score[name]) $display("%s=%0d", name, score[name]);
    if (score.first(k)) do $display("iter %s", k); while (score.next(k));
    score.delete("bob");
    $display("after delete num=%0d", score.num());
    sparse[1000] = 1;
    sparse[-5] = 2;
    if (sparse.first(ik)) $display("first key %0d", ik);
    if (sparse.last(ik)) $display("last key %0d", ik);
  end
endmodule
"#,
    );
    let t = o.text();
    assert!(o.has("num=3 exists=1/0"), "{t}");
    assert!(o.has("amy=5") && o.has("bob=3") && o.has("cat=2"), "{t}");
    let iters: Vec<&String> = o.lines.iter().filter(|l| l.starts_with("iter")).collect();
    assert_eq!(iters, ["iter amy", "iter bob", "iter cat"], "{t}");
    assert!(o.has("after delete num=2") && o.has("first key -5") && o.has("last key 1000"), "{t}");
}

#[test]
fn strings_and_their_methods() {
    let o = run(
        "strings",
        r#"
module tb;
  initial begin
    string s = "Hello";
    string t;
    int n;
    t = {s, ", World"};
    $display("len=%0d upper=%s lower=%s", t.len(), t.toupper(), t.tolower());
    $display("sub=%s getc=%c cmp=%0d", t.substr(7, 11), t.getc(1), s.compare("Hello"));
    n = "42".atoi();
    t.itoa(n + 1);
    $display("atoi=%0d itoa=%s hex=%0d", n, t, "ff".atohex());
    if (s == "Hello" && s != "hello") $display("eq ok");
    $display("fmt=[%5d] [%-4s] [%08b] [%h] [%0t]", 42, "ab", 5, 8'hA5, $time);
  end
endmodule
"#,
    );
    let t = o.text();
    assert!(o.has("len=12 upper=HELLO, WORLD lower=hello, world"), "{t}");
    assert!(o.has("sub=World getc=e cmp=0"), "{t}");
    assert!(o.has("atoi=42 itoa=43 hex=255"), "{t}");
    assert!(o.has("eq ok"), "{t}");
    assert!(o.has("fmt=[   42] [ab  ] [00000101] [a5] [0]"), "{t}");
}

#[test]
fn enums_and_bit_operations() {
    let o = run(
        "enums",
        r#"
typedef enum logic [1:0] {IDLE, RUN = 2, DONE} state_t;
module tb;
  initial begin
    state_t s = IDLE;
    bit [7:0] b = 8'b1010_0101;
    s = s.next();
    $display("s=%s num=%0d", s.name(), s.num());
    s = DONE;
    $display("done=%0d name=%s", s, s.name());
    b[0] = 0;
    b[7:4] = 4'h3;
    $display("b=%b xor=%0d slice=%h", b, ^b, b[3:0]);
    $display("concat=%h repl=%b", {4'hA, 4'h5}, {3{2'b10}});
  end
endmodule
"#,
    );
    let t = o.text();
    assert!(o.has("s=RUN num=3"), "{t}");
    assert!(o.has("done=3 name=DONE"), "{t}");
    assert!(o.has("b=00110100 xor=1 slice=4"), "{t}");
    assert!(o.has("concat=a5 repl=101010"), "{t}");
}
