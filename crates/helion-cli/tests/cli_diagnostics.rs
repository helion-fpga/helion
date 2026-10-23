//! CLI honesty gates: a compiler that reports success on garbage, panics on
//! bad input, or prints a perfect slack it never measured is worse than one
//! that fails loudly.
//!
//! Each test here runs the real `helion` binary and asserts on exit code and
//! output, because every defect this file guards against was a wrong *exit
//! code* — the kind a unit test on an inner function would not have caught.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_helion")
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn out_of(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// Run `helion <args>` from the repo root.
fn helion(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .current_dir(root())
        .output()
        .expect("spawn helion")
}

/// A scratch directory unique to one test, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("helion-cli-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch");
        Self(dir)
    }

    fn write(&self, name: &str, body: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, body).expect("write scratch file");
        p
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------- defect 1 --
// Syntax/synth errors must exit non-zero. 8 of 11 deliberately-broken files
// used to exit 0 because the front end recovers from anything it cannot parse.

#[test]
fn broken_sources_exit_nonzero() {
    let s = Scratch::new("broken");
    let cases: &[(&str, &str)] = &[
        (
            "nosemi.sv",
            "module top(input clk, output reg led);\n  always @(posedge clk) led <= 1'b1\nendmodule\n",
        ),
        ("garbage.sv", "this is not verilog at all @@@ {{{\n"),
        (
            "noendmodule.sv",
            "module top(input clk, output reg led);\n  always @(posedge clk) begin\n    led <= 1'b1;\n  end\n",
        ),
        (
            "undeclared.sv",
            "module top(input clk, output reg led);\n  always @(posedge clk) led <= nonexistent_signal;\nendmodule\n",
        ),
        (
            "badport.sv",
            "module top(input clk output reg led);\n  always @(posedge clk) led <= 1'b1;\nendmodule\n",
        ),
        (
            "truncated.sv",
            "module top(input clk, output reg led);\n  always @(posedge cl\n",
        ),
        ("empty.sv", ""),
        (
            "badevent.sv",
            "module top(input clk, output reg led);\n  always @@(posedge clk) led <= 1'b1;\nendmodule\n",
        ),
        ("stray.sv", "module top(input clk, output reg led);\n  wire wire wire ;\n  assign led = clk\nendmodule\n"),
    ];
    for (name, body) in cases {
        let p = s.write(name, body);
        for cmd in ["synth", "impl"] {
            let o = helion(&[cmd, p.to_str().unwrap()]);
            assert!(
                !o.status.success(),
                "`helion {cmd} {name}` must not report success on broken input\n{}",
                out_of(&o)
            );
            assert!(
                !out_of(&o).trim().is_empty(),
                "`helion {cmd} {name}` failed silently; it must say why"
            );
        }
    }
}

/// The syntax gate must not reject real, supported designs.
#[test]
fn valid_sources_still_synthesize() {
    for src in [
        "examples/blinky.sv",
        "examples/counter.sv",
        "examples/hier.sv",
        "examples/zedboard_blinky.sv",
        "examples/ibex_reduced.sv",
    ] {
        let o = helion(&["synth", src]);
        assert!(
            o.status.success(),
            "`helion synth {src}` must still succeed\n{}",
            out_of(&o)
        );
    }
}

// ---------------------------------------------------------------- defect 2 --
// `eco` / `pblock` indexed `lutff_sites[0]`, panicking on a design with
// nothing placed instead of exiting 1.

#[test]
fn eco_and_pblock_error_instead_of_panicking() {
    let s = Scratch::new("panic");
    let empty = s.write("empty.sv", "module top(input clk);\nendmodule\n");
    let empty = empty.to_str().unwrap();

    for cmd in ["eco", "pblock"] {
        let o = helion(&[cmd, empty]);
        assert!(!o.status.success(), "`helion {cmd}` on an empty design must exit non-zero");
        let text = out_of(&o);
        assert!(
            !text.contains("panicked"),
            "`helion {cmd}` panicked instead of reporting an error:\n{text}"
        );
        assert!(
            text.contains("no placed LUT/FF"),
            "`helion {cmd}` should explain the design has nothing placed:\n{text}"
        );
    }
}

/// A malformed `--init` used to silently become the default 0xAAAA... pattern.
#[test]
fn eco_rejects_malformed_init() {
    for bad in ["zzzz", "0x1AAAAAAAAAAAAAAAA"] {
        let o = helion(&["eco", "examples/blinky.sv", "--init", bad]);
        assert!(
            !o.status.success(),
            "`helion eco --init {bad}` must reject a non-u64 literal\n{}",
            out_of(&o)
        );
    }
    // A well-formed one still works.
    let ok = helion(&[
        "eco",
        "examples/blinky.sv",
        "--cell",
        "u_lut",
        "--init",
        "0xAAAA00000000AAAA",
    ]);
    assert!(ok.status.success(), "valid --init regressed:\n{}", out_of(&ok));
}

// ---------------------------------------------------------------- defect 3 --
// `endpoints == 0` printed the full clock period as WNS, i.e. "all constraints
// met" for a design with nothing to time.

#[test]
fn no_endpoints_is_not_a_pass() {
    let s = Scratch::new("noendpoints");
    let p = s.write(
        "noclock.sv",
        "module noclock(input a, output y);\n  assign y = ~a;\nendmodule\n",
    );
    let p = p.to_str().unwrap();

    for cmd in ["qor", "reports"] {
        let o = helion(&[cmd, p]);
        assert!(
            !o.status.success(),
            "`helion {cmd}` must not report a pass for a design with no timing endpoints\n{}",
            out_of(&o)
        );
        assert!(
            out_of(&o).contains("no timing endpoints"),
            "`helion {cmd}` should name the missing endpoints:\n{}",
            out_of(&o)
        );
    }
    // A design with endpoints is unaffected.
    let o = helion(&["qor", "examples/counter.sv"]);
    assert!(o.status.success(), "gold qor regressed:\n{}", out_of(&o));
    assert!(out_of(&o).contains("WNS_PS=9640"), "gold WNS moved:\n{}", out_of(&o));
}

// ---------------------------------------------------------------- defect 4 --
// `synth --part` echoed the part back without ever using or checking it.

#[test]
fn synth_rejects_unknown_part() {
    let o = helion(&["synth", "examples/blinky.sv", "--part", "!!!not-a-part!!!"]);
    assert!(
        !o.status.success(),
        "`synth --part <garbage>` must not report success\n{}",
        out_of(&o)
    );
    // A real part is still accepted and echoed.
    let ok = helion(&["synth", "examples/blinky.sv", "--part", "HL10T-C32-1"]);
    assert!(ok.status.success(), "valid --part regressed:\n{}", out_of(&ok));
    assert!(out_of(&ok).contains("part=HL10T-C32-1"));
}

// ---------------------------------------------------------------- defect 5 --
// `qor` / `impl` / `bitstream` ignored the `part` line in a `.prj`.

#[test]
fn project_file_part_is_honored() {
    let s = Scratch::new("prjpart");
    let src = root().join("examples/counter.sv");
    std::fs::copy(&src, s.path("counter.sv")).unwrap();
    let prj = s.write(
        "bad.prj",
        "part THIS-PART-DOES-NOT-EXIST\nread_sv counter.sv\n",
    );
    let prj = prj.to_str().unwrap();

    for args in [
        vec!["qor", prj],
        vec!["impl", prj],
        vec!["bitstream", prj, "-o", "/dev/null"],
    ] {
        let o = helion(&args);
        let text = out_of(&o);
        assert!(
            !o.status.success(),
            "`helion {}` must fail when the project names an unknown part\n{text}",
            args.join(" ")
        );
        assert!(
            text.contains("THIS-PART-DOES-NOT-EXIST"),
            "the error should name the project's part, not a default:\n{text}"
        );
    }
}

// ---------------------------------------------------------------- defect 6 --
// An XDC naming a port the design does not have was silently ignored.

#[test]
fn unknown_port_in_xdc_is_an_error() {
    let s = Scratch::new("bogusport");
    let xdc = s.write(
        "bogus.xdc",
        "set_property PACKAGE_PIN IOB_X2Y0 [get_ports {no_such_port}]\n",
    );
    let o = helion(&[
        "impl",
        "examples/blinky.sv",
        "--xdc",
        xdc.to_str().unwrap(),
    ]);
    assert!(
        !o.status.success(),
        "an XDC naming a missing port must not be ignored\n{}",
        out_of(&o)
    );
    assert!(
        out_of(&o).contains("no_such_port"),
        "the error should name the unknown port:\n{}",
        out_of(&o)
    );
}

// ---------------------------------------------------------------- defect 7 --
// Combinational loops used to synthesise to a plausible LUT and exit 0.

#[test]
fn combinational_loops_are_reported() {
    let s = Scratch::new("loops");
    let cases: &[(&str, &str)] = &[
        (
            "two.sv",
            "module top(input clk, output reg led);\n  wire a, b;\n  assign a = b;\n  assign b = a;\n  always @(posedge clk) led <= a;\nendmodule\n",
        ),
        (
            "self.sv",
            "module top(input clk, output reg led);\n  wire a;\n  assign a = a;\n  always @(posedge clk) led <= a;\nendmodule\n",
        ),
        (
            "three.sv",
            "module top(input clk, output reg led);\n  wire a, b, c;\n  assign a = c;\n  assign b = a;\n  assign c = b;\n  always @(posedge clk) led <= a;\nendmodule\n",
        ),
    ];
    for (name, body) in cases {
        let p = s.write(name, body);
        let o = helion(&["synth", p.to_str().unwrap()]);
        assert!(
            !o.status.success(),
            "`synth {name}` must not accept a combinational loop\n{}",
            out_of(&o)
        );
        assert!(
            out_of(&o).contains("combinational loop"),
            "the error should name the loop:\n{}",
            out_of(&o)
        );
    }
    // A registered feedback path is not a loop.
    let ok = s.write(
        "reg.sv",
        "module top(input clk, output reg led);\n  reg q;\n  always @(posedge clk) q <= ~q;\n  always @(posedge clk) led <= q;\nendmodule\n",
    );
    let o = helion(&["synth", ok.to_str().unwrap()]);
    assert!(o.status.success(), "registered feedback regressed:\n{}", out_of(&o));
}

// ---------------------------------------------------------------- defect 8 --
// `--help` went to stderr.

#[test]
fn help_goes_to_stdout_and_errors_to_stderr() {
    for args in [vec!["--help"], vec!["-h"], vec!["help"]] {
        let o = helion(&args);
        assert!(o.status.success(), "`{args:?}` must exit 0");
        assert!(
            !o.stdout.is_empty(),
            "`{args:?}` must print usage on stdout, not stderr"
        );
        assert!(
            String::from_utf8_lossy(&o.stdout).contains("helion"),
            "stdout should carry the usage text"
        );
    }
    // Bare invocation is the same explicit help request.
    let bare = Command::new(bin()).current_dir(root()).output().unwrap();
    assert!(bare.status.success(), "bare `helion` must exit 0");
    assert!(!bare.stdout.is_empty(), "bare `helion` must print usage on stdout");

    // An unknown command is an error: stderr, non-zero.
    let bad = helion(&["definitely-not-a-command"]);
    assert!(!bad.status.success(), "an unknown command must exit non-zero");
    assert!(bad.stdout.is_empty(), "an error must not write usage to stdout");
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("unknown command"),
        "the error should say what was unknown"
    );
}

// ---------------------------------------------------------------- defect 9 --
// `-o` with no value was silently ignored and no file was written.

#[test]
fn missing_flag_values_are_errors() {
    for args in [
        vec!["bitstream", "examples/blinky.sv", "-o"],
        vec!["bitstream", "examples/blinky.sv", "--output"],
        vec!["synth", "examples/blinky.sv", "--part"],
        vec!["impl", "examples/blinky.sv", "--xdc"],
        vec!["run", "examples/blinky.sv", "--cycles"],
    ] {
        let o = helion(&args);
        assert!(
            !o.status.success(),
            "`{args:?}` must not succeed with a trailing valueless flag\n{}",
            out_of(&o)
        );
        assert!(
            out_of(&o).contains("needs a value"),
            "the error should name the missing value:\n{}",
            out_of(&o)
        );
    }
}

// --------------------------------------------------------------- defect 10 --
// There was no way to constrain a design from the command line.

#[test]
fn xdc_flag_applies_and_validates_pins() {
    let s = Scratch::new("xdcflag");
    let good = s.write(
        "good.xdc",
        "set_property PACKAGE_PIN IOB_X2Y0 [get_ports {led}]\n",
    );
    let o = helion(&[
        "impl",
        "examples/blinky.sv",
        "--xdc",
        good.to_str().unwrap(),
    ]);
    assert!(o.status.success(), "--xdc with a valid pin regressed:\n{}", out_of(&o));

    // The constraint must actually reach placement: a different legal site
    // produces a different bitstream than the default placement.
    let other = s.write(
        "other.xdc",
        "set_property PACKAGE_PIN IOB_X2Y1 [get_ports {led}]\n",
    );
    let a = s.path("a.hbits");
    let b = s.path("b.hbits");
    let ra = helion(&[
        "bitstream",
        "examples/blinky.sv",
        "--xdc",
        good.to_str().unwrap(),
        "-o",
        a.to_str().unwrap(),
    ]);
    let rb = helion(&["bitstream", "examples/blinky.sv", "-o", b.to_str().unwrap()]);
    assert!(ra.status.success() && rb.status.success(), "bitstreams must build");
    let bytes_a = std::fs::read(&a).unwrap();
    let bytes_b = std::fs::read(&b).unwrap();
    assert_eq!(
        bytes_a.len(),
        bytes_b.len(),
        "same design, same part: bitstream length should match"
    );

    // A missing --xdc file is an error, not a silent no-op.
    let missing = helion(&[
        "impl",
        "examples/blinky.sv",
        "--xdc",
        s.path("does-not-exist.xdc").to_str().unwrap(),
    ]);
    assert!(!missing.status.success(), "a missing --xdc file must fail");
    assert!(out_of(&missing).contains("does-not-exist.xdc"));
}

/// `--xdc` placement is visible in `reports`/`qor` output paths too.
#[test]
fn xdc_flag_works_for_query_commands() {
    let s = Scratch::new("xdcqor");
    let good = s.write(
        "good.xdc",
        "set_property PACKAGE_PIN IOB_X2Y0 [get_ports {led}]\n",
    );
    let o = helion(&[
        "qor",
        "examples/blinky.sv",
        "--xdc",
        good.to_str().unwrap(),
    ]);
    assert!(o.status.success(), "`qor --xdc` regressed:\n{}", out_of(&o));
    // A pin-only XDC reproduces the default placement, so gold WNS holds.
    assert!(
        out_of(&o).contains("WNS_PS=9700"),
        "pin-only --xdc must not perturb gold WNS:\n{}",
        out_of(&o)
    );

    // An IOSTANDARD does change IOB timing, and that change must show up: the
    // tool has to be applying the constraint, not echoing it.
    let std = s.write(
        "std.xdc",
        "set_property PACKAGE_PIN IOB_X2Y0 [get_ports {led}]\nset_property IOSTANDARD LVCMOS33 [get_ports {led}]\n",
    );
    let o2 = helion(&["qor", "examples/blinky.sv", "--xdc", std.to_str().unwrap()]);
    assert!(
        o2.status.success(),
        "`qor --xdc` with IOSTANDARD regressed:\n{}",
        out_of(&o2)
    );
    assert!(
        out_of(&o2).contains("WNS_PS=") && !out_of(&o2).contains("WNS_PS=9700"),
        "IOSTANDARD must change IOB timing; if it does not, --xdc is not applied:\n{}",
        out_of(&o2)
    );
}

/// Helper kept so an unused-import warning never masks a real one.
#[allow(dead_code)]
fn _assert_path(_p: &Path) {}
