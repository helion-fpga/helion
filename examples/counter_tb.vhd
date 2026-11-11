-- Self-checking VHDL testbench for counter.sv.
--
-- Mixed language: the SystemVerilog DUT is synthesized and stepped as a
-- netlist while this testbench runs in Helion's behavioral VHDL simulator.
-- The 4-bit counter increments on every rising clock edge and led mirrors
-- cnt(3); a numeric_std model of the counter predicts led after each edge.
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use std.env.all;

entity counter_tb is
end entity;

architecture sim of counter_tb is
  constant CLK_PERIOD : time := 10 ns;
  constant EDGES      : natural := 32;

  signal clk : std_logic := '0';
  signal led : std_logic;
begin
  clk <= not clk after CLK_PERIOD / 2;

  dut : entity work.counter
    port map (clk => clk, led => led);

  check : process
    variable model    : unsigned(3 downto 0) := (others => '0');
    variable expected : std_logic;
    variable errors   : natural := 0;
  begin
    report "counter_tb: checking led = cnt(3) over " & integer'image(EDGES) & " edges";
    for i in 1 to EDGES loop
      wait until rising_edge(clk);
      model := model + 1;
      wait for 1 ns;  -- DUT outputs settle one delta after the edge
      expected := model(3);
      if led /= expected then
        errors := errors + 1;
      end if;
      assert led = expected
        report "edge " & integer'image(i) & ": led=" & std_logic'image(led) &
               ", expected " & std_logic'image(expected)
        severity error;
    end loop;
    report "counter_tb: " & integer'image(EDGES) & " edges checked, " &
           integer'image(errors) & " mismatches, model=0x" & to_hstring(model);
    stop;
    wait;
  end process;
end architecture;
