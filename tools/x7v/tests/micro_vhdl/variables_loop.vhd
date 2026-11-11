-- Combinational process with variables, for loop, popcount and priority encoder.
library ieee; use ieee.std_logic_1164.all; use ieee.numeric_std.all;
entity top is port (a : in std_logic_vector(7 downto 0); cnt : out std_logic_vector(3 downto 0); first : out std_logic_vector(2 downto 0); any : out std_logic); end;
architecture rtl of top is begin
  process (a)
    variable c : unsigned(3 downto 0);
    variable f : integer range 0 to 7;
  begin
    c := (others => '0');
    f := 0;
    for i in a'range loop
      if a(i) = '1' then c := c + 1; end if;
    end loop;
    for i in 0 to 7 loop
      if a(i) = '1' then f := i; exit; end if;
    end loop;
    cnt <= std_logic_vector(c);
    first <= std_logic_vector(to_unsigned(f, 3));
  end process;
  any <= or a;
end;
