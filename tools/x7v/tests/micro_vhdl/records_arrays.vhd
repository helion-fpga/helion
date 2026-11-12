-- Records, arrays of vectors, constant ROM, dynamic index read and write.
library ieee; use ieee.std_logic_1164.all; use ieee.numeric_std.all;
entity top is port (clk, we : in std_logic; addr : in std_logic_vector(1 downto 0); din : in std_logic_vector(3 downto 0);
                    dout : out std_logic_vector(3 downto 0); rom : out std_logic_vector(7 downto 0); flag : out std_logic); end;
architecture rtl of top is
  type word_arr is array (0 to 3) of std_logic_vector(3 downto 0);
  type pair_t is record lo : unsigned(3 downto 0); hi : unsigned(3 downto 0); ok : boolean; end record;
  type rom_t is array (natural range <>) of pair_t;
  constant TBL : rom_t := ((x"1", x"2", true), (x"3", x"4", false), (x"5", x"6", true), (x"7", x"8", false));
  signal mem : word_arr := (others => (others => '0'));
begin
  process (clk) begin
    if rising_edge(clk) then
      if we = '1' then mem(to_integer(unsigned(addr))) <= din; end if;
    end if;
  end process;
  dout <= mem(to_integer(unsigned(addr)));
  rom <= std_logic_vector(TBL(to_integer(unsigned(addr))).hi & TBL(to_integer(unsigned(addr))).lo);
  flag <= '1' when TBL(to_integer(unsigned(addr))).ok else '0';
end;
