-- Clocked process variable read before write (a register), shift via concat.
library ieee; use ieee.std_logic_1164.all;
entity top is port (clk, d : in std_logic; q : out std_logic_vector(3 downto 0); edge : out std_logic); end;
architecture rtl of top is
  signal sr : std_logic_vector(3 downto 0) := "0000";
begin
  process (clk)
    variable prev : std_logic := '0';
  begin
    if rising_edge(clk) then
      edge <= d and not prev;
      prev := d;
      sr <= sr(2 downto 0) & d;
    end if;
  end process;
  q <= sr;
end;
