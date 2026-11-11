-- numeric_std signed arithmetic, resize, comparisons, abs, shifts.
library ieee; use ieee.std_logic_1164.all; use ieee.numeric_std.all;
entity top is port (a, b : in std_logic_vector(3 downto 0); s : out std_logic_vector(5 downto 0); p : out std_logic_vector(7 downto 0);
                    lt, ge : out std_logic; ab : out std_logic_vector(3 downto 0); sh : out std_logic_vector(3 downto 0)); end;
architecture rtl of top is
  signal sa, sb : signed(3 downto 0);
begin
  sa <= signed(a); sb <= signed(b);
  s <= std_logic_vector(resize(sa, 6) - resize(sb, 6));
  p <= std_logic_vector(sa * sb);
  lt <= '1' when sa < sb else '0';
  ge <= '1' when sa >= -3 else '0';
  ab <= std_logic_vector(abs(sa));
  sh <= std_logic_vector(shift_right(sa, 2));
end;
