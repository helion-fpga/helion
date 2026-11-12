library ieee; use ieee.std_logic_1164.all; use ieee.numeric_std.all;
entity top is port(clk: in std_logic; sw: in std_logic_vector(7 downto 0); led: out std_logic_vector(7 downto 0)); end;
architecture rtl of top is
  signal a, b: unsigned(11 downto 0) := (others => '0');
  signal p: unsigned(23 downto 0) := (others => '0');
begin
  process(clk) begin if rising_edge(clk) then
    a <= (a(3 downto 0) & unsigned(sw)) + 3;
    b <= (b(7 downto 0) & unsigned(sw(3 downto 0))) xor a;
    p <= a * b;
  end if; end process;
  led <= std_logic_vector(p(7 downto 0) xor p(15 downto 8) xor p(23 downto 16));
end;
