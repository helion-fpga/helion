//! Memory probe: drive a real session headlessly (open counter.prj, Ibex,
//! simulation, testbenches, ILA, editor diagnostics, debugger) with the
//! per-frame poll running, printing resident memory after each step.
//! `cargo run -p helion-rune --example memprobe` — every step should stay flat.
use helion_rune::state::CenterPane;
use rune::{Env, Runtime};

fn rss_mb() -> f64 {
    let pid = std::process::id().to_string();
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap_or(0.0) / 1024.0
}

fn pump(rt: &mut Runtime<helion_rune::AppState>, t: &mut f64, frames: usize) {
    for _ in 0..frames {
        *t += 1.0 / 60.0;
        let now = *t;
        rt.update(move |s, tx| {
            helion_rune::poll_all(s, now, tx);
        });
        rt.frame(*t);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn main() {
    let spec = helion_rune::app_spec();
    let env = Env { system_dark: true, scale: 2.0 };
    let mut rt = Runtime::new(helion_rune::init(), helion_rune::view, spec.size, env);
    let mut t = 0.0;
    let prj = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.prj");
    helion_rune::pickers::open_path(&mut rt.state, &prj);
    rt.invalidate();
    pump(&mut rt, &mut t, 30);
    println!("open                {:8.1} MB", rss_mb());
    let step = |name: &str, rt: &mut Runtime<helion_rune::AppState>, t: &mut f64, f: &dyn Fn(&mut helion_rune::AppState)| {
        rt.update(|s, _| f(s));
        for k in 0..6 {
            pump(rt, t, 300);
            println!("{name:<14} +{:>2}s    {:8.1} MB", (k + 1) * 5, rss_mb());
        }
    };
    step("open ibex", &mut rt, &mut t, &|s| {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/ysyx_ibex.sv");
        helion_rune::pickers::open_path(s, &p);
        s.navigate(CenterPane::Editor);
    });
    step("type in ibex", &mut rt, &mut t, &|s| {
        s.editor.edit.insert("// x\n");
        s.editor.dirty = true;
    });
}
