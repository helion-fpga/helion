//! Debug: print the synthesized IR of a .vhd/.sv file.
fn main() {
    let p = std::env::args().nth(1).expect("file");
    let d = if p.ends_with(".vhd") || p.ends_with(".vhdl") {
        helion_vhdl::synth_vhdl_path(std::path::Path::new(&p)).unwrap()
    } else {
        let src = std::fs::read_to_string(&p).unwrap();
        helion_sv::synth_sv(&src, &p).unwrap()
    };
    println!("design {} ports={} cells={} nets={}", d.name, d.ports.len(), d.cells.len(), d.nets.len());
    for pt in &d.ports { println!("port {:?} {} attrs={:?}", pt.dir, pt.name, pt.attrs); }
    for c in d.cells.iter().take(40) { println!("cell {} {:?} attrs={:?}", c.name, c.kind, c.attrs); }
    for n in d.nets.iter().take(60) { println!("net {} {:?}", n.name, n.endpoints.iter().map(|e| format!("{}.{}", e.cell, e.pin)).collect::<Vec<_>>()); }
}
