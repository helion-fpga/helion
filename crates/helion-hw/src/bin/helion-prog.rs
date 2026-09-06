//! helion-prog — program HAD via sim cable or openFPGALoader USB backend.
use helion_bits::Bitstream;
use helion_device::Device;
use helion_hw::{
    detect_boards, program_hbits_with_cable, prog_sim, resolve_cable, CableBackend, ProgramOutcome,
};
use std::env;
use std::path::PathBuf;
use std::process;

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!(
            "helion-prog [--cable auto|sim|usb|ofl|native] [--part PART] [--flash] [bitstream.hbits]\n\
             \n\
             Detects cables (sim always; USB via openFPGALoader when on PATH),\n\
             loads `.hbits`, programs via TAP CFG_W (sim), openFPGALoader (usb/ofl), or native stub→OFL fallback.\n\
             Never claims DONE on USB without a detected programmer. Empty args\n\
             program an empty bitstream on --cable sim only."
        );
        process::exit(0);
    }
    let mut cable = "auto".to_string();
    let mut part = "HL10T-C32-1".to_string();
    let mut bits_path: Option<PathBuf> = None;
    let mut flash = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cable" => {
                i += 1;
                cable = args.get(i).cloned().unwrap_or_default();
            }
            "--part" => {
                i += 1;
                part = args.get(i).cloned().unwrap_or_else(|| "HL10T-C32-1".into());
            }
            "--flash" => flash = true,
            "--detect" | "detect" => {
                print!("{}", detect_boards().text());
                return;
            }
            other if !other.starts_with('-') => {
                bits_path = Some(PathBuf::from(other));
            }
            other => {
                eprintln!("helion-prog: unknown arg {other}");
                process::exit(2);
            }
        }
        i += 1;
    }
    let info = resolve_cable(&cable).unwrap_or_else(|e| {
        eprintln!("helion-prog: {e}");
        process::exit(2);
    });
    eprintln!(
        "helion-prog: cable {} backend={} — {}",
        info.id,
        info.backend.as_str(),
        info.detail
    );
    let det = detect_boards();
    eprintln!("helion-prog: {}", det.note);
    let dev = Device::load_part(&part).unwrap_or_else(|e| {
        eprintln!("helion-prog: HAD {part}: {e}");
        process::exit(1);
    });
    if let Some(path) = bits_path {
        let outcome = program_hbits_with_cable(&dev, &path, &info, flash).unwrap_or_else(|e| {
            eprintln!("helion-prog: {e}");
            process::exit(1);
        });
        println!("{}", outcome.summary_line("program", &dev.part));
    } else {
        if info.backend != CableBackend::Sim {
            eprintln!(
                "helion-prog: no .hbits given; empty smoke only on --cable sim (got {})",
                info.backend.as_str()
            );
            process::exit(2);
        }
        eprintln!("helion-prog: no .hbits given — programming empty bitstream (smoke)");
        let bits = Bitstream::empty(&dev);
        let st = prog_sim(&dev, &bits).unwrap_or_else(|e| {
            eprintln!("helion-prog: {e}");
            process::exit(1);
        });
        println!(
            "{}",
            ProgramOutcome::Sim { bits, stat: st }.summary_line("program", &dev.part)
        );
    }
}
