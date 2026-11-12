-- Package function, generics, generate, component instantiation by position.
library ieee; use ieee.std_logic_1164.all; use ieee.numeric_std.all;
package util is
  function clog2(n : natural) return natural;
  function parity(v : std_logic_vector) return std_logic;
end package;
package body util is
  function clog2(n : natural) return natural is
    variable r : natural := 0; variable p : natural := 1;
  begin
    while p < n loop r := r + 1; p := p * 2; end loop;
    return r;
  end function;
  function parity(v : std_logic_vector) return std_logic is
    variable x : std_logic := '0';
  begin
    for i in v'range loop x := x xor v(i); end loop;
    return x;
  end function;
end package body;
library ieee; use ieee.std_logic_1164.all; use ieee.numeric_std.all; use work.util.all;
entity cnt is generic (N : natural := 10); port (clk : in std_logic; tick : out std_logic; v : out std_logic_vector(clog2(N)-1 downto 0)); end;
architecture rtl of cnt is
  signal c : unsigned(clog2(N)-1 downto 0) := (others => '0');
begin
  process (clk) begin
    if rising_edge(clk) then
      if c = N - 1 then c <= (others => '0'); else c <= c + 1; end if;
    end if;
  end process;
  tick <= '1' when c = N - 1 else '0';
  v <= std_logic_vector(c);
end;
library ieee; use ieee.std_logic_1164.all; use work.util.all;
entity top is port (clk : in std_logic; d : in std_logic_vector(5 downto 0); t3, t5 : out std_logic; v5 : out std_logic_vector(2 downto 0); par : out std_logic; inv : out std_logic_vector(5 downto 0)); end;
architecture rtl of top is
  component cnt is generic (N : natural := 10); port (clk : in std_logic; tick : out std_logic; v : out std_logic_vector(clog2(N)-1 downto 0)); end component;
  signal dummy : std_logic_vector(1 downto 0);
begin
  u3 : cnt generic map (3) port map (clk, t3, dummy);
  u5 : entity work.cnt generic map (N => 5) port map (clk => clk, tick => t5, v => v5);
  par <= parity(d);
  g : for i in 0 to 5 generate
    inv(i) <= not d(5 - i);
  end generate;
end;
