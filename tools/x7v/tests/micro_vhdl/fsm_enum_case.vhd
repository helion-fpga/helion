-- Enumerated FSM, case with choices lists and others, sync reset.
library ieee; use ieee.std_logic_1164.all;
entity top is port (clk, rst, go, stop : in std_logic; busy : out std_logic; st : out std_logic_vector(1 downto 0)); end;
architecture rtl of top is
  type state_t is (IDLE, RUN, PAUSE, DONE);
  signal s : state_t := IDLE;
begin
  process (clk) begin
    if rising_edge(clk) then
      if rst = '1' then s <= IDLE;
      else
        case s is
          when IDLE => if go = '1' then s <= RUN; end if;
          when RUN => if stop = '1' then s <= PAUSE; elsif go = '0' then s <= DONE; end if;
          when PAUSE | DONE => if go = '1' and stop = '0' then s <= IDLE; end if;
        end case;
      end if;
    end if;
  end process;
  busy <= '1' when s = RUN or s = PAUSE else '0';
  with s select st <= "00" when IDLE, "01" when RUN, "10" when PAUSE, "11" when others;
end;
