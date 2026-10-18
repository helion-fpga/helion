use helion_ir::{resolve, CellKind};
use helion_vhdl::{
    map_vhdl, map_vhdl_files, map_vhdl_path_with_siblings, map_vhdl_sources, synth_vhdl,
    synth_vhdl_files, synth_vhdl_path_with_siblings, synth_vhdl_sources,
};
use std::path::Path;

#[test]
fn vhdl_record_bus_req_rsp_no_longer_bails_out_and_maps_hardware() {
    let src = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

package bus_pkg is
  type bus_req_t is record
    valid : std_logic;
    write : std_logic;
    addr  : std_logic_vector(7 downto 0);
    wdata : std_logic_vector(7 downto 0);
  end record bus_req_t;

  type bus_rsp_t is record
    ready : std_logic;
    err   : std_logic;
    rdata : std_logic_vector(7 downto 0);
  end record;
end package bus_pkg;

library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use work.bus_pkg.all;

entity bus_slave_reg is
  port (
    clk : in  std_logic;
    rst : in  std_logic;
    req : in  bus_req_t;
    rsp : out bus_rsp_t
  );
end entity bus_slave_reg;

architecture rtl of bus_slave_reg is
  signal reg_data : std_logic_vector(7 downto 0) := (others => '0');
  signal rsp_q    : bus_rsp_t;
begin
  process(clk)
  begin
    if rising_edge(clk) then
      if rst = '1' then
        reg_data <= (others => '0');
        rsp_q <= (ready => '0', err => '0', others => (others => '0'));
      else
        rsp_q.ready <= req.valid;
        rsp_q.err   <= req.valid and req.addr(7);
        if req.valid = '1' and req.write = '1' and req.addr(7 downto 0) = x"10" then
          reg_data <= req.wdata;
        end if;
        rsp_q.rdata <= reg_data xor req.addr(7 downto 0);
      end if;
    end if;
  end process;

  rsp <= rsp_q;
end architecture rtl;
"#;

    let mr = map_vhdl(src).expect("record bus_req_t / bus_rsp_t must elaborate");
    assert!(
        mr.softs.is_empty(),
        "record types must not emit record_type soft bailout: {:?}",
        mr.soft_table_lines()
    );
    assert_ne!(
        mr.design.attrs.get("NO_BODY"),
        Some("1"),
        "record entity must not be empty NO_BODY"
    );
    assert_ne!(mr.design.attrs.get("SIM_ONLY"), Some("1"));

    // Verify deterministic, readable flattened port names (<rec>_<field>).
    let port_names: Vec<&str> = mr.design.ports.iter().map(|p| resolve(p.name)).collect();
    assert!(
        port_names.iter().any(|p| p.starts_with("req_valid")),
        "expected flattened req_valid port in {port_names:?}"
    );
    assert!(
        port_names.iter().any(|p| p.starts_with("req_addr")),
        "expected flattened req_addr port in {port_names:?}"
    );
    assert!(
        port_names.iter().any(|p| p.starts_with("req_wdata")),
        "expected flattened req_wdata port in {port_names:?}"
    );
    assert!(
        port_names.iter().any(|p| p.starts_with("rsp_ready")),
        "expected flattened rsp_ready port in {port_names:?}"
    );
    assert!(
        port_names.iter().any(|p| p.starts_with("rsp_rdata")),
        "expected flattened rsp_rdata port in {port_names:?}"
    );

    let luts = mr
        .design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    let ffs = mr
        .design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert!(luts >= 8, "expected real LUT cones for record logic, got {luts}");
    assert!(ffs >= 8, "expected real HFFs for record registers, got {ffs}");
}

#[test]
fn vhdl_record_aggregates_variables_and_nested_records() {
    let src = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

package pkt_pkg is
  type hdr_t is record
    valid : std_logic;
    tag   : std_logic_vector(3 downto 0);
  end record;

  type pkt_t is record
    hdr     : hdr_t;
    payload : std_logic_vector(3 downto 0);
  end record;

  constant IDLE_PKT : pkt_t := (
    hdr     => (valid => '0', tag => x"5"),
    payload => x"A"
  );
end package;

library ieee;
use ieee.std_logic_1164.all;
use work.pkt_pkg.all;

entity pkt_pipe is
  port (
    clk    : in  std_logic;
    en     : in  std_logic;
    in_pkt : in  pkt_t;
    out_p  : out pkt_t;
    same_o : out std_logic
  );
end entity;

architecture rtl of pkt_pipe is
  signal stage : pkt_t;
begin
  process(clk)
    variable tmp : pkt_t;
  begin
    if rising_edge(clk) then
      if en = '1' then
        tmp := in_pkt;
        tmp.payload := in_pkt.payload xor in_pkt.hdr.tag(3 downto 0);
        stage <= tmp;
      else
        stage <= (
          hdr     => (valid => '1', others => (others => '0')),
          others  => (others => '0')
        );
      end if;
    end if;
  end process;

  out_p  <= stage when en = '1' else IDLE_PKT;
  same_o <= '1' when stage = in_pkt else '0';
end architecture;
"#;

    let d = synth_vhdl(src).expect("nested record + variable + aggregate + compare must map");
    assert_ne!(d.attrs.get("NO_BODY"), Some("1"));
    let port_names: Vec<&str> = d.ports.iter().map(|p| resolve(p.name)).collect();
    assert!(
        port_names.iter().any(|p| p.starts_with("in_pkt_hdr_valid")),
        "nested record port in_pkt_hdr_valid missing in {port_names:?}"
    );
    assert!(
        port_names.iter().any(|p| p.starts_with("out_p_hdr_tag")),
        "nested record port out_p_hdr_tag missing in {port_names:?}"
    );
    assert!(
        port_names.iter().any(|p| p.starts_with("out_p_payload")),
        "nested record port out_p_payload missing in {port_names:?}"
    );
    assert!(
        d.cells.iter().any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
        "expected LUT6 cells"
    );
    assert!(
        d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)),
        "expected HFF cells"
    );
}

#[test]
fn vhdl_multifile_cross_package_separate_entity_arch_and_hierarchy() {
    let pkg_vhd = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

package soc_pkg is
  constant DATA_W : integer := 4;

  subtype word_t is std_logic_vector(DATA_W - 1 downto 0);

  type bus_req_t is record
    valid : std_logic;
    op    : std_logic;
    a     : word_t;
    b     : word_t;
  end record;

  type bus_rsp_t is record
    ack : std_logic;
    y   : word_t;
  end record;

  function mix_words(x : word_t; y : word_t) return word_t;
end package soc_pkg;

package body soc_pkg is
  function mix_words(x : word_t; y : word_t) return word_t is
  begin
    return x xor y;
  end function;
end package body soc_pkg;
"#;

    let child_ent_vhd = r#"
library ieee;
use ieee.std_logic_1164.all;
use work.soc_pkg.all;

entity alu_leaf is
  port (
    req : in  bus_req_t;
    rsp : out bus_rsp_t
  );
end entity alu_leaf;
"#;

    let child_arch_vhd = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use work.soc_pkg.all;

architecture rtl of alu_leaf is
begin
  rsp.ack <= req.valid;
  rsp.y   <= std_logic_vector(unsigned(req.a) + unsigned(req.b))
             when req.op = '1'
             else mix_words(req.a, req.b);
end architecture rtl;
"#;

    let top_vhd = r#"
library ieee;
use ieee.std_logic_1164.all;
use work.soc_pkg.all;

entity soc_top is
  port (
    clk   : in  std_logic;
    req_i : in  bus_req_t;
    rsp_o : out bus_rsp_t
  );
end entity soc_top;

architecture rtl of soc_top is
  signal comb_rsp : bus_rsp_t;
begin
  u_alu: entity work.alu_leaf
    port map (
      req => req_i,
      rsp => comb_rsp
    );

  process(clk)
  begin
    if rising_edge(clk) then
      rsp_o <= comb_rsp;
    end if;
  end process;
end architecture rtl;
"#;

    // Order 1: package -> child_entity -> child_arch -> top
    let files_topo = [
        ("soc_pkg.vhd", pkg_vhd),
        ("alu_leaf_ent.vhd", child_ent_vhd),
        ("alu_leaf_arch.vhd", child_arch_vhd),
        ("soc_top.vhd", top_vhd),
    ];
    let mr = map_vhdl_sources(&files_topo).expect("multi-file map_vhdl_sources");
    assert!(
        mr.softs.is_empty(),
        "resolved cross-file child must not emit missing_component: {:?}",
        mr.soft_table_lines()
    );
    assert_eq!(resolve(mr.design.name), "soc_top");
    assert_ne!(mr.design.attrs.get("NO_BODY"), Some("1"));
    let luts = mr
        .design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    let ffs = mr
        .design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert!(luts >= 4, "expected LUTs from alu_leaf stitched into soc_top, got {luts}");
    assert!(ffs >= 5, "expected FFs for registered bus_rsp_t, got {ffs}");

    // Order 2: reverse order (top before child and package) must elaborate identically!
    let files_rev = [
        ("soc_top.vhd", top_vhd),
        ("alu_leaf_arch.vhd", child_arch_vhd),
        ("alu_leaf_ent.vhd", child_ent_vhd),
        ("soc_pkg.vhd", pkg_vhd),
    ];
    let d_rev = synth_vhdl_sources(&files_rev).expect("reverse-order synth_vhdl_sources");
    assert_eq!(resolve(d_rev.name), "soc_top");
    assert_eq!(d_rev.cells.len(), mr.design.cells.len());
}

#[test]
fn vhdl_multifile_disk_files_and_path_with_siblings() {
    let tmp = std::env::temp_dir().join(format!("helion_vhdl_multifile_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");

    let pkg_path = tmp.join("01_defs_pkg.vhd");
    let child_path = tmp.join("02_child_unit.vhd");
    let top_path = tmp.join("03_top_unit.vhd");

    std::fs::write(
        &pkg_path,
        r#"
library ieee;
use ieee.std_logic_1164.all;

package defs_pkg is
  constant W : integer := 4;
  type pair_t is record
    lhs : std_logic_vector(W-1 downto 0);
    rhs : std_logic_vector(W-1 downto 0);
  end record;
end package;
"#,
    )
    .unwrap();

    std::fs::write(
        &child_path,
        r#"
library ieee;
use ieee.std_logic_1164.all;
use work.defs_pkg.all;

entity child_unit is
  port (
    p : in  pair_t;
    y : out std_logic_vector(W-1 downto 0)
  );
end entity;

architecture rtl of child_unit is
begin
  y <= p.lhs xor p.rhs;
end architecture;
"#,
    )
    .unwrap();

    std::fs::write(
        &top_path,
        r#"
library ieee;
use ieee.std_logic_1164.all;
use work.defs_pkg.all;

entity top_unit is
  port (
    a : in  std_logic_vector(W-1 downto 0);
    b : in  std_logic_vector(W-1 downto 0);
    q : out std_logic_vector(W-1 downto 0)
  );
end entity;

architecture rtl of top_unit is
  component child_unit is
    port (
      p : in  pair_t;
      y : out std_logic_vector(W-1 downto 0)
    );
  end component;
  signal s_pair : pair_t;
begin
  s_pair <= (lhs => a, rhs => b);
  u_child: child_unit port map (p => s_pair, y => q);
end architecture;
"#,
    )
    .unwrap();

    let paths: Vec<&Path> = vec![&pkg_path, &child_path, &top_path];
    let mr = map_vhdl_files(&paths).expect("map_vhdl_files");
    assert!(mr.softs.is_empty(), "softs: {:?}", mr.soft_table_lines());
    let d_files = synth_vhdl_files(&paths).expect("synth_vhdl_files");
    assert_eq!(d_files.lut_inits().len(), 4);

    let mr_sib = map_vhdl_path_with_siblings(&top_path).expect("map_vhdl_path_with_siblings");
    assert!(mr_sib.softs.is_empty());
    let d_sib = synth_vhdl_path_with_siblings(&top_path).expect("synth_vhdl_path_with_siblings");
    assert_eq!(d_sib.lut_inits().len(), 4);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn vhdl_mixed_language_vhdl_and_sv_hierarchy() {
    let sv_child = r#"
module sv_xor_leaf(
    input logic [3:0] a,
    input logic [3:0] b,
    output logic [3:0] y
);
    assign y = a ^ b;
endmodule
"#;

    let vhdl_top = r#"
library ieee;
use ieee.std_logic_1164.all;

entity mixed_top is
  port (
    x : in  std_logic_vector(3 downto 0);
    z : in  std_logic_vector(3 downto 0);
    o : out std_logic_vector(3 downto 0)
  );
end entity;

architecture rtl of mixed_top is
  component sv_xor_leaf is
    port (
      a : in  std_logic_vector(3 downto 0);
      b : in  std_logic_vector(3 downto 0);
      y : out std_logic_vector(3 downto 0)
    );
  end component;
begin
  u_sv: sv_xor_leaf port map (a => x, b => z, y => o);
end architecture;
"#;

    let mr = map_vhdl_sources(&[("sv_xor_leaf.sv", sv_child), ("mixed_top.vhd", vhdl_top)])
        .expect("mixed VHDL top + SV child");
    assert!(
        mr.softs.is_empty(),
        "SV child must be recognized as known body: {:?}",
        mr.soft_table_lines()
    );
    assert_eq!(mr.design.lut_inits().len(), 4);
}
