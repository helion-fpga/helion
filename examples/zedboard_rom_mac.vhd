-- ROM-fed multiply-accumulate at 100 MHz: the shape of a small neural-network
-- layer (HELION_GAPS G97, G99). A 32,768 x 8 weight ROM and a 32,768 x 1 input
-- ROM (both block RAM, filled by functions at elaboration) stream into an
-- accumulator; a 64-entry register array of results is read back by index and
-- multiplied by the next weight on a DSP48. Nothing here needs a file.
--   led(7:0)  the running maximum's index and a 1.5 Hz heartbeat on led(7)
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

entity zedboard_rom_mac is
  port (
    clk : in  std_logic;
    led : out std_logic_vector(7 downto 0));
end entity;

architecture rtl of zedboard_rom_mac is
  constant N : natural := 32768;
  type w_t is array (0 to N-1) of std_logic_vector(7 downto 0);
  type x_t is array (0 to N-1) of std_logic_vector(0 downto 0);
  type r_t is array (0 to 63) of signed(15 downto 0);

  function w_init return w_t is
    variable m : w_t;
  begin
    for i in w_t'range loop
      m(i) := std_logic_vector(to_unsigned((i * 73 + 19) mod 251, 8));
    end loop;
    return m;
  end function;

  function x_init return x_t is
    variable m : x_t;
  begin
    for i in x_t'range loop
      if (i * 37 + i / 7) mod 3 = 0 then m(i) := "1"; else m(i) := "0"; end if;
    end loop;
    return m;
  end function;

  signal w_rom : w_t := w_init;
  signal x_rom : x_t := x_init;
  signal res   : r_t := (others => (others => '0'));

  signal i      : unsigned(14 downto 0) := (others => '0');
  signal w_q    : std_logic_vector(7 downto 0) := (others => '0');
  signal x_q    : std_logic_vector(0 downto 0) := (others => '0');
  signal r_q    : signed(15 downto 0) := (others => '0');
  signal last1  : std_logic := '0';
  signal last2  : std_logic := '0';
  signal n1     : unsigned(5 downto 0) := (others => '0');
  signal n2     : unsigned(5 downto 0) := (others => '0');
  signal term   : signed(31 downto 0) := (others => '0');
  signal prod   : signed(31 downto 0) := (others => '0');
  signal acc    : signed(31 downto 0) := (others => '0');
  signal sum    : signed(31 downto 0);
  signal best   : signed(31 downto 0) := (others => '0');
  signal best_k : unsigned(5 downto 0) := (others => '0');
  signal beat   : unsigned(25 downto 0) := (others => '0');
begin
  sum <= acc + term + prod;

  process (clk)
  begin
    if rising_edge(clk) then
      -- stage 0: one input per clock, 512 inputs per result
      i <= i + 1;
      beat <= beat + 1;
      -- stage 1: memory outputs
      w_q   <= w_rom(to_integer(i));
      x_q   <= x_rom(to_integer(i));
      r_q   <= res(to_integer(i(5 downto 0)));
      last1 <= '1' when i(8 downto 0) = 511 else '0';
      n1    <= i(14 downto 9);
      -- stage 2: terms
      if x_q = "1" then term <= resize(signed(w_q), 32); else term <= (others => '0'); end if;
      prod  <= resize(signed(w_q) * r_q, 32);
      last2 <= last1;
      n2    <= n1;
      -- stage 3: accumulate; store a result every 512 inputs
      if last2 = '1' then
        acc <= (others => '0');
        if sum >= 0 then res(to_integer(n2)) <= sum(17 downto 2); else res(to_integer(n2)) <= (others => '0'); end if;
        if n2 = 0 or sum > best then
          best   <= sum;
          best_k <= n2;
        end if;
      else
        acc <= sum;
      end if;
    end if;
  end process;

  led(5 downto 0) <= std_logic_vector(best_k);
  led(6)          <= best(31);
  led(7)          <= beat(25);
end architecture;
