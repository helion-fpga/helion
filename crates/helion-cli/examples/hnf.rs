//! Synthesize SystemVerilog with Helion's front end and print the netlist
//! as HNF text. Used by `tools/svcheck` to compare the netlist with the RTL.
//!
//! ```text
//! cargo run --release -p helion-cli --example hnf -- [--legacy] design.sv [more.sv ...] [--top name] > design.hnf
//! ```

fn main() {
    let all: Vec<String> = std::env::args().collect();
    let legacy = all.iter().any(|a| a == "--legacy");
    let a: Vec<&String> = all.iter().skip(1).filter(|a| !a.starts_with("--")).collect();
    if a.is_empty() {
        eprintln!("usage: hnf [--legacy] <design.sv> [more.sv ...] [--top name]");
        std::process::exit(2);
    }
    let path = std::path::Path::new(a[0].as_str());
    let r = if legacy {
        helion_sv::synth_sv_path(path)
    } else {
        let top = all.iter().position(|x| x == "--top").and_then(|i| all.get(i + 1)).map(|s| s.as_str());
        let regmap_path = all.iter().position(|x| x == "--regmap").and_then(|i| all.get(i + 1)).map(|s| s.as_str());
        let files: Vec<&std::path::Path> = a.iter().filter(|x| Some(x.as_str()) != top && Some(x.as_str()) != regmap_path).map(|x| std::path::Path::new(x.as_str())).collect();
        helion_sv::synth_sv_v2_files(&files, top).map(|s| {
            for w in &s.warnings {
                eprintln!("warning: {w}");
            }
            eprintln!("synth: {} LUTs, {} flops, {} LUT levels", s.luts, s.ffs, s.depth);
            if let Some(path) = all.iter().position(|x| x == "--regmap").and_then(|i| all.get(i + 1)) {
                // net, hierarchical name, index (or -), inverted
                let text: String = s
                    .regmap
                    .iter()
                    .map(|r| format!("{} {} {} {}\n", r.net, r.hier, if r.scalar { "s".to_string() } else { r.index.map(|i| i.to_string()).unwrap_or("-".into()) }, r.inverted as u8))
                    .collect();
                let _ = std::fs::write(path, text);
            }
            s.design
        })
    };
    match r {
        Ok(d) => print!("{}", d.to_hnf()),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
