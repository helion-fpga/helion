#![allow(dead_code)]

use std::path::{Path, PathBuf};

use helion_svtb::{CompileOptions, DutProvider, Event, RunConfig, RunResult, Sev};

/// Synthesizes DUT modules from `.sv` files in a directory.
pub struct DirDut(pub PathBuf);

impl DutProvider for DirDut {
    fn synth(&mut self, module: &str) -> Result<(helion_ir::Design, PathBuf), String> {
        let p = self.0.join(format!("{module}.sv"));
        if !p.is_file() {
            return Err(format!("no {}", p.display()));
        }
        let d = helion_sv::synth_sv_path(&p)?;
        Ok((d, p))
    }
}

pub fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("helion_svtb_{}_{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub struct Out {
    pub res: RunResult,
    pub lines: Vec<String>,
}

impl Out {
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
    pub fn has(&self, s: &str) -> bool {
        self.lines.iter().any(|l| l.contains(s))
    }
    pub fn infos(&self) -> Vec<String> {
        self.res.log.iter().filter(|l| l.sev == Sev::Info).map(|l| l.text.clone()).collect()
    }
}

pub fn run_in(dir: &Path, file: &str, src: &str, plusargs: &[&str]) -> Result<Out, String> {
    let p = dir.join(file);
    std::fs::write(&p, src).unwrap();
    let prog = helion_svtb::compile(&[p], &CompileOptions::default()).map_err(|e| e.to_string())?;
    let mut lines = Vec::new();
    let mut sink = |e: Event| {
        if let Event::Log { text, .. } = e {
            lines.push(text);
        }
    };
    let cfg = RunConfig { plusargs: plusargs.iter().map(|s| s.to_string()).collect(), max_ps: 50_000_000_000, ..Default::default() };
    let mut dut = DirDut(dir.to_path_buf());
    let res = helion_svtb::run(&prog, &mut dut, cfg, &mut sink).map_err(|e| e.to_string())?;
    Ok(Out { res, lines })
}

pub fn run(name: &str, src: &str) -> Out {
    let d = tmp(name);
    match run_in(&d, "tb.sv", src, &[]) {
        Ok(o) => o,
        Err(e) => panic!("{name}: {e}"),
    }
}
