# Helion Tcl Subset — honestly supported

The IDE console (`helion-ide` → Tcl Console tab) runs a **real interpreter**
(`crates/helion-gui/src/tcl/`), not pattern matching: `parse.rs` implements
Tcl word rules (`{}`, `""`, `[...]`, `$var`, backslash escapes, `#` comments),
`interp.rs` evaluates with real variables, scopes, procedures, loops, `expr`,
lists and `catch`, and `design.rs` implements the object model against the
live design database.

Everything is a string, like Tcl. Errors are `ERROR: <msg> (line <n>)`.

## Core language

| Command | Notes |
|---|---|
| `set var ?value?` | read/write; `set a [expr {...}]` works |
| `unset var...` | |
| `puts ?-nonewline? string` | output goes to the console pane |
| `expr arg...` | arithmetic, comparisons, `&&`/`||`/`!`, `?:`, math funcs (`abs int double round sqrt pow sin cos atan2 log exp`), string compare ops (`eq ne`), `[cmd]` substitution inside expressions |
| `if cond body ?elseif ...? ?else?` | |
| `foreach var list body` / `foreach {a b} list body` | |
| `for init test next body` | |
| `while test body` | `break` / `continue` supported |
| `proc name params body` | params may have `{name default}` defaults |
| `return ?value?` | |
| `list`, `llength`, `lindex`, `lrange`, `lappend`, `concat`, `join`, `split` | Tcl list semantics incl. `end-N` indices |
| `string length|range|index|match|tolower|toupper|trim|first|equal` | glob `match`, not regex |
| `info exists|vars|procs|commands|args|body` | |
| `catch script ?var?` | returns 0/1, Tcl-style |
| `error msg` | |
| `eval arg...`, `subst`, `source file` | |
| `format fmt args...` | `%s %d %f %x` subset |
| `incr var ?n?`, `append var args...` | |
| `help ?topic?` | overview, or usage for one command (also works for your `proc`s) |
| `clear` | clear the console transcript |

Unknown commands raise a Tcl-style error with "did you mean …?" suggestions
instead of failing silently. Errors are single-line, never panics.

The console input is multi-line: Enter runs the command when braces /
brackets / quotes are balanced, otherwise it inserts a newline (Shift+Enter
always inserts a newline). Up/Down walk command history on single-line input.

## Object model (Vivado-style, backed by the real design DB)

`get_cells`, `get_nets`, `get_pins`, `get_ports`, `get_clocks`,
`get_timing_paths` — glob patterns, `-filter {PROP == val}` / `!=`, `>`, `<`,
`=~` (glob), `-of_objects`, `-quiet`, `-max_paths` (timing paths).

`get_property`, `set_property`, `report_property`, `list_property`
(`list_properties` is an alias), `select_objects`, `highlight_objects`,
`get_selected_objects`, `current_design`.

`report_timing` prints a WNS/TNS summary plus the worst-paths table;
`report_utilization` prints a per-resource usage table against the die.
Select a console row to read the full report in the properties pane.

## Flow commands (same engines as the GUI buttons)

`synth_design` (`read_sv`/`read_verilog`/`read_vhdl`, `hds::synth`),
`opt_design`, `place_design`, `route_design`, `write_bitstream`,
`write_checkpoint`, `create_clock`, `report_timing`, `report_utilization`,
`mark_debug`, `eco`, `open_hw_manager`, `program_hw`, `report_die`,
`report_featuremap`, `write_hnf`, `hds::impl`, `hds::flow`, `hds::tree`.

## Deliberately NOT supported

`namespace`, `upvar`/`uplevel`, `trace`, `regexp`/`regsub`, `dict`,
coroutines, `after`, `package require`, renaming builtins. Unknown commands
raise Tcl-style errors instead of failing silently.
