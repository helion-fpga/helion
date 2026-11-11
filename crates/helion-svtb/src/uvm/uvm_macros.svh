// Helion UVM macros (subset). See uvm_pkg.sv for what is implemented.
`ifndef HELION_UVM_MACROS_SVH
`define HELION_UVM_MACROS_SVH

`define UVM_VERSION_HELION 1

// ---------------------------------------------------------------------------
// Factory registration
// ---------------------------------------------------------------------------
`define uvm_object_registry(T,S) \
  typedef uvm_object_registry #(T, S) type_id; \
  static function type_id get_type(); return type_id::get(); endfunction \
  virtual function uvm_object_wrapper get_object_type(); return type_id::get(); endfunction

`define uvm_component_registry(T,S) \
  typedef uvm_component_registry #(T, S) type_id; \
  static function type_id get_type(); return type_id::get(); endfunction \
  virtual function uvm_object_wrapper get_object_type(); return type_id::get(); endfunction

`define uvm_object_utils(T) \
  `uvm_object_registry(T, `"T`") \
  virtual function string get_type_name(); return `"T`"; endfunction \
  virtual function uvm_object create(string name=""); T tmp__; tmp__ = new(name); return tmp__; endfunction

`define uvm_object_param_utils(T) \
  typedef uvm_object_registry #(T) type_id; \
  static function type_id get_type(); return type_id::get(); endfunction \
  virtual function uvm_object_wrapper get_object_type(); return type_id::get(); endfunction \
  virtual function uvm_object create(string name=""); T tmp__; tmp__ = new(name); return tmp__; endfunction

`define uvm_component_utils(T) \
  `uvm_component_registry(T, `"T`") \
  virtual function string get_type_name(); return `"T`"; endfunction

`define uvm_component_param_utils(T) \
  typedef uvm_component_registry #(T) type_id; \
  static function type_id get_type(); return type_id::get(); endfunction \
  virtual function uvm_object_wrapper get_object_type(); return type_id::get(); endfunction

// Field automation: copy / compare / print of the listed fields.
`define uvm_field_utils_begin(T) \
  virtual function void m_uvm_field_automation(uvm_object rhs_obj__, int what__, uvm_printer ctx__); \
    T rhs__; \
    super.m_uvm_field_automation(rhs_obj__, what__, ctx__); \
    if (rhs_obj__ != null) void'($cast(rhs__, rhs_obj__));

`define uvm_field_utils_end endfunction

`define uvm_object_utils_begin(T) `uvm_object_utils(T) `uvm_field_utils_begin(T)
`define uvm_object_param_utils_begin(T) `uvm_object_param_utils(T) `uvm_field_utils_begin(T)
`define uvm_object_utils_end `uvm_field_utils_end
`define uvm_component_utils_begin(T) `uvm_component_utils(T) `uvm_field_utils_begin(T)
`define uvm_component_param_utils_begin(T) `uvm_component_param_utils(T) `uvm_field_utils_begin(T)
`define uvm_component_utils_end `uvm_field_utils_end

`define m_uvm_field_body(ARG, FLAG, FMT, PRINTV, NE) \
  case (what__) \
    UVM_COPY: if (!((FLAG) & UVM_NOCOPY)) ARG = rhs__.ARG; \
    UVM_COMPARE: if (!((FLAG) & UVM_NOCOMPARE) && (NE)) ctx__.m_mismatch(`"ARG`", $sformatf(FMT, PRINTV(ARG)), $sformatf(FMT, PRINTV(rhs__.ARG))); \
    UVM_PRINT: if (!((FLAG) & UVM_NOPRINT)) ctx__.m_add(`"ARG`", $sformatf(FMT, PRINTV(ARG))); \
  endcase

`define uvm_field_int(ARG, FLAG) `m_uvm_field_body(ARG, FLAG, (((FLAG) & UVM_DEC) ? "%0d" : (((FLAG) & UVM_BIN) ? "'b%0b" : "'h%0h")), , ARG != rhs__.ARG)
`define uvm_field_string(ARG, FLAG) `m_uvm_field_body(ARG, FLAG, "%s", , ARG != rhs__.ARG)
`define uvm_field_real(ARG, FLAG) `m_uvm_field_body(ARG, FLAG, "%f", , ARG != rhs__.ARG)
`define uvm_field_enum(T, ARG, FLAG) \
  case (what__) \
    UVM_COPY: if (!((FLAG) & UVM_NOCOPY)) ARG = rhs__.ARG; \
    UVM_COMPARE: if (!((FLAG) & UVM_NOCOMPARE) && ARG != rhs__.ARG) ctx__.m_mismatch(`"ARG`", ARG.name(), rhs__.ARG.name()); \
    UVM_PRINT: if (!((FLAG) & UVM_NOPRINT)) ctx__.m_add(`"ARG`", ARG.name()); \
  endcase
`define uvm_field_queue_int(ARG, FLAG) `m_uvm_field_body(ARG, FLAG, "%p", , ARG != rhs__.ARG)
`define uvm_field_array_int(ARG, FLAG) `m_uvm_field_body(ARG, FLAG, "%p", , ARG != rhs__.ARG)
`define uvm_field_sarray_int(ARG, FLAG) `m_uvm_field_body(ARG, FLAG, "%p", , ARG != rhs__.ARG)
`define uvm_field_object(ARG, FLAG) \
  case (what__) \
    UVM_COPY: if (!((FLAG) & UVM_NOCOPY)) ARG = rhs__.ARG; \
    UVM_COMPARE: if (!((FLAG) & UVM_NOCOMPARE)) begin \
      if ((ARG == null) != (rhs__.ARG == null)) ctx__.m_mismatch(`"ARG`", "handle", "null"); \
      else if (ARG != null && !ARG.compare(rhs__.ARG)) ctx__.m_mismatch(`"ARG`", "object", "differs"); \
    end \
    UVM_PRINT: if (!((FLAG) & UVM_NOPRINT)) ctx__.m_add(`"ARG`", (ARG == null) ? "null" : ARG.get_name()); \
  endcase

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------
`define uvm_info(ID, MSG, VERBOSITY) \
  begin \
    if (uvm_report_enabled(VERBOSITY, UVM_INFO, ID)) \
      uvm_report_info(ID, MSG, VERBOSITY, `__FILE__, `__LINE__); \
  end

`define uvm_warning(ID, MSG) \
  begin \
    if (uvm_report_enabled(UVM_NONE, UVM_WARNING, ID)) \
      uvm_report_warning(ID, MSG, UVM_NONE, `__FILE__, `__LINE__); \
  end

`define uvm_error(ID, MSG) \
  begin \
    if (uvm_report_enabled(UVM_NONE, UVM_ERROR, ID)) \
      uvm_report_error(ID, MSG, UVM_NONE, `__FILE__, `__LINE__); \
  end

`define uvm_fatal(ID, MSG) \
  begin \
    if (uvm_report_enabled(UVM_NONE, UVM_FATAL, ID)) \
      uvm_report_fatal(ID, MSG, UVM_NONE, `__FILE__, `__LINE__); \
  end

// ---------------------------------------------------------------------------
// Sequences
// ---------------------------------------------------------------------------
`define uvm_create(SEQ_OR_ITEM) \
  begin \
    uvm_object_wrapper w__; \
    w__ = SEQ_OR_ITEM.get_type(); \
    void'($cast(SEQ_OR_ITEM, create_item(w__, m_sequencer, `"SEQ_OR_ITEM`"))); \
  end

`define uvm_do_with(SEQ_OR_ITEM, CONSTRAINTS) \
  begin \
    uvm_sequence_base seq__; \
    `uvm_create(SEQ_OR_ITEM) \
    if ($cast(seq__, SEQ_OR_ITEM)) begin \
      if (!SEQ_OR_ITEM.randomize() with CONSTRAINTS) \
        `uvm_warning("RNDFLD", "Randomization failed in uvm_do_with action") \
      seq__.start(m_sequencer, this); \
    end else begin \
      start_item(SEQ_OR_ITEM); \
      if (!SEQ_OR_ITEM.randomize() with CONSTRAINTS) \
        `uvm_warning("RNDFLD", "Randomization failed in uvm_do_with action") \
      finish_item(SEQ_OR_ITEM); \
    end \
  end

`define uvm_do(SEQ_OR_ITEM) `uvm_do_with(SEQ_OR_ITEM, {})

`define uvm_send(SEQ_OR_ITEM) \
  begin \
    start_item(SEQ_OR_ITEM); \
    finish_item(SEQ_OR_ITEM); \
  end

`define uvm_rand_send_with(SEQ_OR_ITEM, CONSTRAINTS) \
  begin \
    start_item(SEQ_OR_ITEM); \
    if (!SEQ_OR_ITEM.randomize() with CONSTRAINTS) \
      `uvm_warning("RNDFLD", "Randomization failed in uvm_rand_send_with action") \
    finish_item(SEQ_OR_ITEM); \
  end

`define uvm_rand_send(SEQ_OR_ITEM) `uvm_rand_send_with(SEQ_OR_ITEM, {})

`define uvm_declare_p_sequencer(SEQUENCER) \
  SEQUENCER p_sequencer; \
  virtual function void m_set_p_sequencer(); \
    super.m_set_p_sequencer(); \
    if (!$cast(p_sequencer, m_sequencer)) \
      uvm_report_fatal("DCLPSQ", {get_full_name(), " p_sequencer is not a ", `"SEQUENCER`"}, UVM_NONE, `__FILE__, `__LINE__); \
  endfunction

// ---------------------------------------------------------------------------
// TLM
// ---------------------------------------------------------------------------
`define uvm_analysis_imp_decl(SFX) \
  class uvm_analysis_imp``SFX #(type T=int, type IMP=int) extends uvm_port_base #(T); \
    IMP m_imp; \
    function new(string name, IMP imp); \
      super.new(name, imp); \
      m_imp = imp; \
    endfunction \
    virtual function void write(T t); \
      m_imp.write``SFX(t); \
    endfunction \
  endclass

`endif
