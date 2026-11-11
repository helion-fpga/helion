// Helion UVM — a documented SUBSET of IEEE 1800.2 UVM, written in
// SystemVerilog on top of the Helion testbench engine.
//
// Implemented: uvm_object / uvm_component hierarchy, factory
// (type_id::create, type/instance overrides, create by name), phases
// (build, connect, end_of_elaboration, start_of_simulation, run with
// objections + drain time, extract, check, report, final), uvm_config_db,
// TLM analysis ports/exports/imps (+ `uvm_analysis_imp_decl), put/get
// ports, uvm_tlm_fifo / uvm_tlm_analysis_fifo, uvm_sequence_item /
// uvm_sequence / uvm_sequencer / uvm_driver (get_next_item / item_done /
// get / responses), uvm_monitor, uvm_scoreboard, uvm_agent, uvm_env,
// uvm_test, uvm_subscriber, reporting macros with verbosity
// (+UVM_VERBOSITY) and the end-of-test report summary, run_test()
// with +UVM_TESTNAME, field macros for copy/compare/print.
//
// Not implemented: the register layer (uvm_reg*), sequence libraries,
// virtual-sequence arbitration modes / priorities / grab / lock, phase
// jumping and custom phases/domains, report catchers and actions files,
// uvm_resource_db (use uvm_config_db), uvm_event pools, recording,
// packing (pack/unpack), the command-line processor beyond +UVM_TESTNAME /
// +UVM_VERBOSITY / +UVM_TIMEOUT.
`ifndef HELION_UVM_PKG_SV
`define HELION_UVM_PKG_SV
`include "uvm_macros.svh"

package uvm_pkg;

  typedef enum {UVM_INFO, UVM_WARNING, UVM_ERROR, UVM_FATAL} uvm_severity;
  typedef enum {UVM_NONE = 0, UVM_LOW = 100, UVM_MEDIUM = 200, UVM_HIGH = 300, UVM_FULL = 400, UVM_DEBUG = 500} uvm_verbosity;
  typedef enum {UVM_PASSIVE = 0, UVM_ACTIVE = 1} uvm_active_passive_enum;
  typedef enum {UVM_COPY, UVM_COMPARE, UVM_PRINT} uvm_field_op_e;

  parameter int UVM_ALL_ON    = 'h0;
  parameter int UVM_DEFAULT   = 'h0;
  parameter int UVM_HEX       = 'h0;
  parameter int UVM_DEC       = 'h1;
  parameter int UVM_BIN       = 'h2;
  parameter int UVM_UNSIGNED  = 'h0;
  parameter int UVM_NOCOPY    = 'h10000;
  parameter int UVM_NOCOMPARE = 'h20000;
  parameter int UVM_NOPRINT   = 'h40000;
  parameter int UVM_REFERENCE = 'h0;

  typedef class uvm_object;
  typedef class uvm_component;
  typedef class uvm_root;
  typedef class uvm_sequence_base;
  typedef class uvm_sequencer_base;
  typedef class uvm_printer;

  // -------------------------------------------------------------------------
  // Report server
  // -------------------------------------------------------------------------
  class uvm_report_server;
    static int m_counts[4];
    static int m_id_counts[string];
    static int m_verbosity = UVM_MEDIUM;

    static function string sev_name(int sev);
      case (sev)
        0: return "UVM_INFO";
        1: return "UVM_WARNING";
        2: return "UVM_ERROR";
        default: return "UVM_FATAL";
      endcase
    endfunction

    static function void report(int sev, string id, string msg, int verbosity, string filename, int line, string path);
      string loc;
      string s;
      m_counts[sev]++;
      if (m_id_counts.exists(id)) m_id_counts[id]++;
      else m_id_counts[id] = 1;
      loc = (filename == "") ? "" : $sformatf("%s(%0d) ", filename, line);
      s = $sformatf("%s %s@ %0t: %s [%s] %s", sev_name(sev), loc, $time, path, id, msg);
      $helion_uvm_report(sev, filename, line, s);
      if (sev == 3) begin
        summarize();
        $finish;
      end
    endfunction

    static function int get_severity_count(int sev);
      return m_counts[sev];
    endfunction

    static function void summarize();
      string ids;
      $display("");
      $display("--- UVM Report Summary ---");
      $display("");
      $display("** Report counts by severity");
      $display("UVM_INFO : %0d", m_counts[0]);
      $display("UVM_WARNING : %0d", m_counts[1]);
      $display("UVM_ERROR : %0d", m_counts[2]);
      $display("UVM_FATAL : %0d", m_counts[3]);
      $display("** Report counts by id");
      if (m_id_counts.first(ids)) begin
        do $display("[%s] %0d", ids, m_id_counts[ids]);
        while (m_id_counts.next(ids));
      end
    endfunction
  endclass

  function bit uvm_report_enabled(int verbosity, int severity = UVM_INFO, string id = "");
    return severity != UVM_INFO || verbosity <= uvm_report_server::m_verbosity;
  endfunction
  function void uvm_report_info(string id, string message, int verbosity = UVM_MEDIUM, string filename = "", int line = 0);
    uvm_report_server::report(0, id, message, verbosity, filename, line, "reporter");
  endfunction
  function void uvm_report_warning(string id, string message, int verbosity = UVM_NONE, string filename = "", int line = 0);
    uvm_report_server::report(1, id, message, verbosity, filename, line, "reporter");
  endfunction
  function void uvm_report_error(string id, string message, int verbosity = UVM_NONE, string filename = "", int line = 0);
    uvm_report_server::report(2, id, message, verbosity, filename, line, "reporter");
  endfunction
  function void uvm_report_fatal(string id, string message, int verbosity = UVM_NONE, string filename = "", int line = 0);
    uvm_report_server::report(3, id, message, verbosity, filename, line, "reporter");
  endfunction

  // -------------------------------------------------------------------------
  // Printer / comparer used by field automation
  // -------------------------------------------------------------------------
  class uvm_printer;
    string m_lines[$];
    int m_mismatches;
    function void m_begin(string name, string tname);
      m_lines.delete();
      m_lines.push_back($sformatf("%s (%s)", name, tname));
    endfunction
    function void m_add(string field, string value);
      m_lines.push_back($sformatf("  %s: %s", field, value));
    endfunction
    function void m_mismatch(string field, string lhs, string rhs);
      m_mismatches++;
      m_lines.push_back($sformatf("Miscompare for %s: lhs = %s : rhs = %s", field, lhs, rhs));
    endfunction
    function void print_field(string name, longint value, int size = 64, int radix = 0);
      m_add(name, $sformatf("'h%0h", value));
    endfunction
    function void print_field_int(string name, longint value, int size = 64, int radix = 0);
      m_add(name, $sformatf("'h%0h", value));
    endfunction
    function void print_string(string name, string value);
      m_add(name, value);
    endfunction
    function string m_end();
      string s;
      foreach (m_lines[i]) s = (i == 0) ? m_lines[i] : {s, "\n", m_lines[i]};
      return s;
    endfunction
  endclass

  class uvm_comparer extends uvm_printer;
    function bit compare_field(string name, longint lhs, longint rhs, int size = 64);
      if (lhs != rhs) m_mismatch(name, $sformatf("'h%0h", lhs), $sformatf("'h%0h", rhs));
      return lhs == rhs;
    endfunction
    function bit compare_field_int(string name, longint lhs, longint rhs, int size = 64);
      return compare_field(name, lhs, rhs, size);
    endfunction
    function bit compare_string(string name, string lhs, string rhs);
      if (lhs != rhs) m_mismatch(name, lhs, rhs);
      return lhs == rhs;
    endfunction
  endclass

  typedef class uvm_object_wrapper;

  // -------------------------------------------------------------------------
  // uvm_object
  // -------------------------------------------------------------------------
  virtual class uvm_object;
    protected string m_name;
    static int m_inst_count;
    int m_inst_id;

    function new(string name = "");
      m_name = name;
      m_inst_id = m_inst_count;
      m_inst_count++;
    endfunction

    virtual function string get_name();
      return m_name;
    endfunction
    virtual function void set_name(string name);
      m_name = name;
    endfunction
    virtual function string get_full_name();
      return m_name;
    endfunction
    virtual function string get_type_name();
      return "uvm_object";
    endfunction
    function int get_inst_id();
      return m_inst_id;
    endfunction
    virtual function uvm_object_wrapper get_object_type();
      return null;
    endfunction
    virtual function uvm_object create(string name = "");
      return null;
    endfunction

    virtual function void m_uvm_field_automation(uvm_object rhs_obj__, int what__, uvm_printer ctx__);
    endfunction
    virtual function void do_copy(uvm_object rhs);
    endfunction
    virtual function bit do_compare(uvm_object rhs, uvm_comparer comparer);
      return 1;
    endfunction
    virtual function void do_print(uvm_printer printer);
    endfunction
    virtual function string convert2string();
      return "";
    endfunction

    function void copy(uvm_object rhs);
      if (rhs == null) begin
        uvm_report_error("NULLCP", "copy() from a null object");
        return;
      end
      m_uvm_field_automation(rhs, UVM_COPY, null);
      do_copy(rhs);
    endfunction

    function bit compare(uvm_object rhs, uvm_comparer comparer = null);
      uvm_comparer c;
      c = comparer;
      if (c == null) c = new;
      if (rhs == null) return 0;
      m_uvm_field_automation(rhs, UVM_COMPARE, c);
      if (!do_compare(rhs, c)) return 0;
      return c.m_mismatches == 0;
    endfunction

    virtual function uvm_object clone();
      uvm_object o;
      o = create(get_name());
      if (o == null) begin
        uvm_object_wrapper w;
        w = get_object_type();
        if (w != null) o = w.create_object(get_name());
      end
      if (o != null) o.copy(this);
      return o;
    endfunction

    function string sprint(uvm_printer printer = null);
      uvm_printer p;
      p = printer;
      if (p == null) p = new;
      p.m_begin(get_name(), get_type_name());
      m_uvm_field_automation(null, UVM_PRINT, p);
      do_print(p);
      return p.m_end();
    endfunction

    function void print(uvm_printer printer = null);
      $display("%s", sprint(printer));
    endfunction

    // Reporting (available on every object in this subset).
    virtual function string m_report_path();
      return get_full_name();
    endfunction
    function bit uvm_report_enabled(int verbosity, int severity = UVM_INFO, string id = "");
      return severity != UVM_INFO || verbosity <= uvm_report_server::m_verbosity;
    endfunction
    function void uvm_report_info(string id, string message, int verbosity = UVM_MEDIUM, string filename = "", int line = 0);
      uvm_report_server::report(0, id, message, verbosity, filename, line, m_report_path());
    endfunction
    function void uvm_report_warning(string id, string message, int verbosity = UVM_NONE, string filename = "", int line = 0);
      uvm_report_server::report(1, id, message, verbosity, filename, line, m_report_path());
    endfunction
    function void uvm_report_error(string id, string message, int verbosity = UVM_NONE, string filename = "", int line = 0);
      uvm_report_server::report(2, id, message, verbosity, filename, line, m_report_path());
    endfunction
    function void uvm_report_fatal(string id, string message, int verbosity = UVM_NONE, string filename = "", int line = 0);
      uvm_report_server::report(3, id, message, verbosity, filename, line, m_report_path());
    endfunction
  endclass

  // -------------------------------------------------------------------------
  // Factory
  // -------------------------------------------------------------------------
  virtual class uvm_object_wrapper;
    virtual function uvm_object create_object(string name = "");
      return null;
    endfunction
    virtual function uvm_component create_component(string name, uvm_component parent);
      return null;
    endfunction
    pure virtual function string get_type_name();
  endclass

  class uvm_factory;
    static uvm_factory m_inst;
    uvm_object_wrapper m_types[string];
    uvm_object_wrapper m_overrides[string];
    string m_inst_paths[$];
    uvm_object_wrapper m_inst_orig[$];
    uvm_object_wrapper m_inst_new[$];

    static function uvm_factory get();
      if (m_inst == null) m_inst = new;
      return m_inst;
    endfunction

    function void register(uvm_object_wrapper obj);
      m_types[obj.get_type_name()] = obj;
    endfunction

    function bit is_type_registered(string name);
      return m_types.exists(name);
    endfunction

    function void set_type_override_by_type(uvm_object_wrapper original_type, uvm_object_wrapper override_type, bit replace = 1);
      if (!replace && m_overrides.exists(original_type.get_type_name())) return;
      m_overrides[original_type.get_type_name()] = override_type;
    endfunction

    function void set_type_override_by_name(string original_type_name, string override_type_name, bit replace = 1);
      if (!m_types.exists(override_type_name)) begin
        uvm_report_error("TYPNTF", $sformatf("set_type_override_by_name: type '%s' is not registered with the factory", override_type_name));
        return;
      end
      if (!replace && m_overrides.exists(original_type_name)) return;
      m_overrides[original_type_name] = m_types[override_type_name];
    endfunction

    function void set_inst_override_by_type(uvm_object_wrapper original_type, uvm_object_wrapper override_type, string full_inst_path);
      m_inst_paths.push_back(full_inst_path);
      m_inst_orig.push_back(original_type);
      m_inst_new.push_back(override_type);
    endfunction

    function void set_inst_override_by_name(string original_type_name, string override_type_name, string full_inst_path);
      if (!m_types.exists(original_type_name) || !m_types.exists(override_type_name)) begin
        uvm_report_error("TYPNTF", "set_inst_override_by_name: both types must be registered with the factory");
        return;
      end
      set_inst_override_by_type(m_types[original_type_name], m_types[override_type_name], full_inst_path);
    endfunction

    function uvm_object_wrapper find_override_by_type(uvm_object_wrapper requested_type, string full_inst_path);
      uvm_object_wrapper t;
      int guard;
      t = requested_type;
      for (int i = m_inst_paths.size() - 1; i >= 0; i--)
        if (m_inst_orig[i] == t && $helion_glob(m_inst_paths[i], full_inst_path)) return m_inst_new[i];
      while (m_overrides.exists(t.get_type_name()) && guard < 32) begin
        if (m_overrides[t.get_type_name()] == t) break;
        t = m_overrides[t.get_type_name()];
        guard++;
      end
      return t;
    endfunction

    function uvm_object create_object_by_type(uvm_object_wrapper requested_type, string parent_inst_path = "", string name = "");
      string path;
      path = (parent_inst_path == "") ? name : {parent_inst_path, ".", name};
      return find_override_by_type(requested_type, path).create_object(name);
    endfunction

    function uvm_component create_component_by_type(uvm_object_wrapper requested_type, string parent_inst_path = "", string name = "", uvm_component parent = null);
      string path;
      path = (parent_inst_path == "") ? name : {parent_inst_path, ".", name};
      return find_override_by_type(requested_type, path).create_component(name, parent);
    endfunction

    function uvm_component create_component_by_name(string requested_type_name, string parent_inst_path = "", string name = "", uvm_component parent = null);
      if (!m_types.exists(requested_type_name)) begin
        uvm_report_server::report(3, "INVTST", $sformatf("Requested test from command line +UVM_TESTNAME=%s not found.", requested_type_name), UVM_NONE, "", 0, "reporter");
        return null;
      end
      return create_component_by_type(m_types[requested_type_name], parent_inst_path, name, parent);
    endfunction

    function uvm_object create_object_by_name(string requested_type_name, string parent_inst_path = "", string name = "");
      if (!m_types.exists(requested_type_name)) begin
        uvm_report_error("TYPNTF", $sformatf("create_object_by_name: type '%s' is not registered", requested_type_name));
        return null;
      end
      return create_object_by_type(m_types[requested_type_name], parent_inst_path, name);
    endfunction

    function void print(int all_types = 1);
      string n;
      $display("#### Factory Configuration (*)");
      if (m_overrides.first(n)) begin
        do $display("  Type override: %s -> %s", n, m_overrides[n].get_type_name());
        while (m_overrides.next(n));
      end else
        $display("  No type overrides are registered with this factory");
      foreach (m_inst_paths[i])
        $display("  Instance override: %s (%s -> %s)", m_inst_paths[i], m_inst_orig[i].get_type_name(), m_inst_new[i].get_type_name());
      if (all_types && m_types.first(n)) begin
        $display("  Registered types:");
        do $display("    %s", n);
        while (m_types.next(n));
      end
    endfunction
  endclass

  class uvm_component_registry #(type T = uvm_component, string Tname = "<unknown>") extends uvm_object_wrapper;
    typedef uvm_component_registry #(T, Tname) this_type;
    local static this_type me = get();

    static function this_type get();
      if (me == null) begin
        me = new;
        if (Tname != "<unknown>") uvm_factory::get().register(me);
      end
      return me;
    endfunction

    virtual function string get_type_name();
      return Tname;
    endfunction

    virtual function uvm_component create_component(string name, uvm_component parent);
      T obj;
      obj = new(name, parent);
      return obj;
    endfunction

    static function T create(string name, uvm_component parent, string contxt = "");
      uvm_component c;
      T t;
      string path;
      path = contxt;
      if (path == "" && parent != null) path = parent.get_full_name();
      c = uvm_factory::get().create_component_by_type(get(), path, name, parent);
      if (!$cast(t, c))
        uvm_report_server::report(3, "FCTTYP", $sformatf("factory override for %s created an incompatible type", Tname), UVM_NONE, "", 0, "reporter");
      return t;
    endfunction

    static function void set_type_override(uvm_object_wrapper override_type, bit replace = 1);
      uvm_factory::get().set_type_override_by_type(get(), override_type, replace);
    endfunction

    static function void set_inst_override(uvm_object_wrapper override_type, string inst_path, uvm_component parent = null);
      string p;
      p = inst_path;
      if (parent != null) p = {parent.get_full_name(), ".", inst_path};
      uvm_factory::get().set_inst_override_by_type(get(), override_type, p);
    endfunction
  endclass

  class uvm_object_registry #(type T = uvm_object, string Tname = "<unknown>") extends uvm_object_wrapper;
    typedef uvm_object_registry #(T, Tname) this_type;
    local static this_type me = get();

    static function this_type get();
      if (me == null) begin
        me = new;
        if (Tname != "<unknown>") uvm_factory::get().register(me);
      end
      return me;
    endfunction

    virtual function string get_type_name();
      return Tname;
    endfunction

    virtual function uvm_object create_object(string name = "");
      T obj;
      obj = new(name);
      return obj;
    endfunction

    static function T create(string name = "", uvm_component parent = null, string contxt = "");
      uvm_object o;
      T t;
      string path;
      path = contxt;
      if (path == "" && parent != null) path = parent.get_full_name();
      o = uvm_factory::get().create_object_by_type(get(), path, name);
      if (!$cast(t, o))
        uvm_report_server::report(3, "FCTTYP", $sformatf("factory override for %s created an incompatible type", Tname), UVM_NONE, "", 0, "reporter");
      return t;
    endfunction

    static function void set_type_override(uvm_object_wrapper override_type, bit replace = 1);
      uvm_factory::get().set_type_override_by_type(get(), override_type, replace);
    endfunction

    static function void set_inst_override(uvm_object_wrapper override_type, string inst_path, uvm_component parent = null);
      string p;
      p = inst_path;
      if (parent != null) p = {parent.get_full_name(), ".", inst_path};
      uvm_factory::get().set_inst_override_by_type(get(), override_type, p);
    endfunction
  endclass

  // -------------------------------------------------------------------------
  // Phases and objections
  // -------------------------------------------------------------------------
  class uvm_objection extends uvm_object;
    int m_count;
    int m_total;
    time m_drain;
    function new(string name = "objection");
      super.new(name);
    endfunction
    function void set_drain_time(uvm_object obj = null, time drain = 0);
      m_drain = drain;
    endfunction
    function void raise_objection(uvm_object obj = null, string description = "", int count = 1);
      m_count += count;
      m_total += count;
    endfunction
    function void drop_objection(uvm_object obj = null, string description = "", int count = 1);
      if (m_count < count) begin
        uvm_report_server::report(2, "OBJTN_ZERO", "drop_objection() without a matching raise_objection()", UVM_NONE, "", 0, (obj == null) ? "reporter" : obj.get_full_name());
        return;
      end
      m_count -= count;
    endfunction
    function int get_objection_count(uvm_object obj = null);
      return m_count;
    endfunction
  endclass

  class uvm_phase extends uvm_object;
    uvm_objection phase_done;
    function new(string name = "phase");
      super.new(name);
      phase_done = new({name, "_objection"});
    endfunction
    virtual function string get_type_name();
      return "uvm_phase";
    endfunction
    function void raise_objection(uvm_object obj = null, string description = "", int count = 1);
      phase_done.raise_objection(obj, description, count);
    endfunction
    function void drop_objection(uvm_object obj = null, string description = "", int count = 1);
      phase_done.drop_objection(obj, description, count);
    endfunction
    function uvm_objection get_objection();
      return phase_done;
    endfunction
  endclass

  // -------------------------------------------------------------------------
  // Components
  // -------------------------------------------------------------------------
  virtual class uvm_component extends uvm_object;
    uvm_component m_parent;
    uvm_component m_children[string];
    string m_child_order[$];
    string m_full_name;

    function new(string name, uvm_component parent);
      super.new(name);
      if (parent == null && name != "__top__") parent = uvm_root::get();
      m_parent = parent;
      m_full_name = name;
      if (parent != null) begin
        if (parent.m_children.exists(name))
          uvm_report_warning("BDCLD", $sformatf("a component named '%s' already exists under '%s'", name, parent.get_full_name()));
        else
          parent.m_child_order.push_back(name);
        parent.m_children[name] = this;
        if (parent.get_name() != "__top__") m_full_name = {parent.get_full_name(), ".", name};
      end
    endfunction

    virtual function string get_full_name();
      return m_full_name;
    endfunction
    virtual function string get_type_name();
      return "uvm_component";
    endfunction
    function uvm_component get_parent();
      return m_parent;
    endfunction
    function int get_num_children();
      return m_children.num();
    endfunction
    function uvm_component get_child(string name);
      if (m_children.exists(name)) return m_children[name];
      return null;
    endfunction
    function void get_children(ref uvm_component children[$]);
      foreach (m_child_order[i]) children.push_back(m_children[m_child_order[i]]);
    endfunction
    function uvm_component lookup(string name);
      uvm_component c;
      string rest;
      int dot;
      c = this;
      rest = name;
      while (rest != "" && c != null) begin
        dot = -1;
        for (int i = 0; i < rest.len(); i++)
          if (rest.getc(i) == "." && dot < 0) dot = i;
        if (dot < 0) begin
          c = c.get_child(rest);
          rest = "";
        end else begin
          c = c.get_child(rest.substr(0, dot - 1));
          rest = rest.substr(dot + 1, rest.len() - 1);
        end
      end
      return c;
    endfunction
    function void set_report_verbosity_level(int verbosity_level);
      uvm_report_server::m_verbosity = verbosity_level;
    endfunction
    function void set_report_verbosity_level_hier(int verbosity_level);
      uvm_report_server::m_verbosity = verbosity_level;
    endfunction

    virtual function void build_phase(uvm_phase phase);
    endfunction
    virtual function void connect_phase(uvm_phase phase);
    endfunction
    virtual function void end_of_elaboration_phase(uvm_phase phase);
    endfunction
    virtual function void start_of_simulation_phase(uvm_phase phase);
    endfunction
    virtual task run_phase(uvm_phase phase);
    endtask
    virtual function void extract_phase(uvm_phase phase);
    endfunction
    virtual function void check_phase(uvm_phase phase);
    endfunction
    virtual function void report_phase(uvm_phase phase);
    endfunction
    virtual function void final_phase(uvm_phase phase);
    endfunction
  endclass

  class uvm_root extends uvm_component;
    static uvm_root m_inst;
    uvm_component m_test_top;
    time m_timeout;
    event m_never;

    static function uvm_root get();
      if (m_inst == null) m_inst = new;
      return m_inst;
    endfunction

    function new();
      super.new("__top__", null);
      m_full_name = "";
    endfunction

    virtual function string get_type_name();
      return "uvm_root";
    endfunction
    virtual function string m_report_path();
      return "reporter";
    endfunction

    function void set_timeout(time timeout, bit overridable = 1);
      m_timeout = timeout;
    endfunction

    function void m_call(uvm_component c, int which, uvm_phase ph);
      case (which)
        0: c.build_phase(ph);
        1: c.connect_phase(ph);
        2: c.end_of_elaboration_phase(ph);
        3: c.start_of_simulation_phase(ph);
        4: c.extract_phase(ph);
        5: c.check_phase(ph);
        6: c.report_phase(ph);
        default: c.final_phase(ph);
      endcase
    endfunction

    function void m_do_phase(uvm_component c, int which, uvm_phase ph, bit top_down);
      uvm_component kids[$];
      if (top_down) m_call(c, which, ph);
      c.get_children(kids);
      foreach (kids[i]) m_do_phase(kids[i], which, ph, top_down);
      if (!top_down) m_call(c, which, ph);
    endfunction

    function void m_collect(uvm_component c, ref uvm_component all[$]);
      uvm_component kids[$];
      all.push_back(c);
      c.get_children(kids);
      foreach (kids[i]) m_collect(kids[i], all);
    endfunction

    function void m_print_tree(uvm_component c, string indent);
      uvm_component kids[$];
      $display("%s%s (%s)", indent, c.get_name(), c.get_type_name());
      c.get_children(kids);
      foreach (kids[i]) m_print_tree(kids[i], {indent, "  "});
    endfunction

    function void print_topology(uvm_printer printer = null);
      uvm_report_info("UVMTOP", "UVM testbench topology:", UVM_LOW);
      if (m_test_top != null) m_print_tree(m_test_top, "");
    endfunction

    task run_test(string test_name = "");
      uvm_phase ph;
      uvm_component all[$];
      string tn;
      string vs;
      int tmo;
      tn = test_name;
      if ($value$plusargs("UVM_VERBOSITY=%s", vs)) begin
        case (vs)
          "UVM_NONE": uvm_report_server::m_verbosity = UVM_NONE;
          "UVM_LOW": uvm_report_server::m_verbosity = UVM_LOW;
          "UVM_MEDIUM": uvm_report_server::m_verbosity = UVM_MEDIUM;
          "UVM_HIGH": uvm_report_server::m_verbosity = UVM_HIGH;
          "UVM_FULL": uvm_report_server::m_verbosity = UVM_FULL;
          "UVM_DEBUG": uvm_report_server::m_verbosity = UVM_DEBUG;
          default: uvm_report_server::m_verbosity = vs.atoi();
        endcase
      end
      if ($value$plusargs("UVM_TIMEOUT=%d", tmo)) m_timeout = tmo;
      void'($value$plusargs("UVM_TESTNAME=%s", tn));
      if (tn == "") begin
        uvm_report_server::report(3, "NOTEST", "No test specified: pass a name to run_test() or use +UVM_TESTNAME=<test>", UVM_NONE, "", 0, "reporter");
        return;
      end
      uvm_report_server::report(0, "RNTST", $sformatf("Running test %s...", tn), UVM_LOW, "", 0, "reporter");
      m_test_top = uvm_factory::get().create_component_by_name(tn, "", "uvm_test_top", null);
      if (m_test_top == null) return;

      ph = new("build");              m_do_phase(m_test_top, 0, ph, 1);
      ph = new("connect");            m_do_phase(m_test_top, 1, ph, 0);
      ph = new("end_of_elaboration"); m_do_phase(m_test_top, 2, ph, 0);
      ph = new("start_of_simulation");m_do_phase(m_test_top, 3, ph, 0);

      ph = new("run");
      m_collect(m_test_top, all);
      fork
        begin
          foreach (all[i]) begin
            automatic uvm_component c = all[i];
            fork
              c.run_phase(ph);
            join_none
          end
          fork
            begin
              #0;
              wait (ph.phase_done.m_count == 0);
              if (ph.phase_done.m_drain > 0) #(ph.phase_done.m_drain);
              wait (ph.phase_done.m_count == 0);
            end
            begin
              if (m_timeout > 0) begin
                #(m_timeout);
                uvm_report_server::report(3, "PH_TIMEOUT", $sformatf("Explicit timeout of %0t hit, indicating a probable testbench issue", m_timeout), UVM_NONE, "", 0, "reporter");
              end else
                @(m_never);
            end
          join_any
          disable fork;
        end
      join

      ph = new("extract");            m_do_phase(m_test_top, 4, ph, 0);
      ph = new("check");              m_do_phase(m_test_top, 5, ph, 0);
      ph = new("report");             m_do_phase(m_test_top, 6, ph, 0);
      ph = new("final");              m_do_phase(m_test_top, 7, ph, 1);
      uvm_report_server::summarize();
      $finish;
    endtask
  endclass

  uvm_root uvm_top = uvm_root::get();

  task run_test(string test_name = "");
    uvm_root top;
    top = uvm_root::get();
    top.run_test(test_name);
  endtask

  // -------------------------------------------------------------------------
  // Configuration database
  // -------------------------------------------------------------------------
  class uvm_config_db #(type T = int);
    static string m_scope[$];
    static string m_field[$];
    static T m_val[$];

    static function void set(uvm_component cntxt, string inst_name, string field_name, T value);
      string scope;
      if (cntxt == null || cntxt.get_full_name() == "") scope = inst_name;
      else if (inst_name == "") scope = cntxt.get_full_name();
      else scope = {cntxt.get_full_name(), ".", inst_name};
      m_scope.push_back(scope);
      m_field.push_back(field_name);
      m_val.push_back(value);
    endfunction

    static function bit get(uvm_component cntxt, string inst_name, string field_name, inout T value);
      string path;
      if (cntxt == null || cntxt.get_full_name() == "") path = inst_name;
      else if (inst_name == "") path = cntxt.get_full_name();
      else path = {cntxt.get_full_name(), ".", inst_name};
      for (int i = m_scope.size() - 1; i >= 0; i--)
        if ($helion_glob(m_field[i], field_name) && $helion_glob(m_scope[i], path)) begin
          value = m_val[i];
          return 1;
        end
      return 0;
    endfunction

    static function bit exists(uvm_component cntxt, string inst_name, string field_name, bit spell_chk = 0);
      T tmp;
      return get(cntxt, inst_name, field_name, tmp);
    endfunction
  endclass

  // -------------------------------------------------------------------------
  // TLM
  // -------------------------------------------------------------------------
  virtual class uvm_port_base #(type T = int) extends uvm_object;
    uvm_component m_comp;
    uvm_port_base #(T) m_imps[$];
    function new(string name, uvm_component parent);
      super.new(name);
      m_comp = parent;
    endfunction
    virtual function string get_full_name();
      return (m_comp == null) ? get_name() : {m_comp.get_full_name(), ".", get_name()};
    endfunction
    virtual function void connect(uvm_port_base #(T) provider);
      if (provider == null) begin
        uvm_report_error("CONNECT", {get_full_name(), ": connect() to a null port/export"});
        return;
      end
      m_imps.push_back(provider);
    endfunction
    virtual function int size();
      return m_imps.size();
    endfunction
    function void m_need(string what);
      if (m_imps.size() == 0) uvm_report_fatal("NOCONN", {get_full_name(), " is not connected (", what, ")"});
    endfunction
    virtual function void write(T t);
    endfunction
    virtual task put(T t);
    endtask
    virtual task get(output T t);
    endtask
    virtual task peek(output T t);
    endtask
    virtual function bit try_put(T t);
      return 0;
    endfunction
    virtual function bit try_get(output T t);
      return 0;
    endfunction
    virtual function bit try_peek(output T t);
      return 0;
    endfunction
    virtual function bit can_put();
      return 0;
    endfunction
    virtual function bit can_get();
      return 0;
    endfunction
  endclass

  class uvm_analysis_port #(type T = int) extends uvm_port_base #(T);
    function new(string name, uvm_component parent = null);
      super.new(name, parent);
    endfunction
    virtual function void write(T t);
      foreach (m_imps[i]) m_imps[i].write(t);
    endfunction
  endclass

  class uvm_analysis_export #(type T = int) extends uvm_port_base #(T);
    function new(string name, uvm_component parent = null);
      super.new(name, parent);
    endfunction
    virtual function void write(T t);
      foreach (m_imps[i]) m_imps[i].write(t);
    endfunction
  endclass

  class uvm_analysis_imp #(type T = int, type IMP = int) extends uvm_port_base #(T);
    IMP m_imp;
    function new(string name, IMP imp);
      super.new(name, imp);
      m_imp = imp;
    endfunction
    virtual function void write(T t);
      m_imp.write(t);
    endfunction
  endclass

  // Put / get ports forward to the first connected export.
  class uvm_tlm_port #(type T = int) extends uvm_port_base #(T);
    function new(string name, uvm_component parent = null, int min_size = 1, int max_size = 1);
      super.new(name, parent);
    endfunction
    virtual task put(T t);
      m_need("put");
      m_imps[0].put(t);
    endtask
    virtual task get(output T t);
      m_need("get");
      m_imps[0].get(t);
    endtask
    virtual task peek(output T t);
      m_need("peek");
      m_imps[0].peek(t);
    endtask
    virtual function bit try_put(T t);
      m_need("try_put");
      return m_imps[0].try_put(t);
    endfunction
    virtual function bit try_get(output T t);
      m_need("try_get");
      return m_imps[0].try_get(t);
    endfunction
    virtual function bit try_peek(output T t);
      m_need("try_peek");
      return m_imps[0].try_peek(t);
    endfunction
    virtual function bit can_put();
      return m_imps.size() > 0 && m_imps[0].can_put();
    endfunction
    virtual function bit can_get();
      return m_imps.size() > 0 && m_imps[0].can_get();
    endfunction
  endclass

  class uvm_put_port #(type T = int) extends uvm_tlm_port #(T);
    function new(string name, uvm_component parent = null, int min_size = 1, int max_size = 1); super.new(name, parent); endfunction
  endclass
  class uvm_blocking_put_port #(type T = int) extends uvm_tlm_port #(T);
    function new(string name, uvm_component parent = null, int min_size = 1, int max_size = 1); super.new(name, parent); endfunction
  endclass
  class uvm_nonblocking_put_port #(type T = int) extends uvm_tlm_port #(T);
    function new(string name, uvm_component parent = null, int min_size = 1, int max_size = 1); super.new(name, parent); endfunction
  endclass
  class uvm_get_port #(type T = int) extends uvm_tlm_port #(T);
    function new(string name, uvm_component parent = null, int min_size = 1, int max_size = 1); super.new(name, parent); endfunction
  endclass
  class uvm_blocking_get_port #(type T = int) extends uvm_tlm_port #(T);
    function new(string name, uvm_component parent = null, int min_size = 1, int max_size = 1); super.new(name, parent); endfunction
  endclass
  class uvm_nonblocking_get_port #(type T = int) extends uvm_tlm_port #(T);
    function new(string name, uvm_component parent = null, int min_size = 1, int max_size = 1); super.new(name, parent); endfunction
  endclass
  class uvm_get_peek_port #(type T = int) extends uvm_tlm_port #(T);
    function new(string name, uvm_component parent = null, int min_size = 1, int max_size = 1); super.new(name, parent); endfunction
  endclass
  class uvm_blocking_get_peek_port #(type T = int) extends uvm_tlm_port #(T);
    function new(string name, uvm_component parent = null, int min_size = 1, int max_size = 1); super.new(name, parent); endfunction
  endclass
  class uvm_blocking_peek_port #(type T = int) extends uvm_tlm_port #(T);
    function new(string name, uvm_component parent = null, int min_size = 1, int max_size = 1); super.new(name, parent); endfunction
  endclass

  typedef class uvm_tlm_fifo;

  class uvm_tlm_fifo_imp #(type T = int) extends uvm_port_base #(T);
    uvm_tlm_fifo #(T) m_fifo;
    function new(string name, uvm_tlm_fifo #(T) fifo);
      super.new(name, fifo);
      m_fifo = fifo;
    endfunction
    virtual task put(T t);
      m_fifo.put(t);
    endtask
    virtual task get(output T t);
      m_fifo.get(t);
    endtask
    virtual task peek(output T t);
      m_fifo.peek(t);
    endtask
    virtual function bit try_put(T t);
      return m_fifo.try_put(t);
    endfunction
    virtual function bit try_get(output T t);
      return m_fifo.try_get(t);
    endfunction
    virtual function bit try_peek(output T t);
      return m_fifo.try_peek(t);
    endfunction
    virtual function bit can_put();
      return !m_fifo.is_full();
    endfunction
    virtual function bit can_get();
      return !m_fifo.is_empty();
    endfunction
    virtual function void write(T t);
      void'(m_fifo.try_put(t));
    endfunction
  endclass

  class uvm_tlm_fifo #(type T = int) extends uvm_component;
    mailbox #(T) m;
    int m_size;
    uvm_tlm_fifo_imp #(T) put_export;
    uvm_tlm_fifo_imp #(T) blocking_put_export;
    uvm_tlm_fifo_imp #(T) nonblocking_put_export;
    uvm_tlm_fifo_imp #(T) get_export;
    uvm_tlm_fifo_imp #(T) blocking_get_export;
    uvm_tlm_fifo_imp #(T) nonblocking_get_export;
    uvm_tlm_fifo_imp #(T) get_peek_export;
    uvm_tlm_fifo_imp #(T) blocking_get_peek_export;
    uvm_tlm_fifo_imp #(T) peek_export;
    uvm_tlm_fifo_imp #(T) blocking_peek_export;

    function new(string name, uvm_component parent = null, int size = 1);
      uvm_tlm_fifo_imp #(T) imp;
      super.new(name, parent);
      m_size = size;
      m = new(size);
      imp = new("export", this);
      put_export = imp;
      blocking_put_export = imp;
      nonblocking_put_export = imp;
      get_export = imp;
      blocking_get_export = imp;
      nonblocking_get_export = imp;
      get_peek_export = imp;
      blocking_get_peek_export = imp;
      peek_export = imp;
      blocking_peek_export = imp;
    endfunction
    virtual function string get_type_name();
      return "uvm_tlm_fifo";
    endfunction
    virtual task put(T t);
      m.put(t);
    endtask
    virtual task get(output T t);
      m.get(t);
    endtask
    virtual task peek(output T t);
      m.peek(t);
    endtask
    virtual function bit try_put(T t);
      return m.try_put(t);
    endfunction
    virtual function bit try_get(output T t);
      return m.try_get(t);
    endfunction
    virtual function bit try_peek(output T t);
      return m.try_peek(t);
    endfunction
    function int used();
      return m.num();
    endfunction
    function int size();
      return m_size;
    endfunction
    function bit is_empty();
      return m.num() == 0;
    endfunction
    function bit is_full();
      return m_size != 0 && m.num() >= m_size;
    endfunction
    virtual function void flush();
      T t;
      while (m.try_get(t));
    endfunction
  endclass

  class uvm_tlm_analysis_fifo #(type T = int) extends uvm_tlm_fifo #(T);
    typedef uvm_tlm_analysis_fifo #(T) this_type;
    `uvm_component_param_utils(this_type)
    uvm_tlm_fifo_imp #(T) analysis_export;
    function new(string name, uvm_component parent = null);
      super.new(name, parent, 0);
      analysis_export = put_export;
    endfunction
    virtual function string get_type_name();
      return "uvm_tlm_analysis_fifo";
    endfunction
    virtual function void write(T t);
      void'(m.try_put(t));
    endfunction
  endclass

  // -------------------------------------------------------------------------
  // Sequences
  // -------------------------------------------------------------------------
  class uvm_sequence_item extends uvm_object;
    uvm_sequencer_base m_sequencer;
    uvm_sequence_base m_parent_sequence;
    int m_transaction_id = -1;
    bit m_item_done;

    function new(string name = "uvm_sequence_item");
      super.new(name);
    endfunction
    virtual function string get_type_name();
      return "uvm_sequence_item";
    endfunction
    function void set_id_info(uvm_sequence_item item);
      m_transaction_id = item.m_transaction_id;
      m_parent_sequence = item.m_parent_sequence;
      m_sequencer = item.m_sequencer;
    endfunction
    function int get_transaction_id();
      return m_transaction_id;
    endfunction
    function uvm_sequencer_base get_sequencer();
      return m_sequencer;
    endfunction
    function void set_sequencer(uvm_sequencer_base sequencer);
      m_sequencer = sequencer;
    endfunction
    function uvm_sequence_base get_parent_sequence();
      return m_parent_sequence;
    endfunction
    virtual function string get_full_name();
      if (m_sequencer != null) return {m_sequencer.get_full_name(), ".", get_name()};
      return get_name();
    endfunction
    virtual function string m_report_path();
      if (m_sequencer != null) return {m_sequencer.get_full_name(), "@@", get_name()};
      return get_name();
    endfunction
  endclass

  virtual class uvm_sequence_base extends uvm_sequence_item;
    mailbox m_rsp_mbx;
    static int m_next_id;

    function new(string name = "uvm_sequence");
      super.new(name);
      m_rsp_mbx = new;
    endfunction
    virtual function string get_type_name();
      return "uvm_sequence_base";
    endfunction

    virtual task body();
    endtask
    virtual task pre_body();
    endtask
    virtual task post_body();
    endtask
    virtual task pre_start();
    endtask
    virtual task post_start();
    endtask
    virtual task pre_do(bit is_item);
    endtask
    virtual function void mid_do(uvm_sequence_item this_item);
    endfunction
    virtual function void post_do(uvm_sequence_item this_item);
    endfunction
    virtual function void m_set_p_sequencer();
    endfunction

    virtual task start(uvm_sequencer_base sequencer, uvm_sequence_base parent_sequence = null, int this_priority = -1, bit call_pre_post = 1);
      m_sequencer = sequencer;
      m_parent_sequence = parent_sequence;
      m_set_p_sequencer();
      pre_start();
      if (call_pre_post) pre_body();
      body();
      if (call_pre_post) post_body();
      post_start();
    endtask

    function uvm_sequence_item create_item(uvm_object_wrapper type_var, uvm_sequencer_base l_sequencer, string name);
      uvm_object o;
      uvm_sequence_item it;
      o = uvm_factory::get().create_object_by_type(type_var, (l_sequencer == null) ? "" : l_sequencer.get_full_name(), name);
      void'($cast(it, o));
      if (it != null) begin
        it.m_sequencer = l_sequencer;
        it.m_parent_sequence = this;
      end
      return it;
    endfunction

    virtual task start_item(uvm_sequence_item item, int set_priority = -1, uvm_sequencer_base sequencer = null);
      uvm_sequencer_base s;
      s = sequencer;
      if (s == null) s = m_sequencer;
      if (item == null) begin
        uvm_report_fatal("NULLITM", "start_item() called with a null item: create it first (type_id::create)");
        return;
      end
      if (s == null) begin
        uvm_report_fatal("NOSEQR", "start_item(): the sequence has no sequencer; start it with seq.start(sequencer)");
        return;
      end
      item.m_sequencer = s;
      item.m_parent_sequence = this;
      m_next_id++;
      item.m_transaction_id = m_next_id;
      s.m_wait_for_grant(this);
      pre_do(1);
    endtask

    virtual task finish_item(uvm_sequence_item item, int set_priority = -1);
      uvm_sequencer_base s;
      s = item.m_sequencer;
      mid_do(item);
      s.m_send_request(item);
      s.m_wait_for_item_done(item);
      post_do(item);
      s.m_release(this);
    endtask

    virtual task get_base_response(output uvm_sequence_item response, input int transaction_id = -1);
      m_rsp_mbx.get(response);
    endtask
  endclass

  virtual class uvm_sequence #(type REQ = uvm_sequence_item, type RSP = REQ) extends uvm_sequence_base;
    REQ req;
    RSP rsp;
    function new(string name = "uvm_sequence");
      super.new(name);
    endfunction
    virtual task get_response(output RSP response, input int transaction_id = -1);
      uvm_sequence_item it;
      m_rsp_mbx.get(it);
      void'($cast(response, it));
    endtask
  endclass

  class uvm_sequencer_base extends uvm_component;
    semaphore m_arb;
    mailbox m_req;
    function new(string name, uvm_component parent);
      super.new(name, parent);
      m_arb = new(1);
      m_req = new;
    endfunction
    virtual function string get_type_name();
      return "uvm_sequencer_base";
    endfunction
    virtual task m_wait_for_grant(uvm_sequence_base seq);
      m_arb.get(1);
    endtask
    virtual function void m_release(uvm_sequence_base seq);
      m_arb.put(1);
    endfunction
    virtual task m_send_request(uvm_sequence_item item);
      item.m_item_done = 0;
      m_req.put(item);
    endtask
    virtual task m_wait_for_item_done(uvm_sequence_item item);
      wait (item.m_item_done);
    endtask
    // Driver side.
    virtual task get_next_item(output uvm_sequence_item t);
      m_req.peek(t);
    endtask
    virtual function bit try_next_item(output uvm_sequence_item t);
      return m_req.try_peek(t);
    endfunction
    virtual function void item_done(uvm_sequence_item rsp = null);
      uvm_sequence_item t;
      if (!m_req.try_get(t)) begin
        uvm_report_error("ITMDN", "item_done() called with no outstanding item (missing get_next_item()?)");
        return;
      end
      t.m_item_done = 1;
      if (rsp != null) put_response(rsp);
    endfunction
    virtual task get(output uvm_sequence_item t);
      m_req.get(t);
      t.m_item_done = 1;
    endtask
    virtual function void put_response(uvm_sequence_item rsp);
      if (rsp == null) return;
      if (rsp.m_parent_sequence == null) begin
        uvm_report_error("NOSEQ", "put_response(): response has no parent sequence (call rsp.set_id_info(req))");
        return;
      end
      void'(rsp.m_parent_sequence.m_rsp_mbx.try_put(rsp));
    endfunction
  endclass

  class uvm_seq_item_pull_imp #(type REQ = uvm_sequence_item, type RSP = REQ) extends uvm_object;
    uvm_sequencer_base m_sqr;
    function new(string name, uvm_sequencer_base sqr);
      super.new(name);
      m_sqr = sqr;
    endfunction
  endclass

  class uvm_sequencer #(type REQ = uvm_sequence_item, type RSP = REQ) extends uvm_sequencer_base;
    typedef uvm_sequencer #(REQ, RSP) this_type;
    `uvm_component_param_utils(this_type)
    uvm_seq_item_pull_imp #(REQ, RSP) seq_item_export;
    function new(string name, uvm_component parent = null);
      super.new(name, parent);
      seq_item_export = new("seq_item_export", this);
    endfunction
    virtual function string get_type_name();
      return "uvm_sequencer";
    endfunction
  endclass

  class uvm_seq_item_pull_port #(type REQ = uvm_sequence_item, type RSP = REQ) extends uvm_object;
    uvm_sequencer_base m_sqr;
    uvm_component m_comp;
    function new(string name, uvm_component parent = null, int min_size = 0, int max_size = 1);
      super.new(name);
      m_comp = parent;
    endfunction
    virtual function string get_full_name();
      return (m_comp == null) ? get_name() : {m_comp.get_full_name(), ".", get_name()};
    endfunction
    function void connect(uvm_seq_item_pull_imp #(REQ, RSP) provider);
      if (provider == null) begin
        uvm_report_error("CONNECT", {get_full_name(), ": connect() to a null export"});
        return;
      end
      m_sqr = provider.m_sqr;
    endfunction
    function void m_check();
      if (m_sqr == null)
        uvm_report_fatal("NOCONN", {get_full_name(), " is not connected: call seq_item_port.connect(sequencer.seq_item_export) in connect_phase"});
    endfunction
    task get_next_item(output REQ t);
      uvm_sequence_item it;
      m_check();
      m_sqr.get_next_item(it);
      if (!$cast(t, it)) uvm_report_fatal("DRVCST", "sequence item type does not match the driver's REQ type");
    endtask
    task try_next_item(output REQ t);
      uvm_sequence_item it;
      m_check();
      if (m_sqr.try_next_item(it)) void'($cast(t, it));
      else t = null;
    endtask
    task get(output REQ t);
      uvm_sequence_item it;
      m_check();
      m_sqr.get(it);
      if (!$cast(t, it)) uvm_report_fatal("DRVCST", "sequence item type does not match the driver's REQ type");
    endtask
    function void item_done(RSP item = null);
      m_check();
      m_sqr.item_done(item);
    endfunction
    task put(RSP item);
      m_check();
      m_sqr.put_response(item);
    endtask
    function void put_response(RSP item);
      m_check();
      m_sqr.put_response(item);
    endfunction
  endclass

  class uvm_driver #(type REQ = uvm_sequence_item, type RSP = REQ) extends uvm_component;
    uvm_seq_item_pull_port #(REQ, RSP) seq_item_port;
    uvm_analysis_port #(RSP) rsp_port;
    REQ req;
    RSP rsp;
    function new(string name, uvm_component parent);
      super.new(name, parent);
      seq_item_port = new("seq_item_port", this);
      rsp_port = new("rsp_port", this);
    endfunction
    virtual function string get_type_name();
      return "uvm_driver";
    endfunction
  endclass

  class uvm_monitor extends uvm_component;
    function new(string name, uvm_component parent);
      super.new(name, parent);
    endfunction
    virtual function string get_type_name();
      return "uvm_monitor";
    endfunction
  endclass

  class uvm_scoreboard extends uvm_component;
    function new(string name, uvm_component parent);
      super.new(name, parent);
    endfunction
    virtual function string get_type_name();
      return "uvm_scoreboard";
    endfunction
  endclass

  class uvm_agent extends uvm_component;
    uvm_active_passive_enum is_active = UVM_ACTIVE;
    function new(string name, uvm_component parent);
      super.new(name, parent);
    endfunction
    virtual function string get_type_name();
      return "uvm_agent";
    endfunction
    virtual function uvm_active_passive_enum get_is_active();
      return is_active;
    endfunction
  endclass

  class uvm_env extends uvm_component;
    function new(string name, uvm_component parent);
      super.new(name, parent);
    endfunction
    virtual function string get_type_name();
      return "uvm_env";
    endfunction
  endclass

  class uvm_test extends uvm_component;
    function new(string name, uvm_component parent);
      super.new(name, parent);
    endfunction
    virtual function string get_type_name();
      return "uvm_test";
    endfunction
  endclass

  virtual class uvm_subscriber #(type T = int) extends uvm_component;
    uvm_analysis_imp #(T, uvm_subscriber #(T)) analysis_export;
    function new(string name, uvm_component parent);
      super.new(name, parent);
      analysis_export = new("analysis_export", this);
    endfunction
    pure virtual function void write(T t);
  endclass

endpackage

`endif
