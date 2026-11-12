library ieee; use ieee.std_logic_1164.all; use ieee.numeric_std.all;
entity top is port(clk: in std_logic; sw: in std_logic_vector(7 downto 0); led: out std_logic_vector(7 downto 0)); end;
architecture rtl of top is
  signal a, b, s, d: unsigned(15 downto 0) := (others => '0');
  signal lt: std_logic := '0';
begin
  process(clk) begin if rising_edge(clk) then
    a <= a(7 downto 0) & unsigned(sw);
    b <= b(11 downto 0) & unsigned(sw(3 downto 0));
    s <= a + b;
    d <= a - b;
    if a < b then lt <= '1'; else lt <= '0'; end if;
  end if; end process;
  led <= std_logic_vector(s(7 downto 0) xor s(15 downto 8) xor d(7 downto 0) xor d(15 downto 8)) when lt = '0' else std_logic_vector(d(15 downto 8));
end;
