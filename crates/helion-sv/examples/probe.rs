use helion_ir::CellKind;
fn main(){
    let d = helion_sv::synth_sv("module t(input logic [7:0] a, input logic [2:0] b, output logic [7:0] y);\n assign y = a << b;\nendmodule","t.sv").unwrap();
    for c in &d.cells { if let CellKind::Lut6{init}=c.kind { println!("cell {} init={init:#x}", c.name); } }
    for n in &d.nets { println!("net {} -> {:?}", n.name, n.endpoints.iter().map(|e|format!("{}.{}",e.cell,e.pin)).collect::<Vec<_>>()); }
}
