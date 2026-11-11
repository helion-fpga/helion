-- Async active-low reset, counter with enable, integer range signal.
library ieee; use ieee.std_logic_1164.all; use ieee.numeric_std.all;
entity top is port (clk, rstn, en : in std_logic; q : out std_logic_vector(3 downto 0); wrap : out std_logic); end;
architecture rtl of top is
  signal cnt : integer range 0 to 11 := 0;
begin
  process (clk, rstn) begin
    if rstn = '0' then cnt <= 3;
    elsif rising_edge(clk) then
      if en = '1' then
        if cnt = 11 then cnt <= 0; else cnt <= cnt + 1; end if;
      end if;
    end if;
  end process;
  q <= std_logic_vector(to_unsigned(cnt, 4));
  wrap <= '1' when cnt = 11 else '0';
end;
