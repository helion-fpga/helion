use helion_ir::{resolve, CellKind, PortDir};
use helion_vhdl::{
    map_mixed_sources, map_vhdl, synth_mixed_sources, synth_vhdl_with_sva, vhdl_to_sv_str,
};

#[test]
fn vhdl_2019_record_mode_views_and_converse_bidirectional_ports() {
    let src = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

package stream_pkg is
  type stream_bus_t is record
    valid : std_logic;
    data  : std_logic_vector(3 downto 0);
    ready : std_logic;
    ack   : std_logic;
  end record stream_bus_t;

  view master_view of stream_bus_t is
    valid, data : out;
    ready, ack  : in;
  end view master_view;

  view slave_view of stream_bus_t is
    valid, data : in;
    ready, ack  : out;
  end view slave_view;
end package stream_pkg;

library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use work.stream_pkg.all;

entity stream_master is
  port (
    clk   : in  std_logic;
    en    : in  std_logic;
    din   : in  std_logic_vector(3 downto 0);
    bus_m : view master_view;
    done  : out std_logic
  );
end entity stream_master;

architecture rtl of stream_master is
  signal d_q : std_logic_vector(3 downto 0) := (others => '0');
begin
  process(clk)
  begin
    if rising_edge(clk) then
      if en = '1' and bus_m.ready = '1' then
        d_q <= din;
      end if;
    end if;
  end process;

  bus_m.valid <= en;
  bus_m.data  <= d_q;
  done        <= bus_m.ack and bus_m.ready;
end architecture rtl;

library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use work.stream_pkg.all;

entity stream_slave is
  port (
    clk   : in  std_logic;
    -- VHDL-2019 'converse attribute flips master_view into slave_view!
    bus_s : view master_view'converse;
    q_out : out std_logic_vector(3 downto 0)
  );
end entity stream_slave;

architecture rtl of stream_slave is
  signal r_q   : std_logic_vector(3 downto 0) := (others => '0');
  signal ack_q : std_logic := '0';
begin
  process(clk)
  begin
    if rising_edge(clk) then
      ack_q <= bus_s.valid;
      if bus_s.valid = '1' then
        r_q <= bus_s.data xor x"A";
      end if;
    end if;
  end process;

  bus_s.ready <= '1';
  bus_s.ack   <= ack_q;
  q_out       <= r_q;
end architecture rtl;

library ieee;
use ieee.std_logic_1164.all;
use work.stream_pkg.all;

entity stream_system_top is
  port (
    clk   : in  std_logic;
    en    : in  std_logic;
    din   : in  std_logic_vector(3 downto 0);
    done  : out std_logic;
    q_out : out std_logic_vector(3 downto 0)
  );
end entity stream_system_top;

architecture rtl of stream_system_top is
  signal link : stream_bus_t;
begin
  u_m: entity work.stream_master
    port map (
      clk   => clk,
      en    => en,
      din   => din,
      bus_m => link,
      done  => done
    );

  u_s: entity work.stream_slave
    port map (
      clk   => clk,
      bus_s => link,
      q_out => q_out
    );
end architecture rtl;
"#;

    let sv = vhdl_to_sv_str(src).expect("vhdl_to_sv_str with VHDL-2019 views");
    assert!(
        sv.contains("output logic bus_m_valid")
            && sv.contains("output logic [3:0] bus_m_data")
            && sv.contains("input logic bus_m_ready")
            && sv.contains("input logic bus_m_ack"),
        "master_view must assign per-field output/input directions: {sv}"
    );
    assert!(
        sv.contains("input logic bus_s_valid")
            && sv.contains("input logic [3:0] bus_s_data")
            && sv.contains("output logic bus_s_ready")
            && sv.contains("output logic bus_s_ack"),
        "master_view'converse must flip per-field directions: {sv}"
    );

    let mr = map_vhdl(src).expect("stream_system_top with VHDL-2019 mode views must map");
    assert!(mr.softs.is_empty(), "softs: {:?}", mr.soft_table_lines());
    assert_eq!(resolve(mr.design.name), "stream_system_top");
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
    assert!(ffs >= 5, "expected FFs from master + slave across view link, got {ffs}");
    assert!(luts >= 4, "expected LUTs from master + slave across view link, got {luts}");
}

#[test]
fn vhdl_2019_leaf_view_port_directions_in_design() {
    let src = r#"
library ieee;
use ieee.std_logic_1164.all;

package v_pkg is
  type hs_t is record
    req : std_logic;
    gnt : std_logic;
  end record;

  view initiator_v of hs_t is
    req : out;
    gnt : in;
  end view;
end package;

library ieee;
use ieee.std_logic_1164.all;
use work.v_pkg.all;

entity initiator_leaf is
  port (
    clk : in  std_logic;
    go  : in  std_logic;
    hs  : view initiator_v;
    ok  : out std_logic
  );
end entity;

architecture rtl of initiator_leaf is
  signal req_q : std_logic := '0';
begin
  process(clk)
  begin
    if rising_edge(clk) then
      req_q <= go and not hs.gnt;
    end if;
  end process;
  hs.req <= req_q;
  ok     <= req_q and hs.gnt;
end architecture;
"#;

    let mr = map_vhdl(src).expect("leaf with view port");
    assert!(mr.softs.is_empty());
    let hs_req = mr
        .design
        .ports
        .iter()
        .find(|p| resolve(p.name) == "hs_req")
        .expect("hs_req port");
    let hs_gnt = mr
        .design
        .ports
        .iter()
        .find(|p| resolve(p.name) == "hs_gnt")
        .expect("hs_gnt port");
    assert_eq!(hs_req.dir, PortDir::Out, "hs.req must be PortDir::Out");
    assert_eq!(hs_gnt.dir, PortDir::In, "hs.gnt must be PortDir::In");
}

#[test]
fn vhdl_psl_assertions_and_covers_lower_to_sva_and_elaborate() {
    let src = r#"
library ieee;
use ieee.std_logic_1164.all;

entity req_ack_pipe is
  port (
    clk : in  std_logic;
    req : in  std_logic;
    err : in  std_logic;
    ack : out std_logic
  );
end entity req_ack_pipe;

architecture rtl of req_ack_pipe is
  signal ack_q : std_logic := '0';
  default clock is rising_edge(clk);
begin
  process(clk)
  begin
    if rising_edge(clk) then
      ack_q <= req;
    end if;
  end process;

  ack <= ack_q;

  -- Native VHDL-2008/2019 PSL assertions + cover
  p_req_next_ack : assert always (req -> next ack_q);
  p_no_err       : assert never (err = '1' and ack_q = '1');
  c_handshake    : cover {req; ack_q};

  -- Embedded comment PSL directive
  -- psl p_comment_implication : assert always (req -> next[1] ack_q);
end architecture rtl;
"#;

    let sv = vhdl_to_sv_str(src).expect("vhdl_to_sv_str with PSL");
    assert!(
        sv.contains("p_req_next_ack: assert property (@(posedge clk) (req) |=> (ack_q));"),
        "expected SVA |=> from `req -> next ack_q` in:\n{sv}"
    );
    assert!(
        sv.contains("p_no_err: assert property (@(posedge clk) 1'b1 |-> ~("),
        "expected SVA from `assert never` in:\n{sv}"
    );
    assert!(
        sv.contains("c_handshake: cover property (@(posedge clk)"),
        "expected SVA cover property in:\n{sv}"
    );
    assert!(
        sv.contains("p_comment_implication: assert property (@(posedge clk)"),
        "expected SVA from `-- psl` comment directive in:\n{sv}"
    );

    let sva_out = synth_vhdl_with_sva(src).expect("synth_vhdl_with_sva must elaborate SVA");
    assert_eq!(
        sva_out.properties.len(),
        3,
        "all 3 PSL assertions must elaborate into SvaProperty sidecars"
    );
    assert_eq!(sva_out.design.attrs.get("PSL_ASSERT_COUNT"), Some("3"));
    assert_eq!(sva_out.design.attrs.get("PSL_COVER_COUNT"), Some("1"));
}

#[test]
fn vhdl_mixed_sources_multi_level_vhdl_sv_hierarchy() {
    let vhdl_pkg = r#"
library ieee;
use ieee.std_logic_1164.all;

package mix_pkg is
  type nib_pair_t is record
    a : std_logic_vector(3 downto 0);
    b : std_logic_vector(3 downto 0);
  end record;
end package;
"#;

    let sv_alu = r#"
module sv_nibble_alu(
    input logic [3:0] a,
    input logic [3:0] b,
    output logic [3:0] sum_x
);
    assign sum_x = (a ^ b);
endmodule
"#;

    let vhdl_mid = r#"
library ieee;
use ieee.std_logic_1164.all;
use work.mix_pkg.all;

entity vhdl_mid_stage is
  port (
    clk  : in  std_logic;
    pair : in  nib_pair_t;
    q    : out std_logic_vector(3 downto 0)
  );
end entity;

architecture rtl of vhdl_mid_stage is
  signal comb_y : std_logic_vector(3 downto 0);
begin
  u_sv_alu: entity work.sv_nibble_alu
    port map (
      a     => pair.a,
      b     => pair.b,
      sum_x => comb_y
    );

  process(clk)
  begin
    if rising_edge(clk) then
      q <= comb_y;
    end if;
  end process;
end architecture;
"#;

    let sv_top = r#"
module sv_top_wrapper(
    input logic clk,
    input logic [3:0] in_a,
    input logic [3:0] in_b,
    output logic [3:0] out_q
);
    vhdl_mid_stage u_mid(
        .clk(clk),
        .pair_a(in_a),
        .pair_b(in_b),
        .q(out_q)
    );
endmodule
"#;

    let mr = map_mixed_sources(&[
        ("mix_pkg.vhd", vhdl_pkg),
        ("sv_nibble_alu.sv", sv_alu),
        ("vhdl_mid_stage.vhd", vhdl_mid),
        ("sv_top_wrapper.sv", sv_top),
    ])
    .expect("3-level SV top -> VHDL mid -> SV leaf hierarchy must map");
    assert!(mr.softs.is_empty(), "softs: {:?}", mr.soft_table_lines());
    assert_eq!(resolve(mr.design.name), "sv_top_wrapper");

    let d = synth_mixed_sources(&[
        ("mix_pkg.vhd", vhdl_pkg),
        ("sv_nibble_alu.sv", sv_alu),
        ("vhdl_mid_stage.vhd", vhdl_mid),
        ("sv_top_wrapper.sv", sv_top),
    ])
    .expect("synth_mixed_sources");
    let xor_luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { init: 0x6666_6666_6666_6666 }))
        .count();
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(xor_luts, 4, "expected 4 XOR LUTs from sv_nibble_alu");
    assert_eq!(ffs, 4, "expected 4 HFFs from vhdl_mid_stage");
}
