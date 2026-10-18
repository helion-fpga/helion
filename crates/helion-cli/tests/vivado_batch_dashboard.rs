//! Integration tests for Vivado Batch/Tcl CLI compatibility (`-mode batch|tcl`,
//! `-source <script.tcl>`, `-tclargs ...`, `-log`, `-journal`, `-nojournal`,
//! `-notrace`) and the JSON + Interactive HTML QoR & Timing Summary Dashboard
//! exporter (`--export-dashboard <dir>`).

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn helion_bin() -> &'static str {
    env!("CARGO_BIN_EXE_helion")
}

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helion-vivado-batch-{}-{}",
        name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

#[test]
fn vivado_batch_mode_runs_script_with_tclargs_log_and_journal() {
    let root = workspace_root();
    let dir = scratch_dir("batch-tclargs");
    let script = dir.join("build.tcl");
    let log_file = dir.join("helion_batch.log");
    let jou_file = dir.join("helion_batch.jou");
    let bit_file = dir.join("counter_out.hbits");
    let dash_dir = dir.join("dash");

    let tcl_src = format!(
        r#"# Vivado batch flow script with -tclargs
set design_file [lindex $argv 0]
set top_mod     [lindex $argv 1]
set part_name   [lindex $argv 2]
puts "BATCH_ARGS argc=$argc top=$top_mod part=$part_name script=$argv0"

create_project -force proj_test ./proj_dir -part $part_name
add_files -norecurse $design_file
update_compile_order -fileset sources_1
create_clock -period 10.0 -name clk clk

synth_design -top $top_mod -part $part_name
set luts [llength [get_cells -hierarchical -filter {{PRIMITIVE_TYPE =~ LUT.*}}]]
set ffs  [llength [get_cells -hierarchical -filter {{PRIMITIVE_TYPE =~ FD.*}}]]
puts "CELL_COUNTS luts=$luts ffs=$ffs"

place_design
route_design
report_timing
report_utilization
report_power
report_drc
write_bitstream -force {}
"#,
        bit_file.display()
    );
    fs::write(&script, tcl_src).expect("write script");

    let out = Command::new(helion_bin())
        .current_dir(&root)
        .args([
            "-mode",
            "batch",
            "-source",
            script.to_str().unwrap(),
            "-log",
            log_file.to_str().unwrap(),
            "-journal",
            jou_file.to_str().unwrap(),
            "--export-dashboard",
            dash_dir.to_str().unwrap(),
            "-tclargs",
            "examples/counter.sv",
            "counter",
            "xc7a35tcpg236-1",
        ])
        .output()
        .expect("run helion -mode batch");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "batch run failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("BATCH_ARGS argc=3 top=counter part=xc7a35tcpg236-1"),
        "expected tclargs substitution in stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("CELL_COUNTS luts=5 ffs=4") || stdout.contains("CELL_COUNTS luts=4 ffs=4"),
        "expected get_cells filter counts:\n{stdout}"
    );
    assert!(
        stdout.contains("report_timing counter WNS_PS=9640"),
        "expected gold WNS_PS=9640 in batch output:\n{stdout}"
    );
    assert!(bit_file.exists(), "bitstream file must be written");
    assert!(
        fs::metadata(&bit_file).unwrap().len() > 100,
        "bitstream must be non-empty"
    );

    // Check log and journal files
    assert!(log_file.exists(), "-log file must be created");
    let log_text = fs::read_to_string(&log_file).unwrap();
    assert!(
        log_text.contains("# [trace] synth_design"),
        "trace should appear in log when -notrace is not set:\n{log_text}"
    );
    assert!(
        log_text.contains("report_timing counter WNS_PS=9640"),
        "log must contain timing report:\n{log_text}"
    );

    assert!(jou_file.exists(), "-journal file must be created");
    let jou_text = fs::read_to_string(&jou_file).unwrap();
    assert!(
        jou_text.contains("synth_design -top $top_mod -part $part_name"),
        "journal must record executed Tcl commands:\n{jou_text}"
    );

    // Check exported HTML + JSON dashboard
    let json_path = dash_dir.join("dashboard.json");
    let html_path = dash_dir.join("index.html");
    assert!(json_path.exists(), "dashboard.json must exist");
    assert!(html_path.exists(), "index.html must exist");

    let json_text = fs::read_to_string(&json_path).unwrap();
    assert!(json_text.contains("\"schema\": \"helion.qor_dashboard.v1\""));
    assert!(json_text.contains("\"design\": \"counter\""));
    assert!(json_text.contains("\"wns_ps\": 9640"));
    assert!(json_text.contains("\"timing_met\": true"));

    let html_text = fs::read_to_string(&html_path).unwrap();
    assert!(html_text.contains("<!DOCTYPE html>"));
    assert!(html_text.contains("Helion QoR &amp; Timing Summary Dashboard"));
    assert!(html_text.contains("TIMING MET &amp; DRC CLEAN"));
    assert!(html_text.contains("<svg"));
}

#[test]
fn vivado_nojournal_and_notrace_suppress_journal_and_trace() {
    let root = workspace_root();
    let dir = scratch_dir("nojournal-notrace");
    let script = dir.join("quiet.tcl");
    let log_file = dir.join("quiet.log");
    let jou_file = dir.join("should_not_exist.jou");

    fs::write(
        &script,
        r#"read_sv examples/blinky.sv
synth_design -top blinky -part HL10T-C32-1
puts "QUIET_OK"
"#,
    )
    .unwrap();

    let out = Command::new(helion_bin())
        .current_dir(&root)
        .args([
            "-mode",
            "batch",
            "-notrace",
            "-nojournal",
            "-journal",
            jou_file.to_str().unwrap(),
            "-log",
            log_file.to_str().unwrap(),
            "-source",
            script.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(out.status.success());
    assert!(
        !jou_file.exists(),
        "-nojournal must prevent journal file creation"
    );
    let log_text = fs::read_to_string(&log_file).unwrap();
    assert!(
        !log_text.contains("# [trace]"),
        "-notrace must suppress command trace lines in log:\n{log_text}"
    );
    assert!(log_text.contains("QUIET_OK"));
}

#[test]
fn vivado_tcl_interactive_pipe_mode_and_foreach_benchmark_loop() {
    let root = workspace_root();
    let dir = scratch_dir("tcl-mode-loop");
    let csv_out = dir.join("bench_out.csv");

    let tcl_input = format!(
        r#"set part "xc7a35tcpg236-1"
set designs {{
    {{blinky  "examples/blinky.sv"  "blinky"}}
    {{counter "examples/counter.sv" "counter"}}
}}
set fp [open "{}" w]
puts $fp "design,luts,ffs"
foreach entry $designs {{
    set name [lindex $entry 0]
    set file [lindex $entry 1]
    set top  [lindex $entry 2]
    create_project -force bench_$name ./tmp_$name -part $part
    add_files -norecurse $file
    synth_design -top $top -part $part
    set luts [llength [get_cells -hierarchical -filter {{PRIMITIVE_TYPE =~ LUT.*}}]]
    set ffs  [llength [get_cells -hierarchical -filter {{PRIMITIVE_TYPE =~ FD.*}}]]
    puts $fp "$name,$luts,$ffs"
    close_project
}}
close $fp
puts "TCL_PIPE_DONE"
exit 0
"#,
        csv_out.display()
    );

    let mut child = Command::new(helion_bin())
        .current_dir(&root)
        .args(["-mode", "tcl", "-nojournal", "-notrace"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn helion -mode tcl");

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(tcl_input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "-mode tcl failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("TCL_PIPE_DONE"));
    assert!(csv_out.exists(), "Tcl open/puts/close must write CSV file");
    let csv_text = fs::read_to_string(&csv_out).unwrap();
    assert!(csv_text.contains("design,luts,ffs"));
    assert!(csv_text.contains("blinky,"));
    assert!(csv_text.contains("counter,"));
}

#[test]
fn standalone_and_reports_export_dashboard_flag() {
    let root = workspace_root();
    let dir = scratch_dir("export-dash-subcmd");
    let dash1 = dir.join("d1");
    let dash2 = dir.join("d2");

    // 1. Standalone `helion --export-dashboard <dir> examples/counter.sv`
    let out1 = Command::new(helion_bin())
        .current_dir(&root)
        .args([
            "--export-dashboard",
            dash1.to_str().unwrap(),
            "examples/counter.sv",
        ])
        .output()
        .unwrap();
    assert!(
        out1.status.success(),
        "standalone --export-dashboard failed: {}",
        String::from_utf8_lossy(&out1.stderr)
    );
    assert!(dash1.join("dashboard.json").exists());
    assert!(dash1.join("qor_summary.json").exists());
    assert!(dash1.join("index.html").exists());
    assert!(dash1.join("dashboard.html").exists());

    // 2. Subcommand `helion qor examples/blinky.sv --export-dashboard <dir>`
    let out2 = Command::new(helion_bin())
        .current_dir(&root)
        .args([
            "qor",
            "examples/blinky.sv",
            "--export-dashboard",
            dash2.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out2.status.success());
    let j2 = fs::read_to_string(dash2.join("dashboard.json")).unwrap();
    assert!(j2.contains("\"design\": \"blinky\""));
    assert!(j2.contains("\"wns_ps\": 9700"));
}
