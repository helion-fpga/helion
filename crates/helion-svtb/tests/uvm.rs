//! Stage f/g: the UVM library running the shipped examples/uvm_counter
//! environment against examples/counter.sv, and a failing variant.

mod common;
use common::*;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Copy the example into a temp dir with `counter_sv` as the DUT.
fn stage(name: &str, counter_sv: &str) -> PathBuf {
    let d = tmp(name);
    for f in ["counter_if.sv", "counter_pkg.sv", "uvm_counter_tb.sv"] {
        std::fs::copy(repo().join("examples/uvm_counter").join(f), d.join(f)).unwrap();
    }
    std::fs::write(d.join("counter.sv"), counter_sv).unwrap();
    d
}

fn run_example(d: &Path, plusargs: &[&str]) -> Out {
    let src = std::fs::read_to_string(d.join("uvm_counter_tb.sv")).unwrap();
    run_in(d, "uvm_counter_tb.sv", &src, plusargs).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn uvm_counter_example_passes_with_report_summary() {
    let counter = std::fs::read_to_string(repo().join("examples/counter.sv")).unwrap();
    let d = stage("uvm_ok", &counter);
    let o = run_example(&d, &[]);
    let t = o.text();
    assert!(o.res.failures.is_empty(), "{:?}\n{t}", o.res.failures);
    assert!(o.has("UVM_INFO @ 0: reporter [RNTST] Running test counter_test..."), "{t}");
    assert!(o.has("uvm_test_top (counter_test)") && o.has("    drv (counter_driver)"), "topology: {t}");
    assert!(o.has("--- UVM Report Summary ---") && o.has("UVM_ERROR : 0") && o.has("UVM_FATAL : 0"), "{t}");
    assert!(o.lines.iter().any(|l| l.contains("uvm_test_top.env.scb [SCB]") && l.contains("mismatch(es)") && l.contains(", 0 mismatch")), "{t}");
    assert!(o.lines.iter().any(|l| l.contains("counter_pkg.sv(") && l.contains("[TEST] driver ran")), "file(line) in messages: {t}");
    assert!(o.res.finished);
}

#[test]
fn uvm_testname_plusarg_and_factory_override_select_the_long_test() {
    let counter = std::fs::read_to_string(repo().join("examples/counter.sv")).unwrap();
    let d = stage("uvm_long", &counter);
    let o = run_example(&d, &["+UVM_TESTNAME=counter_long_test"]);
    let t = o.text();
    assert!(o.res.failures.is_empty(), "{:?}\n{t}", o.res.failures);
    assert!(o.has("Running test counter_long_test..."), "{t}");
    assert!(o.has("[TEST] driver ran 40 cycle(s)"), "override swapped in counter_long_seq: {t}");
}

#[test]
fn uvm_counter_against_a_buggy_dut_fails_with_scoreboard_errors() {
    let buggy = r#"
module counter (input logic clk, output logic led);
    logic [3:0] cnt;
    always_ff @(posedge clk) cnt <= cnt + 1;
    assign led = cnt[2];
endmodule
"#;
    let d = stage("uvm_bug", buggy);
    let o = run_example(&d, &[]);
    let t = o.text();
    assert!(!o.res.failures.is_empty(), "{t}");
    let f = &o.res.failures[0];
    assert!(f.file.ends_with("counter_pkg.sv"), "{f:?}");
    let src = std::fs::read_to_string(d.join("counter_pkg.sv")).unwrap();
    let line = src.lines().nth(f.line - 1).unwrap();
    assert!(line.contains("uvm_error(\"SCB\""), "failure points at the uvm_error call: {line}");
    assert!(f.msg.contains("UVM_ERROR") && f.msg.contains("[SCB] cycle 5: led=1, expected 0"), "{f:?}");
    assert!(o.lines.iter().any(|l| l.starts_with("UVM_ERROR : ") && l != "UVM_ERROR : 0"), "{t}");
}

#[test]
fn unknown_uvm_testname_is_fatal() {
    let counter = std::fs::read_to_string(repo().join("examples/counter.sv")).unwrap();
    let d = stage("uvm_bad_name", &counter);
    let o = run_example(&d, &["+UVM_TESTNAME=nope_test"]);
    assert_eq!(o.res.failures.len(), 1, "{}", o.text());
    assert!(o.res.failures[0].msg.contains("UVM_FATAL") && o.res.failures[0].msg.contains("nope_test"));
}

#[test]
fn uvm_tests_are_discovered_statically() {
    let counter = std::fs::read_to_string(repo().join("examples/counter.sv")).unwrap();
    let d = stage("uvm_list", &counter);
    let prog = helion_svtb::compile(&[d.join("uvm_counter_tb.sv")], &Default::default()).unwrap();
    assert_eq!(helion_svtb::uvm_tests(&prog), ["counter_long_test", "counter_test"]);
    assert!(prog.uses_uvm);
}

