use helion_ir::{resolve, CellKind};
use helion_vhdl::{map_vhdl, map_vhdl_sources, synth_vhdl, synth_vhdl_sources};

#[test]
fn vhdl_2008_generic_package_instantiation_and_generic_functions() {
    let src = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

package arith_gen_pkg is
  generic (
    WIDTH  : positive := 8;
    OFFSET : integer  := 1
  );
  subtype word_t is std_logic_vector(WIDTH - 1 downto 0);
  type lane_t is record
    en   : std_logic;
    data : word_t;
  end record;
  function bias_add(x : word_t) return word_t;
end package arith_gen_pkg;

package body arith_gen_pkg is
  function bias_add(x : word_t) return word_t is
  begin
    return std_logic_vector(unsigned(x) + OFFSET);
  end function;
end package body arith_gen_pkg;

package arith4_pkg is new work.arith_gen_pkg
  generic map (
    WIDTH  => 4,
    OFFSET => 3
  );

library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use work.arith4_pkg.all;

entity gen_pkg_top is
  port (
    clk   : in  std_logic;
    inp   : in  lane_t;
    outp  : out word_t
  );
end entity gen_pkg_top;

architecture rtl of gen_pkg_top is
  function scale_xor
    generic (MASK : integer := 5)
    parameter (v : word_t) return word_t is
  begin
    return v xor std_logic_vector(to_unsigned(MASK, 4));
  end function;

  function xor3 is new scale_xor generic map (MASK => 3);
begin
  process(clk)
  begin
    if rising_edge(clk) then
      if inp.en = '1' then
        outp <= bias_add(inp.data);
      else
        outp <= xor3(inp.data);
      end if;
    end if;
  end process;
end architecture rtl;
"#;

    let mr = map_vhdl(src).expect("generic package instantiation + generic function must map");
    assert!(mr.softs.is_empty(), "softs: {:?}", mr.soft_table_lines());
    assert_ne!(mr.design.attrs.get("NO_BODY"), Some("1"));
    let ports: Vec<&str> = mr.design.ports.iter().map(|p| resolve(p.name)).collect();
    assert!(
        ports.iter().any(|p| p.starts_with("inp_en")),
        "expected inp_en in {ports:?}"
    );
    assert!(
        ports.iter().any(|p| p.starts_with("inp_data")),
        "expected inp_data in {ports:?}"
    );
    let ffs = mr
        .design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let luts = mr
        .design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    assert_eq!(ffs, 4, "WIDTH=4 generic package must instantiate 4-bit register");
    assert!(luts >= 4, "expected LUTs from bias_add/xor3, got {luts}");
}

#[test]
fn vhdl_2008_fixed_pkg_ufixed_sfixed_and_float_pkg_float32() {
    let src = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use ieee.fixed_pkg.all;
use ieee.float_pkg.all;

entity dsp_fixed_float is
  port (
    clk   : in  std_logic;
    a_ufx : in  ufixed(3 downto -4);
    b_ufx : in  ufixed(3 downto -4);
    c_sfx : in  sfixed(2 downto -1);
    f_in  : in  float32;
    int_o : out std_logic_vector(3 downto 0);
    frac_o: out std_logic_vector(3 downto 0);
    sgn_o : out std_logic;
    exp_o : out std_logic_vector(7 downto 0)
  );
end entity dsp_fixed_float;

architecture rtl of dsp_fixed_float is
  signal sum_ufx : ufixed(3 downto -4);
begin
  process(clk)
  begin
    if rising_edge(clk) then
      sum_ufx <= a_ufx + b_ufx;
    end if;
  end process;

  -- Negative-index slicing on ufixed(3 downto -4) and float32(8 downto -23)
  int_o  <= to_slv(sum_ufx(3 downto 0));
  frac_o <= to_slv(sum_ufx(-1 downto -4));
  sgn_o  <= c_sfx(2) xor c_sfx(-1) xor f_in(8);
  exp_o  <= to_slv(f_in(7 downto 0));
end architecture rtl;
"#;

    let d = synth_vhdl(src).expect("ufixed/sfixed/float32 negative-index slicing must map");
    assert_ne!(d.attrs.get("NO_BODY"), Some("1"));
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    assert_eq!(ffs, 8, "ufixed(3 downto -4) is 8 bits (4 int + 4 frac)");
    assert!(luts >= 8, "expected adder + xor LUTs, got {luts}");
}

#[test]
fn vhdl_2008_matching_case_question_and_context_clauses() {
    let src = r#"
context ieee_rtl_ctx is
  library ieee;
  use ieee.std_logic_1164.all;
  use ieee.numeric_std.all;
end context ieee_rtl_ctx;

context work.ieee_rtl_ctx;

entity priority_dec is
  port (
    req  : in  std_logic_vector(3 downto 0);
     d0   : in  std_logic_vector(3 downto 0);
    d1   : in  std_logic_vector(3 downto 0);
    d2   : in  std_logic_vector(3 downto 0);
    d3   : in  std_logic_vector(3 downto 0);
    y    : out std_logic_vector(3 downto 0);
    hit  : out std_logic
  );
end entity priority_dec;

architecture rtl of priority_dec is
begin
  process(all)
  begin
    case? req is
      when "1---" =>
        y <= d3;
      when "01--" =>
        y <= d2;
      when "001-" =>
        y <= d1;
      when "0001" =>
        y <= d0;
      when others =>
        y <= (others => '0');
    end case?;
  end process;

  with req select?
    hit <= '1' when "1---" | "01--" | "001-" | "0001",
           '0' when others;
end architecture rtl;
"#;

    let mr = map_vhdl(src).expect("context + case? + select? must map");
    assert!(mr.softs.is_empty(), "softs: {:?}", mr.soft_table_lines());
    let luts = mr
        .design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    assert!(
        luts >= 4,
        "case? priority decoder must synthesize LUTs, got {luts}"
    );
}

#[test]
fn vhdl_2008_unconstrained_arrays_of_records_and_vectors_inferred_from_port_map() {
    let pkg_src = r#"
library ieee;
use ieee.std_logic_1164.all;

package arr_pkg is
  type byte_arr_t is array (natural range <>) of std_logic_vector(3 downto 0);

  type ch_t is record
    valid : std_logic;
    data  : std_logic_vector(3 downto 0);
  end record;

  type ch_arr_t is array (natural range <>) of ch_t;
end package arr_pkg;
"#;

    let child_src = r#"
library ieee;
use ieee.std_logic_1164.all;
use work.arr_pkg.all;

entity ch_merger is
  port (
    chans : in  ch_arr_t;
    bytes : in  byte_arr_t;
    out_v : out std_logic;
    out_d : out std_logic_vector(3 downto 0)
  );
end entity ch_merger;

architecture rtl of ch_merger is
begin
  out_v <= chans(0).valid or chans(1).valid;
  out_d <= (chans(0).data xor bytes(0)) when chans(0).valid = '1'
           else (chans(1).data xor bytes(1));
end architecture rtl;
"#;

    let top_src = r#"
library ieee;
use ieee.std_logic_1164.all;
use work.arr_pkg.all;

entity ch_top is
  port (
    v0, v1 : in  std_logic;
    d0, d1 : in  std_logic_vector(3 downto 0);
    k0, k1 : in  std_logic_vector(3 downto 0);
    y_v    : out std_logic;
    y_d    : out std_logic_vector(3 downto 0)
  );
end entity ch_top;

architecture rtl of ch_top is
  signal s_chans : ch_arr_t(0 to 1);
  signal s_bytes : byte_arr_t(0 to 1);
begin
  s_chans(0).valid <= v0;
  s_chans(0).data  <= d0;
  s_chans(1)       <= (valid => v1, data => d1);
  s_bytes          <= (0 => k0, 1 => k1);

  u_merge: entity work.ch_merger
    port map (
      chans => s_chans,
      bytes => s_bytes,
      out_v => y_v,
      out_d => y_d
    );
end architecture rtl;
"#;

    let d = synth_vhdl_sources(&[
        ("arr_pkg.vhd", pkg_src),
        ("ch_merger.vhd", child_src),
        ("ch_top.vhd", top_src),
    ])
    .expect("unconstrained arrays of records and vectors inferred from port bindings");
    assert_eq!(resolve(d.name), "ch_top");
    let luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    assert!(
        luts >= 5,
        "expected LUTs from ch_merger stitched into ch_top, got {luts}"
    );
}
