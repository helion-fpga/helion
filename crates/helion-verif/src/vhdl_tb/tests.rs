//! End-to-end tests of the VHDL testbench simulator through the same
//! `run_testbench` entry point the IDE uses.

use std::path::{Path, PathBuf};

use crate::tb::{TbRunConfig, TbSev, discover_testbenches, is_testbench_file, parse_testbench, run_testbench};

fn tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("helion-vhdltb-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

fn run(tb: &Path) -> crate::tb::TbOutcome {
    let mut cfg = TbRunConfig::new(tb.to_path_buf());
    cfg.vcd = true;
    cfg.max_units = 100_000;
    run_testbench(&cfg, None).unwrap_or_else(|e| panic!("{e}"))
}

const COUNTER_VHD: &str = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

entity counter4 is
  generic (WIDTH : natural := 4);
  port (
    clk : in  std_logic;
    rst : in  std_logic;
    en  : in  std_logic;
    q   : out std_logic_vector(WIDTH - 1 downto 0)
  );
end entity;

architecture rtl of counter4 is
  signal cnt : unsigned(WIDTH - 1 downto 0) := (others => '0');
begin
  process (clk)
  begin
    if rising_edge(clk) then
      if rst = '1' then
        cnt <= (others => '0');
      elsif en = '1' then
        cnt <= cnt + 1;
      end if;
    end if;
  end process;
  q <= std_logic_vector(cnt);
end architecture;
"#;

const COUNTER_TB: &str = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use std.env.all;

entity counter4_tb is
end entity;

architecture sim of counter4_tb is
  signal clk : std_logic := '0';
  signal rst : std_logic := '1';
  signal en  : std_logic := '0';
  signal q   : std_logic_vector(3 downto 0);

  procedure tick(signal c : in std_logic; n : natural) is
  begin
    for k in 1 to n loop
      wait until rising_edge(c);
    end loop;
  end procedure;
begin
  clk <= not clk after 5 ns;

  dut : entity work.counter4
    generic map (WIDTH => 4)
    port map (clk => clk, rst => rst, en => en, q => q);

  stim : process
  begin
    tick(clk, 2);
    rst <= '0';
    en <= '1';
    tick(clk, 5);
    wait for 1 ns;
    assert unsigned(q) = 5 report "count after 5 enabled edges is " & integer'image(to_integer(unsigned(q))) severity error;
    en <= '0';
    tick(clk, 3);
    wait for 1 ns;
    assert q = "0101" report "count moved while disabled" severity error;
    report "done at " & time'image(now);
    stop;
    wait;
  end process;
end architecture;
"#;

#[test]
fn behavioral_vhdl_dut_and_testbench_pass_with_vcd() {
    let d = tmp_dir("beh");
    write(&d, "counter4.vhd", COUNTER_VHD);
    let tb = write(&d, "counter4_tb.vhd", COUNTER_TB);
    let out = run(&tb);
    assert!(out.passed, "{:#?}", out.log);
    assert_eq!(out.dut_module, "counter4");
    assert_eq!(out.dut_instance, "dut");
    assert!(out.log.iter().any(|l| l.text.contains("report note: done at")), "{:#?}", out.log);
    let vcd = std::fs::read_to_string(out.vcd_path.as_ref().unwrap()).unwrap();
    // The DUT's internal signal is traced (behavioral elaboration).
    assert!(vcd.contains("counter4_tb.dut.cnt"), "{vcd}");
    assert!(vcd.contains("counter4_tb.clk"));
    // Stopped by std.env.stop shortly after the last check (~ 106 ns).
    assert!(out.sim_time_ns > 90 && out.sim_time_ns < 200, "{}", out.sim_time_ns);
    assert!(out.coverage.lines_hit() > 5);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn failing_assert_reports_file_line_and_failure_severity_stops() {
    let d = tmp_dir("fail");
    write(&d, "counter4.vhd", COUNTER_VHD);
    let src = COUNTER_TB.replace("unsigned(q) = 5 report", "unsigned(q) = 6 report");
    let src = src.replace("report \"count moved while disabled\" severity error;", "report \"count moved while disabled\" severity error;\n    assert false report \"fatal stop\" severity failure;\n    report \"never printed\";");
    let tb = write(&d, "counter4_tb.vhd", &src);
    let out = run(&tb);
    assert!(!out.passed);
    assert_eq!(out.failures.len(), 2, "{:#?}", out.failures);
    let f = &out.failures[0];
    let want_line = src.lines().position(|l| l.contains("unsigned(q) = 6")).unwrap() + 1;
    assert_eq!(f.line, want_line);
    assert!(f.file.ends_with("counter4_tb.vhd"));
    assert!(f.message.contains("count after 5 enabled edges is 5"), "{}", f.message);
    assert!(f.signals.iter().any(|(n, v)| n == "dut.q" && *v == 5), "{:?}", f.signals);
    assert!(out.failures[1].message.contains("fatal stop"));
    assert!(!out.log.iter().any(|l| l.text.contains("never printed")));
    assert!(out.log.iter().any(|l| l.sev == TbSev::Error));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn signal_vs_variable_delta_and_after_semantics() {
    let d = tmp_dir("delta");
    let tb = write(
        &d,
        "delta_tb.vhd",
        r#"
library ieee;
use ieee.std_logic_1164.all;
entity delta_tb is end;
architecture sim of delta_tb is
  signal s, a, b : std_logic := '0';
  signal p : std_logic := '0';
  signal t : integer := 0;
begin
  b <= a;            -- one delta behind a
  process
    variable v : std_logic := '0';
  begin
    s <= '1';
    v := '1';
    assert s = '0' report "signal updated before a delta" severity error;
    assert v = '1' report "variable not updated immediately" severity error;
    wait for 0 ns;
    assert s = '1' report "signal not updated after one delta" severity error;
    a <= '1';
    wait on a;
    assert b = '0' report "b followed a in the same delta" severity error;
    wait on b;
    assert b = '1' report "b did not follow a" severity error;
    -- waveform with transport-like elements
    p <= '1' after 10 ns, '0' after 20 ns;
    wait for 15 ns;
    assert p = '1' report "p not high at 15 ns" severity error;
    wait for 10 ns;
    assert p = '0' report "p not low at 25 ns" severity error;
    -- inertial: a later assignment cancels the pending one
    t <= 1 after 10 ns;
    t <= 2 after 5 ns;
    wait for 12 ns;
    assert t = 2 report "inertial assignment did not replace: t=" & integer'image(t) severity error;
    report "ok";
    wait;
  end process;
end;
"#,
    );
    let out = run(&tb);
    assert!(out.passed, "{:#?}", out.log);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn records_arrays_enums_functions_numeric_std_and_textio() {
    let d = tmp_dir("types");
    write(
        &d,
        "util_pkg.vhd",
        r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
package util_pkg is
  type state_t is (IDLE, LOAD, RUN, DONE);
  type sample_t is record
    addr : unsigned(7 downto 0);
    data : std_logic_vector(7 downto 0);
    ok   : boolean;
  end record;
  type mem_t is array (0 to 3) of std_logic_vector(7 downto 0);
  function parity(v : std_logic_vector) return std_logic;
  constant MAGIC : std_logic_vector(7 downto 0) := x"A5";
end package;
package body util_pkg is
  function parity(v : std_logic_vector) return std_logic is
    variable p : std_logic := '0';
  begin
    for i in v'range loop
      p := p xor v(i);
    end loop;
    return p;
  end function;
end package body;
"#,
    );
    let tb = write(
        &d,
        "types_tb.vhd",
        r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use std.textio.all;
use work.util_pkg.all;
entity types_tb is end;
architecture sim of types_tb is
  signal st : state_t := IDLE;
  signal mem : mem_t := (others => (others => '0'));
begin
  process
    variable s : sample_t;
    variable l : line;
    variable acc : integer := 0;
    variable u : unsigned(7 downto 0);
    variable sg : signed(7 downto 0);
  begin
    s := (addr => to_unsigned(3, 8), data => MAGIC, ok => true);
    assert s.addr = 3 and s.ok report "record aggregate" severity error;
    s.data(0) := '0';
    assert s.data = x"A4" report "record element bit write" severity error;
    mem(2) <= MAGIC;
    wait for 1 ns;
    assert mem(2) = x"A5" and mem(1) = x"00" report "array of vectors" severity error;
    assert parity(MAGIC) = '0' and parity(x"A4") = '1' report "package function" severity error;
    for k in state_t loop
      acc := acc + state_t'pos(k);
    end loop;
    assert acc = 6 report "enum loop" severity error;
    st <= RUN;
    wait for 1 ns;
    case st is
      when IDLE | LOAD => report "wrong state" severity error;
      when RUN => acc := 100;
      when others => null;
    end case;
    assert acc = 100 and state_t'image(st) = "run" report "case on enum" severity error;
    u := to_unsigned(250, 8) + 10;            -- wraps modulo 256
    assert to_integer(u) = 4 report "unsigned wrap: " & integer'image(to_integer(u)) severity error;
    sg := to_signed(-3, 8);
    assert to_integer(resize(sg, 16)) = -3 and sg < 0 report "signed" severity error;
    assert shift_left(u, 2) = 16 and (u sll 1) = 8 report "shifts" severity error;
    assert to_hstring(MAGIC) = "A5" report "to_hstring" severity error;
    write(l, string'("acc="));
    write(l, acc);
    writeline(output, l);
    report "types ok";
    wait;
  end process;
end;
"#,
    );
    let out = run(&tb);
    assert!(out.passed, "{:#?}", out.log);
    assert!(out.log.iter().any(|l| l.text == "acc=100"), "{:#?}", out.log);
    let _ = std::fs::remove_dir_all(&d);
}

/// The shipped `examples/counter_tb.vhd` drives the SystemVerilog
/// `examples/counter.sv` (synthesized netlist under the VHDL kernel).
#[test]
fn shipped_vhdl_counter_tb_passes_against_the_sv_counter() {
    let d = tmp_dir("example");
    let ex = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    for f in ["counter.sv", "counter_tb.vhd"] {
        std::fs::copy(ex.join(f), d.join(f)).unwrap();
    }
    let out = run(&d.join("counter_tb.vhd"));
    assert!(out.passed, "{:#?}", out.log);
    assert!(out.log[0].text.contains("synthesized netlist"), "{}", out.log[0].text);
    assert!(out.log.iter().any(|l| l.text.contains("32 edges checked, 0 mismatches")), "{:#?}", out.log);
    let vcd = std::fs::read_to_string(out.vcd_path.as_ref().unwrap()).unwrap();
    assert!(vcd.contains("counter_tb.led"));
    // A wrong expectation is caught at the right line.
    let bad = std::fs::read_to_string(ex.join("counter_tb.vhd")).unwrap().replace("expected := model(3);", "expected := model(2);");
    std::fs::write(d.join("counter_tb.vhd"), &bad).unwrap();
    let out = run(&d.join("counter_tb.vhd"));
    assert!(!out.passed);
    let line = bad.lines().position(|l| l.contains("assert led = expected")).unwrap() + 1;
    assert_eq!(out.failures[0].line, line);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn discovery_and_summary_parse_for_vhdl_testbenches() {
    assert!(is_testbench_file(Path::new("a_tb.vhd")));
    assert!(is_testbench_file(Path::new("tb_a.VHDL")));
    assert!(!is_testbench_file(Path::new("a.vhd")));
    let d = tmp_dir("disc");
    write(&d, "counter4.vhd", COUNTER_VHD);
    let tb = write(&d, "counter4_tb.vhd", COUNTER_TB);
    let found = discover_testbenches(&d);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "counter4_tb");
    let p = parse_testbench(&tb.to_string_lossy(), COUNTER_TB).unwrap();
    assert_eq!(p.module, "counter4_tb");
    assert_eq!(p.dut_module, "counter4");
    assert_eq!(p.port_map.len(), 4);
    assert_eq!(p.clocks.len(), 1);
    assert_eq!(p.clocks[0].half_period_units, 5);
    assert!(p.decls.iter().any(|x| x.name == "q" && x.width == 4));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn runaway_testbench_times_out_and_bad_code_is_a_located_error() {
    let d = tmp_dir("timeout");
    let tb = write(&d, "spin_tb.vhd", "library ieee; use ieee.std_logic_1164.all;\nentity spin_tb is end;\narchitecture a of spin_tb is\n  signal c : std_logic := '0';\nbegin\n  c <= not c after 5 ns;\nend;\n");
    let mut cfg = TbRunConfig::new(tb);
    cfg.max_units = 1_000;
    let e = run_testbench(&cfg, None).unwrap_err();
    assert!(e.msg.contains("timed out"), "{e}");
    let tb = write(&d, "bad_tb.vhd", "entity bad_tb is end;\narchitecture a of bad_tb is\nbegin\n  process begin\n    wait for 1 ns;\n    undefined_thing <= '1';\n  end process;\nend;\n");
    let e = run_testbench(&TbRunConfig::new(tb), None).unwrap_err();
    assert_eq!(e.line, 6);
    assert!(e.msg.contains("undefined_thing"), "{e}");
    let tb = write(&d, "oob_tb.vhd", "library ieee; use ieee.std_logic_1164.all;\nentity oob_tb is end;\narchitecture a of oob_tb is\n  signal v : std_logic_vector(3 downto 0) := \"0000\";\nbegin\n  process\n    variable i : integer := 7;\n  begin\n    wait for 1 ns;\n    v(i) <= '1';\n    wait;\n  end process;\nend;\n");
    let out = run(&tb);
    assert!(!out.passed);
    assert_eq!(out.failures[0].line, 10);
    assert!(out.failures[0].message.contains("out of range"), "{}", out.failures[0].message);
    let _ = std::fs::remove_dir_all(&d);
}
