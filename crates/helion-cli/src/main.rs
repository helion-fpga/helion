use helion_bits::{bitgen, bitgen_pblock, eco_lut, readback_lut_init, Bitstream};
use helion_device::Device;
use helion_drc::check_routed;
use helion_fabric::Fabric;
use helion_hw::{prog_sim, Tap};
use helion_ir::Design;
use helion_pack::pack;
use helion_place::{place, place_with, PlaceOpts};
use helion_route::{route, Routed};
use helion_sta::{create_clock, load_sdc, report_timing_routed, TimingResult};
use helion_hls::synth_c_path;
use helion_proj::load_prj;
use helion_sv::synth_sv_path;
use helion_vhdl::synth_vhdl_path;
use std::path::Path;

struct Compiled {
    dev: Device,
    design: Design,
    routed: Routed,
    bits: Bitstream,
    timing: TimingResult,
}

fn synth_any(path: &str) -> Result<helion_ir::Design, String> {
    let p = Path::new(path);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("sv")
        .to_ascii_lowercase();
    match ext.as_str() {
        "vhd" | "vhdl" => synth_vhdl_path(p),
        "c" | "cc" | "cpp" => synth_c_path(p),
        _ => synth_sv_path(p),
    }
}

fn compile_sv(path: &str, part: &str, timing_weight: f64) -> Result<Compiled, String> {
    let design = synth_any(path)?;
    compile_design(design, part, timing_weight)
}

fn compile_design(design: Design, part: &str, timing_weight: f64) -> Result<Compiled, String> {
    let dev = Device::load_part(part).map_err(|e| format!("HAD {part}: {e}"))?;
    let packed = pack(&design, &dev)?;
    let placed = place_with(&packed, &dev, PlaceOpts { timing_weight })?;
    let routed = route(&placed, &dev)?;
    check_routed(&design, &routed, &dev).fail()?;
    let bits = bitgen(&dev, &routed)?;
    let mut clks = Vec::new();
    create_clock(&mut clks, "clk", 10_000, "clk");
    let timing = report_timing_routed(&design, &routed, &clks)?;
    Ok(Compiled {
        dev,
        design,
        routed,
        bits,
        timing,
    })
}

fn main() {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        args.push("--help".into());
    }
    let cmd = args.remove(0);
    match cmd.as_str() {
        "--version" | "-V" | "version" => {
            println!("helion {}", env!("CARGO_PKG_VERSION"));
        }
        "doctor" => doctor(),
        "hw" => hw(args),
        "synth" => cmd_synth(&args),
        "impl" => cmd_impl(&args),
        "run" => cmd_run(&args),
        "report_timing" => cmd_timing(&args),
        "report_utilization" => cmd_util(&args),
        "bitstream" => cmd_bits(&args),
        "eco" => cmd_eco(&args),
        "pblock" => cmd_pblock(&args),
        "qor" => cmd_qor(&args),
        "project" => cmd_project(&args),
        "hnf" => cmd_hnf(&args),
        "--help" | "-h" | "help" => usage(),
        other => {
            eprintln!("unknown command {other}");
            usage();
            std::process::exit(2);
        }
    }
}

fn usage() {
    eprintln!(
        "helion {v}
  helion --version
  helion doctor
  helion synth <file.sv> [--part P]
  helion impl <file.sv> [--part P]
  helion run <file.sv> [--cycles N] [--part P]
  helion report_timing <file.sv> [--sdc f.sdc]
  helion report_utilization <file.sv>
  helion bitstream <file.sv|.vhd|.c> -o out.hbits
  helion eco <file.sv> --cell u_lut --init 0xAAAAAAAAAAAAAAAA
  helion pblock <file.sv>
  helion qor <file.sv>
  helion project <file.prj>
  helion hnf <file.sv> [-o out.hnf]
  helion hw program --cable sim",
        v = env!("CARGO_PKG_VERSION")
    );
}

fn take_flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn positional(args: &[String]) -> Option<&str> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-o" || args[i].starts_with("--") {
            i += 2;
            continue;
        }
        return Some(args[i].as_str());
    }
    None
}

fn cmd_synth(args: &[String]) {
    let path = positional(args).unwrap_or("examples/blinky.sv");
    let part = take_flag(args, "--part").unwrap_or_else(|| "HL10T-C32-1".into());
    let d = synth_any(path).unwrap_or_else(|e| {
        eprintln!("synth: {e}");
        std::process::exit(1);
    });
    let luts = d.lut_inits();
    println!(
        "synth {} cells={} luts={} inits={luts:#x?} part={part}",
        d.name,
        d.cells.len(),
        luts.len()
    );
}

fn cmd_impl(args: &[String]) {
    let path = positional(args).unwrap_or("examples/blinky.sv");
    let part = take_flag(args, "--part").unwrap_or_else(|| "HL10T-C32-1".into());
    let c = compile_sv(path, &part, 0.0).unwrap_or_else(|e| {
        eprintln!("impl: {e}");
        std::process::exit(1);
    });
    println!(
        "impl {} lutffs={} iobs={} brams={} pathfinder_iters={} hops={} frames={}",
        c.design.name,
        c.routed.placed.packed.lutffs.len(),
        c.routed.placed.packed.iobs.len(),
        c.routed.placed.packed.brams.len(),
        c.routed.pathfinder_iters,
        c.routed.iob_src.first().map(|r| r.hops).unwrap_or(0),
        c.bits.frames.len()
    );
}

fn cmd_run(args: &[String]) {
    let path = positional(args).unwrap_or("examples/blinky.sv");
    let part = take_flag(args, "--part").unwrap_or_else(|| "HL10T-C32-1".into());
    let cycles: u32 = take_flag(args, "--cycles")
        .and_then(|s| s.parse().ok())
        .unwrap_or(16);
    let c = compile_sv(path, &part, 0.75).unwrap_or_else(|e| {
        eprintln!("run: {e}");
        std::process::exit(1);
    });
    let mut sim = Fabric::new(&c.dev);
    sim.program(&c.bits).unwrap();
    sim.finish_startup();
    let iob = c.routed.iob_src[0].iob;
    let mut wave = Vec::new();
    let mut changes = 0u32;
    let mut last = sim.led_at(iob.0, iob.1);
    for i in 0..cycles {
        sim.step_user();
        let now = sim.led_at(iob.0, iob.1);
        wave.push(now);
        if now != last {
            changes += 1;
            last = now;
        }
        let _ = i;
    }
    let bits: String = wave.iter().map(|b| if *b { '1' } else { '0' }).collect();
    println!(
        "run {} part={} STAT INIT={} DONE={} EOS={} GWE={} GSR={} GTS={} CRC_ERR={}",
        c.design.name,
        c.dev.part,
        sim.stat.init as u8,
        sim.stat.done as u8,
        sim.stat.eos as u8,
        sim.stat.gwe as u8,
        sim.stat.gsr as u8,
        sim.stat.gts as u8,
        sim.stat.crc_err as u8
    );
    println!(
        "WNS_PS={} R2R_PS={} IOB_PS={} LED[{cycles}]={bits} changes={changes}",
        c.timing.wns_ps, c.timing.r2r_ps, c.timing.iob_ps
    );
    println!("ok");
}

fn cmd_timing(args: &[String]) {
    let path = positional(args).unwrap_or("examples/blinky.sv");
    let part = take_flag(args, "--part").unwrap_or_else(|| "HL10T-C32-1".into());
    let c = compile_sv(path, &part, 0.75).unwrap_or_else(|e| {
        eprintln!("report_timing: {e}");
        std::process::exit(1);
    });
    let mut timing = c.timing.clone();
    if let Some(sdc) = take_flag(args, "--sdc") {
        let text = std::fs::read_to_string(&sdc).unwrap_or_else(|e| {
            eprintln!("sdc {sdc}: {e}");
            std::process::exit(1);
        });
        let mut clks = Vec::new();
        load_sdc(&text, &mut clks).unwrap_or_else(|e| {
            eprintln!("sdc: {e}");
            std::process::exit(1);
        });
        timing = report_timing_routed(&c.design, &c.routed, &clks).unwrap_or_else(|e| {
            eprintln!("report_timing: {e}");
            std::process::exit(1);
        });
    }
    println!(
        "report_timing {} WNS_PS={} TNS_PS={} endpoints={} r2r_ps={} iob_ps={}",
        c.design.name,
        timing.wns_ps,
        timing.tns_ps,
        timing.endpoints,
        timing.r2r_ps,
        timing.iob_ps
    );
}

fn cmd_util(args: &[String]) {
    let path = positional(args).unwrap_or("examples/blinky.sv");
    let part = take_flag(args, "--part").unwrap_or_else(|| "HL10T-C32-1".into());
    let c = compile_sv(path, &part, 0.0).unwrap_or_else(|e| {
        eprintln!("report_utilization: {e}");
        std::process::exit(1);
    });
    let p = &c.routed.placed.packed;
    println!(
        "report_utilization {} LUTFF={}/{} IOB={}/{} BRAM={}/{} DSP={}/{}",
        c.design.name,
        p.lutffs.len(),
        c.dev.lut6_count(),
        p.iobs.len(),
        c.dev.iob_sites().count(),
        p.brams.len(),
        c.dev.n_bram,
        p.macs.len(),
        c.dev.n_dsp
    );
}

fn cmd_bits(args: &[String]) {
    let path = positional(args).unwrap_or("examples/blinky.sv");
    let part = take_flag(args, "--part").unwrap_or_else(|| "HL10T-C32-1".into());
    let out = take_flag(args, "-o").or_else(|| take_flag(args, "--output"));
    let c = compile_sv(path, &part, 0.0).unwrap_or_else(|e| {
        eprintln!("bitstream: {e}");
        std::process::exit(1);
    });
    if let Some(p) = out {
        std::fs::write(&p, &c.bits.packets).unwrap_or_else(|e| {
            eprintln!("write {p}: {e}");
            std::process::exit(1);
        });
        println!("wrote {p} {} bytes", c.bits.packets.len());
    } else {
        println!("bitstream {} bytes={}", c.design.name, c.bits.packets.len());
    }
}

fn cmd_eco(args: &[String]) {
    let path = positional(args).unwrap_or("examples/blinky.sv");
    let part = take_flag(args, "--part").unwrap_or_else(|| "HL10T-C32-1".into());
    let cell = take_flag(args, "--cell").unwrap_or_else(|| "u_lut".into());
    let init = take_flag(args, "--init").unwrap_or_else(|| "0xAAAAAAAAAAAAAAAA".into());
    let new_init = u64::from_str_radix(init.trim_start_matches("0x").trim_start_matches("0X"), 16)
        .unwrap_or(0xAAAA_AAAA_AAAA_AAAA);
    let c = compile_sv(path, &part, 0.0).unwrap_or_else(|e| {
        eprintln!("eco: {e}");
        std::process::exit(1);
    });
    let (site, ble) = c.routed.placed.lutff_sites[0];
    let before = readback_lut_init(&c.dev, &c.bits, site.x, site.y, ble as u32).unwrap();
    let after_bs = eco_lut(&c.dev, &c.routed, &cell, new_init).unwrap_or_else(|e| {
        eprintln!("eco: {e}");
        std::process::exit(1);
    });
    let after = readback_lut_init(&c.dev, &after_bs, site.x, site.y, ble as u32).unwrap();
    println!("eco {cell} INIT {before:#x} -> {after:#x}");
}

fn cmd_pblock(args: &[String]) {
    let path = positional(args).unwrap_or("examples/blinky.sv");
    let part = take_flag(args, "--part").unwrap_or_else(|| "HL10T-C32-1".into());
    let c = compile_sv(path, &part, 0.0).unwrap_or_else(|e| {
        eprintln!("pblock: {e}");
        std::process::exit(1);
    });
    let (site, _) = c.routed.placed.lutff_sites[0];
    let pb = bitgen_pblock(&c.dev, &c.routed, &[(site.x, site.y)]).unwrap();
    println!(
        "pblock CLB_X{}Y{} frames {} / full {}",
        site.x,
        site.y,
        pb.frames.len(),
        c.bits.frames.len()
    );
}

/// One line of QoR for a design: the axes the README table publishes and
/// `crates/helion-cli/tests/qor.rs` gates (LUT, WNS, bitstream size, wall time).
fn cmd_qor(args: &[String]) {
    let path = positional(args).unwrap_or("examples/counter.sv");
    let part = take_flag(args, "--part").unwrap_or_else(|| "HL10T-C32-1".into());
    let t0 = std::time::Instant::now();
    let c = compile_sv(path, &part, 0.75).unwrap_or_else(|e| {
        eprintln!("qor: {e}");
        std::process::exit(1);
    });
    let elapsed_ms = t0.elapsed().as_millis();
    let p = &c.routed.placed.packed;
    println!(
        "qor {} part={} LUTFF={} IOB={} BRAM={} DSP={} WNS_PS={} R2R_PS={} IOB_PS={} FRAMES={} BYTES={} ELAPSED_MS={}",
        c.design.name,
        c.dev.part,
        p.lutffs.len(),
        p.iobs.len(),
        p.brams.len(),
        p.macs.len(),
        c.timing.wns_ps,
        c.timing.r2r_ps,
        c.timing.iob_ps,
        c.bits.frames.len(),
        c.bits.packets.len(),
        elapsed_ms
    );
}

fn cmd_hnf(args: &[String]) {
    let path = positional(args).unwrap_or("examples/blinky.sv");
    let d = synth_any(path).unwrap_or_else(|e| {
        eprintln!("hnf: {e}");
        std::process::exit(1);
    });
    let text = d.to_hnf();
    if let Some(out) = take_flag(args, "-o").or_else(|| take_flag(args, "--output")) {
        std::fs::write(&out, &text).unwrap_or_else(|e| {
            eprintln!("write {out}: {e}");
            std::process::exit(1);
        });
        println!("wrote {out} {} bytes", text.len());
    } else {
        print!("{text}");
    }
}

fn cmd_project(args: &[String]) {
    let path = positional(args).unwrap_or("examples/blinky.prj");
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("project {path}: {e}");
        std::process::exit(1);
    });
    let prj = load_prj(&text).unwrap_or_else(|e| {
        eprintln!("project: {e}");
        std::process::exit(1);
    });
    let src = prj.sources.first().cloned().unwrap_or_default();
    let src_path = {
        let given = Path::new(&src);
        let mut resolved = given.to_path_buf();
        if !given.exists() {
            for anc in Path::new(path).ancestors() {
                let cand = anc.join(&src);
                if cand.exists() {
                    resolved = cand;
                    break;
                }
                if let Some(name) = given.file_name() {
                    let cand = anc.join(name);
                    if cand.exists() {
                        resolved = cand;
                        break;
                    }
                }
            }
        }
        resolved.to_string_lossy().into_owned()
    };
    let c = compile_sv(&src_path, &prj.part, 0.75).unwrap_or_else(|e| {
        eprintln!("project impl: {e}");
        std::process::exit(1);
    });
    println!(
        "project {} part={} source={} lutffs={} PACKAGE_PIN={} create_clock={} frames={}",
        path,
        prj.part,
        src,
        c.routed.placed.packed.lutffs.len(),
        prj.package_pins.len(),
        prj.sdc.len(),
        c.bits.frames.len()
    );
}

fn hw(args: Vec<String>) {
    let mut it = args.into_iter();
    let sub = it.next().unwrap_or_default();
    let mut cable = String::new();
    while let Some(a) = it.next() {
        if a == "--cable" {
            cable = it.next().unwrap_or_default();
        }
    }
    if sub != "program" || cable != "sim" {
        eprintln!("usage: helion hw program --cable sim");
        std::process::exit(2);
    }
    let dev = Device::load_part("HL10T-C32-1").expect("HAD");
    let st = prog_sim(&dev, &Bitstream::empty(&dev)).expect("prog");
    println!(
        "hw sim STAT INIT={} DONE={} EOS={} GWE={} GSR={} GTS={} CRC_ERR={}",
        st.init as u8,
        st.done as u8,
        st.eos as u8,
        st.gwe as u8,
        st.gsr as u8,
        st.gts as u8,
        st.crc_err as u8
    );
}

fn doctor() {
    println!("helion doctor");
    let dev = Device::load_part("HL10T-C32-1").expect("HAD");
    println!("  {}", dev.report_die());
    println!(
        "  HAD {}: {}×{} CLB, {} LUT6, {} BRAM18, idcode {:#010x}",
        dev.part,
        dev.interior_cols,
        dev.interior_rows,
        dev.lut6_count(),
        dev.n_bram,
        dev.idcode
    );
    let loc = dev.locate("CLB_X2Y1.BLE0.LUT.INIT[0]").unwrap();
    let frac = dev.locate("CLB_X2Y1.BLE0.LUT.FRACTURE").unwrap();
    println!(
        "  FeatureMap INIT[0] minor {} bit {}; FRACTURE minor {} bit {}",
        loc.far.minor, loc.bit, frac.far.minor, frac.bit
    );
    let mut tap = Tap::new(&dev);
    let st = tap.program(&Bitstream::empty(&dev)).unwrap();
    println!(
        "  TAP empty STAT INIT={} DONE={} EOS={} GWE={} GSR={} GTS={} CRC_ERR={}",
        st.init as u8,
        st.done as u8,
        st.eos as u8,
        st.gwe as u8,
        st.gsr as u8,
        st.gts as u8,
        st.crc_err as u8
    );
    let c = compile_design(Design::structural_blinky(), "HL10T-C32-1", 0.0).unwrap();
    let _ = place(&c.routed.placed.packed, &dev);
    let _ = Fabric::new(&dev);
    println!("  blinky pack/place/route/bitgen/drc: ok");
    let cc = compile_design(Design::structural_counter(), "HL10T-C32-1", 0.75).unwrap();
    println!(
        "  counter lutffs={} iob_ble={} pathfinder={} WNS_PS={}",
        cc.routed.placed.packed.lutffs.len(),
        cc.routed.iob_src[0].ble,
        cc.routed.pathfinder_iters,
        cc.timing.wns_ps
    );
    println!("ok");
}
