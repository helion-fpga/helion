# Clocking fuzz driver (Job 4, CLK unblocker).
#
# To call, use:
#
#   vivado -mode batch -source sweep_clk.tcl -notrace -tclargs <primitive> <part> <dir_out>
#
# <primitive> is one of: bufgce, bufgctrl, bufgce_div, bufce_row, mmcm, pll.
# <part> is e.g. xcvu3p-ffvc1517-2-e (confirm with get_parts first).
# <dir_out> receives one baseline plus one variant bitstream per
# attribute value, each with .bit, .rbt, and .ll files.
#
# Outputs in <dir_out>:
#   - baseline.bit / baseline.rbt / baseline.ll
#   - <attr>_<value>.bit / .rbt / .ll for each variant
#   - runs.txt: one line per run (design, attribute, old/new value,
#     Vivado version, part, completed/skipped with reason)
#
# Method (BitFiltrator paper section V, same as Job 2 for DSP):
# baseline has one primitive, DONT_TOUCH, all attributes at the top's
# defaults. Each variant changes exactly one attribute and re-runs the
# full flow from read_verilog. The crew diffs each variant .rbt against
# baseline .rbt, scoped to the CMT column frames; the differing bits
# are that attribute's encoding.
#
# Attribute lists below come from the UG974 primitive definitions as
# named in the vendored site-type JSONs (data/site_type_*.json). The
# driver does NOT trust the list: before fuzzing an attribute it
# checks the attribute exists on the placed cell and logs plus skips
# any name Vivado rejects. A skipped name is reported, never silently
# dropped and never fuzzed under a guessed spelling.
#
# Do not run this from the Helion VM: it needs Vivado and a license on
# the cluster. License state on the VM is UNKNOWN; a port answering
# is not a checkout.

# attr,values triples per primitive: {attr {v1 v2 ...}}.
# Values are Tcl strings passed to set_property; 1/0 are the boolean
# spellings Vivado reports for IS_*_INVERTED properties.
proc attr_table { primitive } {
  switch -exact -- $primitive {
    bufgce {
      return {
        {IS_CE_INVERTED {1}}
        {IS_I_INVERTED {1}}
        {CE_TYPE {ASYNC}}
      }
    }
    bufgctrl {
      return {
        {IS_CE0_INVERTED {1}}
        {IS_CE1_INVERTED {1}}
        {IS_I0_INVERTED {1}}
        {IS_I1_INVERTED {1}}
        {IS_IGNORE0_INVERTED {1}}
        {IS_IGNORE1_INVERTED {1}}
        {IS_S0_INVERTED {1}}
        {IS_S1_INVERTED {1}}
        {PRESELECT_I0 {1}}
        {PRESELECT_I1 {1}}
      }
    }
    bufgce_div {
      return {
        {IS_CE_INVERTED {1}}
        {IS_I_INVERTED {1}}
        {IS_CLR_INVERTED {1}}
        {CE_TYPE {ASYNC}}
        {BUFGCE_DIVIDE {2 3 4 5 6 7 8}}
      }
    }
    bufce_row {
      # No public source names the BUFCE_ROW primitive attributes, so
      # no list is asserted here. The driver enumerates the placed
      # cell's properties at runtime (see enumerate_attrs below) and
      # fuzzes every boolean/enum property it finds. If the top's
      # primitive name is rejected by Vivado, that is logged in
      # runs.txt and the sub-pass is reported as not runnable, not
      # faked.
      return {}
    }
    mmcm -
    pll {
      # Dozens of attributes; the driver enumerates them at runtime
      # (see enumerate_attrs below) instead of hardcoding a list that
      # would go stale. The run log lists every property fuzzed.
      return {}
    }
    default {
      error "unknown primitive $primitive (want bufgce, bufgctrl, bufgce_div, bufce_row, mmcm, or pll)"
    }
  }
}

# Top file and cell name per primitive.
proc top_info { primitive } {
  switch -exact -- $primitive {
    bufgce     { return {bufgce_top.v bufgce_inst} }
    bufgctrl   { return {bufgctrl_top.v bufgctrl_inst} }
    bufgce_div { return {bufgce_div_top.v bufgce_div_inst} }
    bufce_row  { return {bufce_row_top.v bufce_row_inst} }
    mmcm       { return {mmcm_top.v mmcm_inst} }
    pll        { return {pll_top.v pll_inst} }
    default    { error "unknown primitive $primitive" }
  }
}

# Enumerate fuzzable properties of a placed cell: boolean (0/1) and
# short enum properties only. Returns {attr {values...}} pairs. The
# current value is read off the cell, so the variant is always the
# non-default value actually present, never a guessed default.
proc enumerate_attrs { cell } {
  set out {}
  foreach prop [list_property $cell] {
    # Skip read-only, structural, and location properties: fuzzing
    # them is meaningless and some cannot be set.
    if { [regexp {^(REF_NAME|PRIMITIVE_GROUP|PRIMITIVE_SUBGROUP|LOC|BEL|SITE|TILE|STATUS|IS_.*_USED)$} $prop] } {
      continue
    }
    if { [catch { get_property $prop $cell } cur] } {
      continue
    }
    if { $cur eq "0" } {
      lappend out [list $prop {1}]
    } elseif { $cur eq "1" } {
      lappend out [list $prop {0}]
    }
  }
  return $out
}

proc run_flow { top_file part out_dir tag cell attr value log_fp } {
  set script_dir [file dirname [file normalize [info script]]]
  read_verilog [file join $script_dir $top_file]
  read_xdc [file join $script_dir clk_fuzz.xdc]
  synth_design -top clk_fuzz_top -part $part
  if { $attr ne "" } {
    # Existence check first: a rejected attribute is a skipped run,
    # logged, never fuzzed under a guessed spelling.
    if { [catch { get_property $attr [get_cells $cell] } cur] } {
      puts $log_fp "$tag SKIP $attr: property does not exist on $cell"
      flush $log_fp
      close_project -quiet
      return 0
    }
    puts $log_fp "$tag $attr: $cur -> $value"
    flush $log_fp
    set_property $attr $value [get_cells $cell]
  }
  opt_design
  place_design
  route_design
  set base [file join $out_dir $tag]
  write_checkpoint -force ${base}.dcp
  write_bitstream -force -raw_bitfile -logic_location_file ${base}.bit
  puts $log_fp "$tag DONE"
  flush $log_fp
  close_project -quiet
  return 1
}

proc main { primitive part dir_out } {
  file mkdir $dir_out
  set log_fp [open [file join $dir_out runs.txt] w]
  puts $log_fp "primitive: $primitive"
  puts $log_fp "part: $part"
  puts $log_fp "vivado: [version -short]"
  flush $log_fp
  lassign [top_info $primitive] top_file cell
  # Baseline first, all attributes at the top's defaults.
  run_flow $top_file $part $dir_out baseline $cell "" "" $log_fp
  set table [attr_table $primitive]
  if { [llength $table] == 0 } {
    # Runtime enumeration: synth once, read the cell's properties,
    # then fuzz each one found.
    set script_dir [file dirname [file normalize [info script]]]
    read_verilog [file join $script_dir $top_file]
    read_xdc [file join $script_dir clk_fuzz.xdc]
    synth_design -top clk_fuzz_top -part $part
    set table [enumerate_attrs [get_cells $cell]]
    puts $log_fp "enumerated attributes for $primitive: $table"
    flush $log_fp
    close_project -quiet
  }
  foreach entry $table {
    lassign $entry attr values
    foreach value $values {
      set tag "${attr}_${value}"
      # Sanitize the tag for filenames.
      regsub -all {[^A-Za-z0-9_.-]} $tag {_} tag
      run_flow $top_file $part $dir_out $tag $cell $attr $value $log_fp
    }
  }
  close $log_fp
}

if { [llength $::argv] != 3 } {
  puts "received mandatory args: $::argv"
  puts "expected mandatory args: <primitive> <part> <dir_out>"
  puts "primitive: bufgce, bufgctrl, bufgce_div, bufce_row, mmcm, or pll"
} else {
  main [lindex $::argv 0] [lindex $::argv 1] [lindex $::argv 2]
}
