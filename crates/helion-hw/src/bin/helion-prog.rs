//! helion-prog — program HAD via sim cable (or `.hbits` path). Physical USB TBD.
use helion_bits::Bitstream;
use helion_device::Device;
use helion_hw::{detect_boards, program_hbits_path, prog_sim, resolve_cable};
use std::env;
use std::path::PathBuf;
use std::process;

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!(
            "helion-prog [--cable sim] [--part PART] [bitstream.hbits]\n\
             \n\
             Detects cables (sim always; no physical HAD USB yet), loads `.hbits`,\n\
             programs via TAP CFG_W, prints STAT. Empty args program an empty bitstream."
        );
        process::exit(0);
    }
    let mut cable = "sim".to_string();
    let mut part = "HL10T-C32-1".to_string();
    let mut bits_path: Option<PathBuf> = None;
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
    eprintln!("helion-prog: cable {} — {}", info.id, info.detail);
    let det = detect_boards();
    if !det.physical_had {
        eprintln!("helion-prog: {}", det.note);
    }
    let dev = Device::load_part(&part).unwrap_or_else(|e| {
        eprintln!("helion-prog: HAD {part}: {e}");
        process::exit(1);
    });
    let st = if let Some(path) = bits_path {
        let (_bits, st) = program_hbits_path(&dev, &path).unwrap_or_else(|e| {
            eprintln!("helion-prog: {e}");
            process::exit(1);
        });
        st
    } else {
        eprintln!("helion-prog: no .hbits given — programming empty bitstream (smoke)");
        prog_sim(&dev, &Bitstream::empty(&dev)).unwrap_or_else(|e| {
            eprintln!("helion-prog: {e}");
            process::exit(1);
        })
    };
    println!(
        "helion-prog sim STAT INIT={} DONE={} EOS={} GWE={} GSR={} GTS={} CRC_ERR={}",
        st.init as u8,
        st.done as u8,
        st.eos as u8,
        st.gwe as u8,
        st.gsr as u8,
        st.gts as u8,
        st.crc_err as u8
    );
}
