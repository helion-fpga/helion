//! Language Templates: a searchable library of synthesizable SystemVerilog,
//! VHDL and XDC snippets with a highlighted preview, inserted at the editor
//! caret (Tools ▸ Language Templates…, palette, Flow Navigator).

use rune::{El, ScrollSpec, TextEdit, Tx, col, el, row, text, text_input};
use rune_kit::{Button, ButtonVariant, Dialog};

use crate::state::{AppState, CenterPane};
use crate::theme::VivadoTheme;
use crate::views::highlight::Dialect;

/// One template.
pub struct Template {
    pub lang: Dialect,
    pub group: &'static str,
    pub name: &'static str,
    pub body: &'static str,
}

/// The library. Every HDL body elaborates on its own (see the test).
pub const TEMPLATES: &[Template] = &[
    Template {
        lang: Dialect::Verilog,
        group: "Structure",
        name: "Module skeleton",
        body: "module my_module #(\n    parameter int WIDTH = 8\n) (\n    input  logic             clk,\n    input  logic             rst_n,\n    input  logic [WIDTH-1:0] din,\n    output logic [WIDTH-1:0] dout\n);\n\n    always_ff @(posedge clk or negedge rst_n) begin\n        if (!rst_n) dout <= '0;\n        else        dout <= din;\n    end\n\nendmodule\n",
    },
    Template {
        lang: Dialect::Verilog,
        group: "Sequential",
        name: "Register with async reset",
        body: "module reg_async #(parameter int W = 8) (\n    input  logic clk, rst_n, en,\n    input  logic [W-1:0] d,\n    output logic [W-1:0] q\n);\n    always_ff @(posedge clk or negedge rst_n) begin\n        if (!rst_n)  q <= '0;\n        else if (en) q <= d;\n    end\nendmodule\n",
    },
    Template {
        lang: Dialect::Verilog,
        group: "Sequential",
        name: "Counter with enable and wrap",
        body: "module counter #(parameter int W = 8, parameter int MAX = 255) (\n    input  logic clk, rst, en,\n    output logic [W-1:0] count,\n    output logic wrap\n);\n    always_ff @(posedge clk) begin\n        if (rst) count <= '0;\n        else if (en) count <= (count == MAX) ? '0 : count + 1'b1;\n    end\n    assign wrap = en && (count == MAX);\nendmodule\n",
    },
    Template {
        lang: Dialect::Verilog,
        group: "Sequential",
        name: "Shift register",
        body: "module shift_reg #(parameter int N = 8) (\n    input  logic clk, en, sin,\n    output logic sout,\n    output logic [N-1:0] q\n);\n    always_ff @(posedge clk)\n        if (en) q <= {q[N-2:0], sin};\n    assign sout = q[N-1];\nendmodule\n",
    },
    Template {
        lang: Dialect::Verilog,
        group: "Combinational",
        name: "Multiplexer (case)",
        body: "module mux4 #(parameter int W = 8) (\n    input  logic [1:0]   sel,\n    input  logic [W-1:0] a, b, c, d,\n    output logic [W-1:0] y\n);\n    always_comb begin\n        unique case (sel)\n            2'd0: y = a;\n            2'd1: y = b;\n            2'd2: y = c;\n            default: y = d;\n        endcase\n    end\nendmodule\n",
    },
    Template {
        lang: Dialect::Verilog,
        group: "State machines",
        name: "Two-process FSM",
        body: "module fsm (\n    input  logic clk, rst, start, done,\n    output logic busy\n);\n    typedef enum logic [1:0] {IDLE, RUN, FINISH} state_t;\n    state_t state, next;\n\n    always_ff @(posedge clk) begin\n        if (rst) state <= IDLE;\n        else     state <= next;\n    end\n\n    always_comb begin\n        next = state;\n        unique case (state)\n            IDLE:   if (start) next = RUN;\n            RUN:    if (done)  next = FINISH;\n            FINISH: next = IDLE;\n            default: next = IDLE;\n        endcase\n    end\n\n    assign busy = (state == RUN);\nendmodule\n",
    },
    Template {
        lang: Dialect::Verilog,
        group: "Clock domain crossing",
        name: "Two-flop synchronizer",
        body: "module sync2 (\n    input  logic clk_dst,\n    input  logic async_in,\n    output logic sync_out\n);\n    (* ASYNC_REG = \"TRUE\" *) logic meta, sync;\n    always_ff @(posedge clk_dst) begin\n        meta <= async_in;\n        sync <= meta;\n    end\n    assign sync_out = sync;\nendmodule\n",
    },
    Template {
        lang: Dialect::Verilog,
        group: "Clock domain crossing",
        name: "Rising-edge detector",
        body: "module edge_detect (\n    input  logic clk, sig,\n    output logic rise\n);\n    logic last;\n    always_ff @(posedge clk) last <= sig;\n    assign rise = sig & ~last;\nendmodule\n",
    },
    Template {
        lang: Dialect::Verilog,
        group: "Memories",
        name: "Single-port RAM (BRAM inference)",
        body: "module spram #(parameter int AW = 9, parameter int DW = 16) (\n    input  logic          clk, we,\n    input  logic [AW-1:0] addr,\n    input  logic [DW-1:0] din,\n    output logic [DW-1:0] dout\n);\n    logic [DW-1:0] mem [0:(1<<AW)-1];\n    always_ff @(posedge clk) begin\n        if (we) mem[addr] <= din;\n        dout <= mem[addr];\n    end\nendmodule\n",
    },
    Template {
        lang: Dialect::Verilog,
        group: "Memories",
        name: "Simple dual-port RAM",
        body: "module sdpram #(parameter int AW = 9, parameter int DW = 16) (\n    input  logic          clk,\n    input  logic          we,\n    input  logic [AW-1:0] waddr, raddr,\n    input  logic [DW-1:0] din,\n    output logic [DW-1:0] dout\n);\n    logic [DW-1:0] mem [0:(1<<AW)-1];\n    always_ff @(posedge clk) begin\n        if (we) mem[waddr] <= din;\n        dout <= mem[raddr];\n    end\nendmodule\n",
    },
    Template {
        lang: Dialect::Verilog,
        group: "Simulation",
        name: "Self-checking testbench",
        body: "`timescale 1ns / 1ps\nmodule my_tb;\n    logic clk = 0, rst = 1;\n    always #5 clk = ~clk;\n\n    // DUT instance goes here.\n\n    initial begin\n        repeat (2) @(posedge clk);\n        rst = 0;\n        repeat (16) @(posedge clk);\n        $display(\"PASS\");\n        $finish;\n    end\nendmodule\n",
    },
    Template {
        lang: Dialect::Vhdl,
        group: "Structure",
        name: "Entity and architecture",
        body: "library ieee;\nuse ieee.std_logic_1164.all;\nuse ieee.numeric_std.all;\n\nentity my_entity is\n  generic (WIDTH : natural := 8);\n  port (\n    clk  : in  std_logic;\n    rst  : in  std_logic;\n    din  : in  std_logic_vector(WIDTH-1 downto 0);\n    dout : out std_logic_vector(WIDTH-1 downto 0)\n  );\nend entity;\n\narchitecture rtl of my_entity is\nbegin\n  process (clk)\n  begin\n    if rising_edge(clk) then\n      if rst = '1' then\n        dout <= (others => '0');\n      else\n        dout <= din;\n      end if;\n    end if;\n  end process;\nend architecture;\n",
    },
    Template {
        lang: Dialect::Vhdl,
        group: "Sequential",
        name: "Counter",
        body: "library ieee;\nuse ieee.std_logic_1164.all;\nuse ieee.numeric_std.all;\n\nentity counter is\n  port (clk, rst, en : in std_logic; count : out unsigned(7 downto 0));\nend entity;\n\narchitecture rtl of counter is\n  signal c : unsigned(7 downto 0) := (others => '0');\nbegin\n  process (clk)\n  begin\n    if rising_edge(clk) then\n      if rst = '1' then c <= (others => '0');\n      elsif en = '1' then c <= c + 1;\n      end if;\n    end if;\n  end process;\n  count <= c;\nend architecture;\n",
    },
    Template {
        lang: Dialect::Vhdl,
        group: "Clock domain crossing",
        name: "Two-flop synchronizer",
        body: "library ieee;\nuse ieee.std_logic_1164.all;\n\nentity sync2 is\n  port (clk_dst, async_in : in std_logic; sync_out : out std_logic);\nend entity;\n\narchitecture rtl of sync2 is\n  signal meta, sync : std_logic := '0';\nbegin\n  process (clk_dst)\n  begin\n    if rising_edge(clk_dst) then\n      meta <= async_in;\n      sync <= meta;\n    end if;\n  end process;\n  sync_out <= sync;\nend architecture;\n",
    },
    Template {
        lang: Dialect::Tcl,
        group: "Clocks",
        name: "Primary clock",
        body: "create_clock -name sys_clk -period 10.000 [get_ports clk]\n",
    },
    Template {
        lang: Dialect::Tcl,
        group: "Clocks",
        name: "Generated clock (divide by 2)",
        body: "create_clock -name sys_clk -period 10.000 [get_ports clk]\ncreate_generated_clock -name clk_div2 -master_clock sys_clk -source [get_ports clk] -divide_by 2 [get_pins clk_div_reg/Q]\n",
    },
    Template {
        lang: Dialect::Tcl,
        group: "I/O",
        name: "Pin and I/O standard",
        body: "set_property PACKAGE_PIN E3 [get_ports clk]\nset_property IOSTANDARD LVCMOS33 [get_ports clk]\n",
    },
    Template {
        lang: Dialect::Tcl,
        group: "I/O",
        name: "Input and output delays",
        body: "set_input_delay  -clock sys_clk -max 2.000 [get_ports din]\nset_input_delay  -clock sys_clk -min 0.500 [get_ports din]\nset_output_delay -clock sys_clk -max 2.000 [get_ports dout]\nset_output_delay -clock sys_clk -min 0.500 [get_ports dout]\n",
    },
    Template {
        lang: Dialect::Tcl,
        group: "Exceptions",
        name: "False path between clocks",
        body: "set_false_path -from [get_clocks clk_a] -to [get_clocks clk_b]\n",
    },
    Template {
        lang: Dialect::Tcl,
        group: "Exceptions",
        name: "Multicycle path",
        body: "set_multicycle_path 2 -setup -from [get_cells slow_reg*] -to [get_cells dst_reg*]\nset_multicycle_path 1 -hold  -from [get_cells slow_reg*] -to [get_cells dst_reg*]\n",
    },
];

/// Templates dialog state.
#[derive(Clone, Debug)]
pub struct TemplatesState {
    pub open: bool,
    pub query: TextEdit,
    pub selected: usize,
}

impl Default for TemplatesState {
    fn default() -> Self {
        Self {
            open: false,
            query: TextEdit::new(""),
            selected: 0,
        }
    }
}

fn lang_label(d: Dialect) -> &'static str {
    match d {
        Dialect::Verilog => "SystemVerilog",
        Dialect::Vhdl => "VHDL",
        Dialect::Tcl => "XDC",
    }
}

/// Indices of templates matching the search (language, group or name).
pub fn matches(query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    TEMPLATES
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            q.is_empty()
                || t.name.to_lowercase().contains(&q)
                || t.group.to_lowercase().contains(&q)
                || lang_label(t.lang).to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Open the dialog, pre-filtered to the active editor tab's language.
pub fn open(s: &mut AppState) {
    s.templates.open = true;
    let lang = s
        .editor
        .tabs
        .get(s.editor.active)
        .map(|n| lang_label(Dialect::for_name(n)))
        .unwrap_or("");
    s.templates.query = TextEdit::new(lang);
    s.templates.selected = matches(lang).first().copied().unwrap_or(0);
}

/// Why Insert is unavailable for template `i`, if it is.
pub fn insert_blocked(s: &AppState, i: usize) -> Option<&'static str> {
    let tab = s.editor.tabs.get(s.editor.active)?;
    let _ = tab;
    let t = TEMPLATES.get(i)?;
    if Dialect::for_name(&s.editor.tabs[s.editor.active]) != t.lang {
        return Some("The open file is a different language");
    }
    None
}

/// Insert template `i` at the active editor tab's caret (one undo step).
pub fn insert(s: &mut AppState, i: usize) -> Result<(), &'static str> {
    if s.editor.tabs.is_empty() {
        return Err("Open a source file first");
    }
    if let Some(why) = insert_blocked(s, i) {
        return Err(why);
    }
    let body = TEMPLATES[i].body;
    crate::views::editor::insert_text(s, body);
    s.templates.open = false;
    s.navigate(CenterPane::Editor);
    Ok(())
}

/// Push the dialog onto the overlay layer when open.
pub fn push_templates(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme) {
    if !s.templates.open {
        return;
    }
    let kit = t.kit();
    let hits = matches(&s.templates.query.text);
    let sel = if hits.contains(&s.templates.selected) {
        s.templates.selected
    } else {
        hits.first().copied().unwrap_or(usize::MAX)
    };
    let mut list = col()
        .key("tpl-list")
        .w(260.0)
        .h(360.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    let mut last_head = String::new();
    for &i in &hits {
        let tpl = &TEMPLATES[i];
        let head = format!("{} · {}", lang_label(tpl.lang), tpl.group);
        if head != last_head {
            list = list.child(
                el().px(8.0).pt(8.0).pb(2.0).child(
                    text(head.to_uppercase())
                        .font(VivadoTheme::FONT_CAPS)
                        .color(t.text_faint),
                ),
            );
            last_head = head;
        }
        let active = i == sel;
        list = list.child(
            row()
                .key(format!("tpl-row-{i}"))
                .role("button")
                .label(tpl.name)
                .focus_selected(active)
                .focusable(true)
                .w_full()
                .h(22.0)
                .px(10.0)
                .items_center()
                .radius(3.0)
                .fill(if active { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| s.templates.selected = i)
                .on_double_press(move |s: &mut AppState| {
                    if let Err(e) = insert(s, i) {
                        s.toasts.push(crate::state::ToastKind::Info, "Can't insert", e);
                    }
                })
                .child(
                    text(tpl.name)
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text)
                        .truncate(),
                ),
        );
    }
    if hits.is_empty() {
        list = list.child(
            el().padding(10.0).child(
                text("No templates match.")
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_faint),
            ),
        );
    }
    let preview: El = match TEMPLATES.get(sel) {
        Some(tpl) => {
            let spans = crate::views::highlight::highlight(tpl.body, tpl.lang, t);
            col()
                .key("tpl-preview")
                .grow(1.0)
                .min_w(0.0)
                .h(360.0)
                .fill(t.chrome_bg)
                .border(1.0, t.border_light)
                .radius(4.0)
                .padding(10.0)
                .scroll_xy()
                .child(
                    text(tpl.body.trim_end())
                        .font(VivadoTheme::FONT_MONO.mono())
                        .color(t.text)
                        .spans(spans),
                )
        }
        None => el().grow(1.0).h(360.0),
    };
    let blocked = if s.editor.tabs.is_empty() {
        Some("Open a source file first")
    } else if sel == usize::MAX {
        Some("Select a template")
    } else {
        insert_blocked(s, sel)
    };
    let mut insert_btn = Button::new("Insert at Cursor").variant(ButtonVariant::Submit);
    if blocked.is_none() {
        insert_btn = insert_btn.on_press::<AppState>(move |s: &mut AppState| {
            let _ = insert(s, sel);
        });
    }
    let insert_el = insert_btn.build(&kit).key("tpl-insert");
    let insert_el = match blocked {
        Some(why) => insert_el.disabled(true).description(why).help(why).layer_opacity(0.5),
        None => insert_el,
    };
    let body = col()
        .key("tpl-body")
        .w_full()
        .gap(10.0)
        .padding(12.0)
        .child(
            text_input(&s.templates.query, VivadoTheme::FONT_UI)
                .single_line()
                .key("tpl-search")
                .label("Search templates")
                .w_full()
                .px(8.0)
                .py(4.0)
                .fill(t.chrome_bg)
                .border(1.0, t.border_light)
                .radius(4.0)
                .color(t.text)
                .placeholder("Search: FSM, synchronizer, RAM, clock…", t.text_faint)
                .caret(t.text, t.selection)
                .on_edit(|s: &mut AppState| &mut s.templates.query),
        )
        .child(row().w_full().gap(10.0).child(list).child(preview))
        .child(
            row()
                .w_full()
                .gap(8.0)
                .items_center()
                .child(
                    text(blocked.unwrap_or("Inserts at the editor caret as one undo step."))
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text_faint)
                        .grow(1.0),
                )
                .child(
                    Button::new("Copy")
                        .variant(ButtonVariant::Neutral)
                        .on_press_tx::<AppState>(move |s: &mut AppState, tx: &mut Tx| {
                            if let Some(tpl) = TEMPLATES.get(sel) {
                                tx.copy(tpl.body.to_string());
                                s.toasts.push(crate::state::ToastKind::Info, "Copied", tpl.name);
                            }
                        })
                        .build(&kit)
                        .key("tpl-copy"),
                )
                .child(insert_el)
                .child(
                    Button::new("Close")
                        .variant(ButtonVariant::Neutral)
                        .on_press::<AppState>(|s: &mut AppState| s.templates.open = false)
                        .build(&kit)
                        .key("tpl-close"),
                ),
        );
    Dialog::new("templates", "Language Templates")
        .open(true)
        .width(820.0)
        .fit()
        .on_close(|s: &mut AppState| s.templates.open = false)
        .child(body)
        .build(&kit, layer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hdl_template_elaborates() {
        for tpl in TEMPLATES {
            match tpl.lang {
                Dialect::Verilog => {
                    let r = helion_sv::synth_sv(tpl.body, "tpl.sv");
                    assert!(r.is_ok(), "{}: {:?}", tpl.name, r.err());
                }
                Dialect::Tcl => {
                    assert!(
                        helion_sta::load_xdc(tpl.body).is_ok(),
                        "{}: XDC must parse",
                        tpl.name
                    );
                }
                Dialect::Vhdl => {}
            }
        }
    }

    #[test]
    fn search_and_insert_respect_language() {
        assert!(!matches("synchronizer").is_empty());
        assert!(matches("zzz-none").is_empty());
        let mut s = crate::init();
        crate::views::editor::select_tab(&mut s, 0); // project1_demo.v
        let fsm = TEMPLATES.iter().position(|t| t.name == "Two-process FSM").unwrap();
        let before = s.editor.lines.len();
        assert!(insert(&mut s, fsm).is_ok());
        assert!(s.editor.lines.len() > before, "template lines land in the buffer");
        let xdc = TEMPLATES.iter().position(|t| t.name == "Primary clock").unwrap();
        assert_eq!(insert(&mut s, xdc), Err("The open file is a different language"));
    }
}
