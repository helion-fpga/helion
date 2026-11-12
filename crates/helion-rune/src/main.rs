//! `helion-ide`: the Helion FPGA IDE (Vivado-style shell on the Rune UI).
//! `helion-ide` opens the window (Start page or the project/file named on the
//! command line); `--headless`, `--source`, `--stdin`, `--version`,
//! `--doctor` and `--help` never open one (see [`helion_rune::cli`]).
//! `--snap out.png --scene name` renders headless for docs and tests (flags
//! from Rune's tooling).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = helion_rune::cli::run(&args) {
        std::process::exit(code);
    }
    let mut spec = helion_rune::app_spec();
    // Headless tooling (`--snap`, `--scene`, `--tree`, scripts) renders the
    // deterministic demo state; a real launch boots the Start page or the
    // project named on the command line, with saved prefs.
    if !args.iter().any(|a| a.starts_with("--")) {
        spec.init = helion_rune::boot;
    }
    rune::run(spec);
}
