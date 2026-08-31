-- Helion VHDL blinky (original subset, not UNISIM)
entity blinky is
  port (
    clk : in  std_logic;
    led : out std_logic
  );
end entity;

architecture rtl of blinky is
  signal q : std_logic := '0';
begin
  process (clk)
  begin
    if rising_edge(clk) then
      q <= not q;
    end if;
  end process;
  led <= q;
end architecture;
