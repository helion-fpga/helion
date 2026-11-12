-- std_logic_unsigned arithmetic on std_logic_vector, conv_integer, when/else chain.
library ieee; use ieee.std_logic_1164.all; use ieee.std_logic_unsigned.all;
entity top is port (clk : in std_logic; a, b : in std_logic_vector(3 downto 0); sum : out std_logic_vector(3 downto 0); big : out std_logic; sel : out std_logic_vector(1 downto 0); acc : out std_logic_vector(7 downto 0)); end;
architecture rtl of top is
  signal r : std_logic_vector(7 downto 0) := x"00";
begin
  sum <= a + b;
  big <= '1' when a > b else '0';
  sel <= "11" when conv_integer(a) > 12 else "10" when a = "0101" else "01" when b(0) = '1' else "00";
  process (clk) begin
    if clk'event and clk = '1' then r <= r + a; end if;
  end process;
  acc <= r;
end;
