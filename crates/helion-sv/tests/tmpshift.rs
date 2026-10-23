mod common;
use helion_sv::synth_sv;
#[test]
fn pins(){
    let terms: Vec<String> = (0..4).map(|k| format!("(a[{k}]&(b==3'd{}))", 4-k)).collect();
    let src = format!("module t(input logic [7:0] a, input logic [2:0] b, output logic y); assign y = {}; endmodule", terms.join("|"));
    let d = synth_sv(&src,"t.sv").unwrap();
    for c in &d.cells {
        let pins: Vec<String> = (0..6).map(|i| {
            let n = d.net_on(&c.name, &format!("I{i}"));
            format!("I{i}={:?}", n)
        }).collect();
        println!("cell {} {:?} pins {:?}", c.name, c.kind, pins);
    }
}
