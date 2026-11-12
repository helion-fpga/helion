library ieee; use ieee.std_logic_1164.all; use ieee.numeric_std.all;
entity top is port(clk: in std_logic; sw: in std_logic_vector(7 downto 0); led: out std_logic_vector(7 downto 0)); end;
architecture rtl of top is
  signal s, n: std_logic_vector(127 downto 0) := (others => '0');
begin
  g: for i in 0 to 127 generate
    n(i) <= s((i*7+3) mod 128) xor (s((i*13+5) mod 128) and s((i*29+11) mod 128)) xor s((i*37+1) mod 128) xor (s((i*5+17) mod 128) or sw(i mod 8)) xor s((i*53+9) mod 128);
  end generate;
  process(clk) begin if rising_edge(clk) then s <= n; end if; end process;
  led <= s(7 downto 0) xor s(15 downto 8) xor s(23 downto 16) xor s(31 downto 24) xor s(71 downto 64) xor s(127 downto 120);
end;
