//! `helion-rune` binary: the Vivado-style IDE shell on Rune.
//! `cargo run -p helion-rune` opens the window;
//! `cargo run -p helion-rune -- --snap hello.png --scene on` renders headless
//! (flags from `rune/crates/rune/src/tooling.rs`: `--snap/--tree/--scene`).
//!
//! `--version` / `--doctor` never open a window (old `doctor.rs`), so they
//! are intercepted before Rune parses its own flags.

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--version") | Some("-V") | Some("version") => {
            println!("{}", helion_rune::doctor::version_line());
            return;
        }
        Some("--doctor") | Some("doctor") => {
            print!("{}", helion_rune::doctor::doctor_report());
            return;
        }
        _ => {}
    }
    rune::run(helion_rune::app_spec());
}
