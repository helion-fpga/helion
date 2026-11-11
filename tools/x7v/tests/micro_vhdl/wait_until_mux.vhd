-- wait-until process, aggregate with named choices, case on integer ranges.
library ieee; use ieee.std_logic_1164.all; use ieee.numeric_std.all;
entity top is port (clk : in std_logic; x : in std_logic_vector(3 downto 0); y : out std_logic_vector(7 downto 0); k : out std_logic_vector(1 downto 0)); end;
architecture rtl of top is
  signal r : std_logic_vector(7 downto 0) := (others => '0');
begin
  process begin
    wait until rising_edge(clk);
    r <= (7 => x(3), 0 => x(0), 4 downto 2 => x(1), others => '0');
  end process;
  y <= r;
  process (x)
    variable n : integer range 0 to 15;
  begin
    n := to_integer(unsigned(x));
    case n is
      when 0 to 3 => k <= "00";
      when 4 | 5 | 6 => k <= "01";
      when 7 to 12 => k <= "10";
      when others => k <= "11";
    end case;
  end process;
end;
