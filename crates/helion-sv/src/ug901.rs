//! Vivado UG901 hardware resource inference for `helion-sv`:
//! - `(* use_dsp = "yes|no|simd|logic" *)` with `MREG`/`PREG` pipelining and `PCOUT`->`PCIN` cascade
//! - `(* ram_style = "block|distributed|registers|ultra" *)` and `(* rom_style = "block|distributed" *)`
//!   with asymmetric aspect ratios and byte-write enables
//! - `(* shreg_extract = "yes|no" *)` and `(* srl_style = "register|srl|srl_reg|reg_srl|reg_srl_reg|block" *)`
//!   `SRL16E`/`SRLC32E` dynamic and static shift registers
//! - `(* fsm_encoding = "one_hot|gray|sequential|johnson|auto|none" *)` with
//!   `(* fsm_safe_state = "reset_state|default_state|power_on_state|auto_safe_state" *)`

use std::collections::{BTreeMap, HashMap, HashSet};

use bumpalo::Bump;
use helion_ir::{resolve, CellKind, Design};

use crate::{
    addr_match_expr, bit_name, care_mask, clog2_u, emit_small_lut, normalize_mul_operands,
    sig_depth, sig_width, simple_word_enable, str_param_hash, unpacked_word_q, word_ce_mux, Aig,
    Expr, RExpr, Rtl, Signal, Tok,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UseDspMode {
    Yes,
    No,
    Simd,
    Logic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RamStyleMode {
    Block,
    Distributed,
    Registers,
    Ultra,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RomStyleMode {
    Block,
    Distributed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SrlStyleMode {
    Register,
    Srl,
    SrlReg,
    RegSrl,
    RegSrlReg,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FsmEncodingMode {
    OneHot,
    Gray,
    Sequential,
    Johnson,
    Auto,
    None,
}

impl FsmEncodingMode {
    pub(crate) fn as_attr_str(self) -> &'static str {
        match self {
            Self::OneHot => "ONE_HOT",
            Self::Gray => "GRAY",
            Self::Sequential => "SEQUENTIAL",
            Self::Johnson => "JOHNSON",
            Self::Auto => "AUTO",
            Self::None => "NONE",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FsmSafeStateMode {
    ResetState,
    DefaultState,
    PowerOnState,
    AutoSafeState,
}

impl FsmSafeStateMode {
    pub(crate) fn as_attr_str(self) -> &'static str {
        match self {
            Self::ResetState => "RESET_STATE",
            Self::DefaultState => "DEFAULT_STATE",
            Self::PowerOnState => "POWER_ON_STATE",
            Self::AutoSafeState => "AUTO_SAFE_STATE",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SignalUg901Attrs {
    pub(crate) use_dsp: Option<UseDspMode>,
    pub(crate) ram_style: Option<RamStyleMode>,
    pub(crate) rom_style: Option<RomStyleMode>,
    pub(crate) shreg_extract: Option<bool>,
    pub(crate) srl_style: Option<SrlStyleMode>,
    pub(crate) fsm_encoding: Option<FsmEncodingMode>,
    pub(crate) fsm_safe_state: Option<FsmSafeStateMode>,
}

impl SignalUg901Attrs {
    fn is_empty(&self) -> bool {
        self.use_dsp.is_none()
            && self.ram_style.is_none()
            && self.rom_style.is_none()
            && self.shreg_extract.is_none()
            && self.srl_style.is_none()
            && self.fsm_encoding.is_none()
            && self.fsm_safe_state.is_none()
    }

    fn merge_from(&mut self, other: &Self) {
        if other.use_dsp.is_some() {
            self.use_dsp = other.use_dsp;
        }
        if other.ram_style.is_some() {
            self.ram_style = other.ram_style;
        }
        if other.rom_style.is_some() {
            self.rom_style = other.rom_style;
        }
        if other.shreg_extract.is_some() {
            self.shreg_extract = other.shreg_extract;
        }
        if other.srl_style.is_some() {
            self.srl_style = other.srl_style;
        }
        if other.fsm_encoding.is_some() {
            self.fsm_encoding = other.fsm_encoding;
        }
        if other.fsm_safe_state.is_some() {
            self.fsm_safe_state = other.fsm_safe_state;
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Ug901ModuleAttrs {
    pub(crate) module_defaults: SignalUg901Attrs,
    pub(crate) per_signal: HashMap<String, SignalUg901Attrs>,
    pub(crate) enum_types: HashMap<String, Vec<(String, u128)>>,
    pub(crate) signal_types: HashMap<String, String>,
}

impl Ug901ModuleAttrs {
    pub(crate) fn effective_for(&self, sig: &str) -> SignalUg901Attrs {
        let mut out = self.module_defaults.clone();
        if let Some(ty) = self.signal_types.get(sig) {
            if let Some(ta) = self.per_signal.get(ty) {
                out.merge_from(ta);
            }
        }
        if let Some(sa) = self.per_signal.get(sig) {
            out.merge_from(sa);
        }
        out
    }
}

fn resolve_param_str(raw: &str, params: &[(String, u128)]) -> String {
    if let Some((_, val)) = params.iter().find(|(k, _)| k == raw) {
        for cand in [
            "yes",
            "no",
            "true",
            "false",
            "simd",
            "logic",
            "block",
            "distributed",
            "registers",
            "ultra",
            "register",
            "srl",
            "srl_reg",
            "reg_srl",
            "reg_srl_reg",
            "one_hot",
            "onehot",
            "gray",
            "sequential",
            "binary",
            "johnson",
            "auto",
            "none",
            "reset_state",
            "default_state",
            "power_on_state",
            "auto_safe_state",
        ] {
            if *val == str_param_hash(cand) {
                return cand.to_string();
            }
        }
    }
    raw.to_ascii_lowercase()
}

fn apply_kv_to_attrs(attrs: &mut SignalUg901Attrs, key: &str, val: &str) {
    let k = key.to_ascii_lowercase();
    let v = val.trim().to_ascii_lowercase();
    match k.as_str() {
        "use_dsp" | "use_dsp48" => {
            attrs.use_dsp = match v.as_str() {
                "yes" | "true" | "1" => Some(UseDspMode::Yes),
                "no" | "false" | "0" => Some(UseDspMode::No),
                "simd" | "two24" | "four12" => Some(UseDspMode::Simd),
                "logic" => Some(UseDspMode::Logic),
                _ => attrs.use_dsp,
            };
        }
        "ram_style" | "syn_ramstyle" => {
            attrs.ram_style = match v.as_str() {
                "block" | "block_ram" => Some(RamStyleMode::Block),
                "distributed" | "lutram" | "pipe_distributed" | "select_ram" => {
                    Some(RamStyleMode::Distributed)
                }
                "registers" | "reg" | "flip_flops" => Some(RamStyleMode::Registers),
                "ultra" | "uram" | "ultra_ram" => Some(RamStyleMode::Ultra),
                _ => attrs.ram_style,
            };
        }
        "rom_style" => {
            attrs.rom_style = match v.as_str() {
                "block" | "block_rom" => Some(RomStyleMode::Block),
                "distributed" | "lutrom" => Some(RomStyleMode::Distributed),
                _ => attrs.rom_style,
            };
        }
        "shreg_extract" => {
            attrs.shreg_extract = match v.as_str() {
                "yes" | "true" | "1" => Some(true),
                "no" | "false" | "0" => Some(false),
                _ => attrs.shreg_extract,
            };
        }
        "srl_style" => {
            attrs.srl_style = match v.as_str() {
                "register" | "registers" | "reg" => Some(SrlStyleMode::Register),
                "srl" => Some(SrlStyleMode::Srl),
                "srl_reg" => Some(SrlStyleMode::SrlReg),
                "reg_srl" => Some(SrlStyleMode::RegSrl),
                "reg_srl_reg" => Some(SrlStyleMode::RegSrlReg),
                "block" => Some(SrlStyleMode::Block),
                _ => attrs.srl_style,
            };
        }
        "fsm_encoding" | "syn_encoding" => {
            attrs.fsm_encoding = match v.as_str() {
                "one_hot" | "onehot" | "one-hot" => Some(FsmEncodingMode::OneHot),
                "gray" | "grey" => Some(FsmEncodingMode::Gray),
                "sequential" | "binary" => Some(FsmEncodingMode::Sequential),
                "johnson" => Some(FsmEncodingMode::Johnson),
                "auto" => Some(FsmEncodingMode::Auto),
                "none" | "user" => Some(FsmEncodingMode::None),
                _ => attrs.fsm_encoding,
            };
        }
        "fsm_safe_state" => {
            attrs.fsm_safe_state = match v.as_str() {
                "reset_state" | "yes" | "true" | "1" => Some(FsmSafeStateMode::ResetState),
                "default_state" => Some(FsmSafeStateMode::DefaultState),
                "power_on_state" => Some(FsmSafeStateMode::PowerOnState),
                "auto_safe_state" | "auto" => Some(FsmSafeStateMode::AutoSafeState),
                _ => attrs.fsm_safe_state,
            };
        }
        _ => {}
    }
}

fn parse_attr_block_at(toks: &[Tok], start: usize, params: &[(String, u128)]) -> Option<(Vec<(String, String)>, usize)> {
    if !matches!(toks.get(start), Some(Tok::Sym('(')))
        || !matches!(toks.get(start + 1), Some(Tok::Sym('*')))
    {
        return None;
    }
    let mut i = start + 2;
    let mut out = Vec::new();
    while i < toks.len() {
        if matches!(toks.get(i), Some(Tok::Sym('*')))
            && matches!(toks.get(i + 1), Some(Tok::Sym(')')))
        {
            return Some((out, i + 2));
        }
        let key = match toks.get(i) {
            Some(Tok::Ident(s) | Tok::Kw(s)) => {
                i += 1;
                (*s).to_string()
            }
            _ => {
                i += 1;
                continue;
            }
        };
        let val = if matches!(toks.get(i), Some(Tok::Sym('='))) {
            i += 1;
            match toks.get(i) {
                Some(Tok::Str(s)) => {
                    i += 1;
                    (*s).to_string()
                }
                Some(Tok::Ident(s) | Tok::Kw(s)) => {
                    i += 1;
                    resolve_param_str(s, params)
                }
                Some(Tok::Number(n, _)) => {
                    i += 1;
                    n.to_string()
                }
                _ => "1".to_string(),
            }
        } else {
            "1".to_string()
        };
        out.push((key, val));
        if matches!(toks.get(i), Some(Tok::Sym(','))) {
            i += 1;
        }
    }
    None
}

fn skip_bracket_group(toks: &[Tok], mut i: usize) -> usize {
    if !matches!(toks.get(i), Some(Tok::Sym('['))) {
        return i;
    }
    let mut d = 1i32;
    i += 1;
    while i < toks.len() && d > 0 {
        match toks.get(i) {
            Some(Tok::Sym('[')) => d += 1,
            Some(Tok::Sym(']')) => d -= 1,
            _ => {}
        }
        i += 1;
    }
    i
}

/// Extract UG901 attributes, enum state definitions, and signal types from `rtl.toks`.
pub(crate) fn extract_ug901_attrs(rtl: &Rtl) -> Ug901ModuleAttrs {
    let mut res = Ug901ModuleAttrs::default();
    let toks = &rtl.toks;
    let mut i = 0usize;
    let mut pending = SignalUg901Attrs::default();
    let mut before_module = true;

    while i < toks.len() {
        if let Some((kvs, next_i)) = parse_attr_block_at(toks, i, &rtl.params) {
            for (k, v) in kvs {
                apply_kv_to_attrs(&mut pending, &k, &v);
            }
            i = next_i;
            continue;
        }

        match toks.get(i) {
            Some(Tok::Kw(k)) if *k == "module" => {
                if !pending.is_empty() {
                    res.module_defaults.merge_from(&pending);
                    pending = SignalUg901Attrs::default();
                }
                before_module = false;
                i += 1;
                // Advance past module header `;` while also scanning port attributes inside `(...)`.
                while i < toks.len() {
                    if let Some((kvs, next_i)) = parse_attr_block_at(toks, i, &rtl.params) {
                        for (pk, pv) in kvs {
                            apply_kv_to_attrs(&mut pending, &pk, &pv);
                        }
                        i = next_i;
                        continue;
                    }
                    if matches!(toks.get(i), Some(Tok::Kw(pk)) if matches!(*pk, "input" | "output" | "inout")) {
                        i += 1;
                        while i < toks.len() {
                            if matches!(
                                toks.get(i),
                                Some(Tok::Kw(q))
                                    if matches!(*q, "wire" | "reg" | "logic" | "signed" | "unsigned" | "var" | "bit")
                            ) {
                                i += 1;
                                continue;
                            }
                            if matches!(toks.get(i), Some(Tok::Sym('['))) {
                                i = skip_bracket_group(toks, i);
                                continue;
                            }
                            break;
                        }
                        if let Some(Tok::Ident(pname)) = toks.get(i) {
                            if !pending.is_empty() {
                                res.per_signal
                                    .entry((*pname).to_string())
                                    .or_default()
                                    .merge_from(&pending);
                            }
                        }
                        pending = SignalUg901Attrs::default();
                    }
                    if matches!(toks.get(i), Some(Tok::Sym(';'))) {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                pending = SignalUg901Attrs::default();
            }
            Some(Tok::Kw(k)) if *k == "typedef" => {
                i += 1;
                if matches!(toks.get(i), Some(Tok::Kw(ek)) if *ek == "enum") {
                    i += 1;
                    while i < toks.len() && !matches!(toks.get(i), Some(Tok::Sym('{'))) {
                        i += 1;
                    }
                    if matches!(toks.get(i), Some(Tok::Sym('{'))) {
                        i += 1;
                        let mut members = Vec::new();
                        let mut next_val = 0u128;
                        while i < toks.len() && !matches!(toks.get(i), Some(Tok::Sym('}'))) {
                            if let Some(Tok::Ident(mname)) = toks.get(i) {
                                let mname_s = (*mname).to_string();
                                i += 1;
                                let val = if matches!(toks.get(i), Some(Tok::Sym('='))) {
                                    i += 1;
                                    match toks.get(i) {
                                        Some(Tok::Number(v, _)) => {
                                            i += 1;
                                            *v
                                        }
                                        _ => next_val,
                                    }
                                } else {
                                    next_val
                                };
                                members.push((mname_s, val));
                                next_val = val.wrapping_add(1);
                            } else {
                                i += 1;
                            }
                        }
                        if matches!(toks.get(i), Some(Tok::Sym('}'))) {
                            i += 1;
                        }
                        if let Some(Tok::Ident(tname)) = toks.get(i) {
                            let tname_s = (*tname).to_string();
                            res.enum_types.insert(tname_s.clone(), members);
                            if !pending.is_empty() {
                                res.per_signal
                                    .entry(tname_s)
                                    .or_default()
                                    .merge_from(&pending);
                            }
                            i += 1;
                        }
                    }
                }
                pending = SignalUg901Attrs::default();
            }
            Some(Tok::Kw(k))
                if matches!(
                    *k,
                    "logic" | "wire" | "reg" | "bit" | "input" | "output" | "inout"
                ) =>
            {
                let is_port_kw = matches!(*k, "input" | "output" | "inout");
                let mut has_reg_or_logic = matches!(*k, "logic" | "wire" | "reg" | "bit");
                i += 1;
                while i < toks.len() {
                    if let Some(Tok::Kw(q)) = toks.get(i) {
                        if matches!(*q, "logic" | "wire" | "reg" | "bit") {
                            has_reg_or_logic = true;
                            i += 1;
                            continue;
                        }
                        if matches!(*q, "signed" | "unsigned" | "var") {
                            i += 1;
                            continue;
                        }
                    }
                    if matches!(toks.get(i), Some(Tok::Sym('['))) {
                        i = skip_bracket_group(toks, i);
                        continue;
                    }
                    break;
                }
                // Keep `ram_style` off plain `input` ports (per `ram_style_pending_does_not_stick_past_input_port`).
                let mut decl_attrs = pending.clone();
                if is_port_kw && !has_reg_or_logic {
                    decl_attrs.ram_style = None;
                }
                while let Some(Tok::Ident(sname)) = toks.get(i) {
                    if !decl_attrs.is_empty() && !(is_port_kw && *k == "input") {
                        res.per_signal
                            .entry((*sname).to_string())
                            .or_default()
                            .merge_from(&decl_attrs);
                    }
                    i += 1;
                    while matches!(toks.get(i), Some(Tok::Sym('['))) {
                        i = skip_bracket_group(toks, i);
                    }
                    if matches!(toks.get(i), Some(Tok::Sym('='))) {
                        i += 1;
                        let mut d = 0i32;
                        while i < toks.len() {
                            match toks.get(i) {
                                Some(Tok::Sym('(' | '[' | '{')) => d += 1,
                                Some(Tok::Sym(')' | ']' | '}')) => d = (d - 1).max(0),
                                Some(Tok::Sym(',' | ';')) if d == 0 => break,
                                _ => {}
                            }
                            i += 1;
                        }
                    }
                    if matches!(toks.get(i), Some(Tok::Sym(','))) {
                        i += 1;
                    } else {
                        break;
                    }
                }
                pending = SignalUg901Attrs::default();
            }
            Some(Tok::Kw(k)) if *k == "assign" => {
                i += 1;
                if let Some(Tok::Ident(lhs)) = toks.get(i) {
                    if !pending.is_empty() {
                        res.per_signal
                            .entry((*lhs).to_string())
                            .or_default()
                            .merge_from(&pending);
                    }
                }
                pending = SignalUg901Attrs::default();
            }
            Some(Tok::Kw(k))
                if matches!(*k, "always" | "always_ff" | "always_comb" | "always_latch") =>
            {
                // Attributes on an `always` block apply only to LHS signals assigned inside
                // that `always` block (and never `ram_style` on an empty block).
                let mut blk_attrs = pending.clone();
                blk_attrs.ram_style = None;
                pending = SignalUg901Attrs::default();
                i += 1;
                // Skip sensitivity `@(...)` or `@*`.
                if matches!(toks.get(i), Some(Tok::Sym('@'))) {
                    i += 1;
                    if matches!(toks.get(i), Some(Tok::Sym('*'))) {
                        i += 1;
                    } else if matches!(toks.get(i), Some(Tok::Sym('('))) {
                        let mut d = 1i32;
                        i += 1;
                        while i < toks.len() && d > 0 {
                            match toks.get(i) {
                                Some(Tok::Sym('(')) => d += 1,
                                Some(Tok::Sym(')')) => d -= 1,
                                _ => {}
                            }
                            i += 1;
                        }
                    }
                }
                if !blk_attrs.is_empty() {
                    let body_start = i;
                    let body_end = if matches!(toks.get(i), Some(Tok::Kw(bk)) if *bk == "begin") {
                        let mut d = 1i32;
                        let mut j = i + 1;
                        while j < toks.len() && d > 0 {
                            match toks.get(j) {
                                Some(Tok::Kw(kw)) if *kw == "begin" => d += 1,
                                Some(Tok::Kw(kw)) if *kw == "end" => d -= 1,
                                _ => {}
                            }
                            j += 1;
                        }
                        j
                    } else {
                        let mut j = i;
                        while j < toks.len() && !matches!(toks.get(j), Some(Tok::Sym(';'))) {
                            j += 1;
                        }
                        (j + 1).min(toks.len())
                    };
                    for idx in body_start..body_end {
                        if matches!(toks.get(idx), Some(Tok::Le) | Some(Tok::Sym('='))) {
                            // Walk back before optional `[...]` to find LHS ident.
                            let mut back = idx;
                            while back > body_start && matches!(toks.get(back - 1), Some(Tok::Sym(']'))) {
                                let mut d = 1i32;
                                back -= 1;
                                while back > body_start && d > 0 {
                                    back -= 1;
                                    match toks.get(back) {
                                        Some(Tok::Sym(']')) => d += 1,
                                        Some(Tok::Sym('[')) => d -= 1,
                                        _ => {}
                                    }
                                }
                            }
                            if back > body_start {
                                if let Some(Tok::Ident(lhs_id)) = toks.get(back - 1) {
                                    res.per_signal
                                        .entry((*lhs_id).to_string())
                                        .or_default()
                                        .merge_from(&blk_attrs);
                                }
                            }
                        }
                    }
                }
            }
            Some(Tok::Ident(first_id)) if !before_module => {
                // Typedef-typed declaration: `state_t state_q, state_d;`
                let ty_name = (*first_id).to_string();
                if res.enum_types.contains_key(&ty_name)
                    && matches!(toks.get(i + 1), Some(Tok::Ident(_)))
                {
                    i += 1;
                    while let Some(Tok::Ident(sname)) = toks.get(i) {
                        res.signal_types
                            .insert((*sname).to_string(), ty_name.clone());
                        if !pending.is_empty() {
                            res.per_signal
                                .entry((*sname).to_string())
                                .or_default()
                                .merge_from(&pending);
                        }
                        i += 1;
                        if matches!(toks.get(i), Some(Tok::Sym(','))) {
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    pending = SignalUg901Attrs::default();
                    continue;
                }
                pending = SignalUg901Attrs::default();
                i += 1;
            }
            _ => {
                if !pending.is_empty() && !before_module {
                    pending = SignalUg901Attrs::default();
                }
                i += 1;
            }
        }
    }
    res
}

// ============================================================================
// Domain 4: FSM Encoding (`one_hot|gray|sequential|johnson|auto|none`) & Safe-State
// ============================================================================

fn johnson_code(idx: usize, width: usize) -> u128 {
    let w = width.max(1).min(64);
    let period = 2 * w;
    let k = idx % period;
    if k == 0 {
        0
    } else if k <= w {
        (1u128 << k) - 1
    } else {
        let shift = k - w;
        let full = (1u128 << w) - 1;
        let low = (1u128 << shift) - 1;
        full & !low
    }
}

fn collect_state_consts_from_rexpr(e: &RExpr<'_>, target_sig: &str, out: &mut Vec<u128>) {
    match e {
        RExpr::Eq(a, b) | RExpr::Ne(a, b) => match (*a, *b) {
            (RExpr::Ident(s), RExpr::Const { val, .. })
            | (RExpr::Const { val, .. }, RExpr::Ident(s))
                if *s == target_sig =>
            {
                if !out.contains(val) {
                    out.push(*val);
                }
            }
            _ => {
                collect_state_consts_from_rexpr(a, target_sig, out);
                collect_state_consts_from_rexpr(b, target_sig, out);
            }
        },
        RExpr::Mux(c, t, f) => {
            collect_state_consts_from_rexpr(c, target_sig, out);
            collect_state_consts_from_rexpr(t, target_sig, out);
            collect_state_consts_from_rexpr(f, target_sig, out);
        }
        RExpr::And(a, b)
        | RExpr::Or(a, b)
        | RExpr::Xor(a, b)
        | RExpr::Add(a, b)
        | RExpr::Sub(a, b)
        | RExpr::Mul(a, b)
        | RExpr::Shl(a, b)
        | RExpr::Shr(a, b)
        | RExpr::Ashr(a, b)
        | RExpr::Lt(a, b) => {
            collect_state_consts_from_rexpr(a, target_sig, out);
            collect_state_consts_from_rexpr(b, target_sig, out);
        }
        RExpr::Not(x) | RExpr::RedAnd(x) | RExpr::RedOr(x) | RExpr::RedXor(x) => {
            collect_state_consts_from_rexpr(x, target_sig, out);
        }
        RExpr::Concat(parts) => {
            for p in parts {
                collect_state_consts_from_rexpr(p, target_sig, out);
            }
        }
        _ => {}
    }
}

fn collect_leaf_consts(e: &RExpr<'_>, out: &mut Vec<u128>) {
    match e {
        RExpr::Const { val, .. } => {
            if !out.contains(val) {
                out.push(*val);
            }
        }
        RExpr::Mux(_, t, f) => {
            collect_leaf_consts(t, out);
            collect_leaf_consts(f, out);
        }
        _ => {}
    }
}

fn remap_fsm_rhs<'a>(
    e: &RExpr<'a>,
    code_map: &HashMap<u128, u128>,
    new_w: usize,
    state_sigs: &HashSet<String>,
    arena: &'a Bump,
) -> RExpr<'a> {
    match e {
        RExpr::Const { val, .. } => {
            let mapped = code_map.get(val).copied().unwrap_or(*val);
            RExpr::Const {
                val: mapped,
                width: new_w,
                care: care_mask(new_w),
            }
        }
        RExpr::Mux(c, t, f) => {
            let nc = remap_fsm_cond(c, code_map, new_w, state_sigs, arena);
            let nt = remap_fsm_rhs(t, code_map, new_w, state_sigs, arena);
            let nf = remap_fsm_rhs(f, code_map, new_w, state_sigs, arena);
            RExpr::Mux(arena.alloc(nc), arena.alloc(nt), arena.alloc(nf))
        }
        other => remap_fsm_cond(other, code_map, new_w, state_sigs, arena),
    }
}

fn remap_fsm_cond<'a>(
    e: &RExpr<'a>,
    code_map: &HashMap<u128, u128>,
    new_w: usize,
    state_sigs: &HashSet<String>,
    arena: &'a Bump,
) -> RExpr<'a> {
    match e {
        RExpr::Eq(a, b) => match (*a, *b) {
            (RExpr::Ident(s), RExpr::Const { val, .. }) if state_sigs.contains(*s) => {
                let mapped = code_map.get(val).copied().unwrap_or(*val);
                RExpr::Eq(
                    arena.alloc(RExpr::Ident(s)),
                    arena.alloc(RExpr::Const {
                        val: mapped,
                        width: new_w,
                        care: care_mask(new_w),
                    }),
                )
            }
            (RExpr::Const { val, .. }, RExpr::Ident(s)) if state_sigs.contains(*s) => {
                let mapped = code_map.get(val).copied().unwrap_or(*val);
                RExpr::Eq(
                    arena.alloc(RExpr::Const {
                        val: mapped,
                        width: new_w,
                        care: care_mask(new_w),
                    }),
                    arena.alloc(RExpr::Ident(s)),
                )
            }
            _ => RExpr::Eq(
                arena.alloc(remap_fsm_cond(a, code_map, new_w, state_sigs, arena)),
                arena.alloc(remap_fsm_cond(b, code_map, new_w, state_sigs, arena)),
            ),
        },
        RExpr::Ne(a, b) => match (*a, *b) {
            (RExpr::Ident(s), RExpr::Const { val, .. }) if state_sigs.contains(*s) => {
                let mapped = code_map.get(val).copied().unwrap_or(*val);
                RExpr::Ne(
                    arena.alloc(RExpr::Ident(s)),
                    arena.alloc(RExpr::Const {
                        val: mapped,
                        width: new_w,
                        care: care_mask(new_w),
                    }),
                )
            }
            (RExpr::Const { val, .. }, RExpr::Ident(s)) if state_sigs.contains(*s) => {
                let mapped = code_map.get(val).copied().unwrap_or(*val);
                RExpr::Ne(
                    arena.alloc(RExpr::Const {
                        val: mapped,
                        width: new_w,
                        care: care_mask(new_w),
                    }),
                    arena.alloc(RExpr::Ident(s)),
                )
            }
            _ => RExpr::Ne(
                arena.alloc(remap_fsm_cond(a, code_map, new_w, state_sigs, arena)),
                arena.alloc(remap_fsm_cond(b, code_map, new_w, state_sigs, arena)),
            ),
        },
        RExpr::Mux(c, t, f) => RExpr::Mux(
            arena.alloc(remap_fsm_cond(c, code_map, new_w, state_sigs, arena)),
            arena.alloc(remap_fsm_cond(t, code_map, new_w, state_sigs, arena)),
            arena.alloc(remap_fsm_cond(f, code_map, new_w, state_sigs, arena)),
        ),
        RExpr::And(a, b) => RExpr::And(
            arena.alloc(remap_fsm_cond(a, code_map, new_w, state_sigs, arena)),
            arena.alloc(remap_fsm_cond(b, code_map, new_w, state_sigs, arena)),
        ),
        RExpr::Or(a, b) => RExpr::Or(
            arena.alloc(remap_fsm_cond(a, code_map, new_w, state_sigs, arena)),
            arena.alloc(remap_fsm_cond(b, code_map, new_w, state_sigs, arena)),
        ),
        RExpr::Xor(a, b) => RExpr::Xor(
            arena.alloc(remap_fsm_cond(a, code_map, new_w, state_sigs, arena)),
            arena.alloc(remap_fsm_cond(b, code_map, new_w, state_sigs, arena)),
        ),
        RExpr::Not(x) => RExpr::Not(arena.alloc(remap_fsm_cond(
            x,
            code_map,
            new_w,
            state_sigs,
            arena,
        ))),
        other => other.clone(),
    }
}

fn build_valid_state_expr<'a>(
    state_sig: &'a str,
    valid_codes: &[u128],
    width: usize,
    arena: &'a Bump,
) -> RExpr<'a> {
    let mut acc: Option<RExpr<'a>> = None;
    for &code in valid_codes {
        let eq = RExpr::Eq(
            arena.alloc(RExpr::Ident(state_sig)),
            arena.alloc(RExpr::Const {
                val: code,
                width,
                care: care_mask(width),
            }),
        );
        acc = Some(match acc {
            None => eq,
            Some(prev) => RExpr::Or(arena.alloc(prev), arena.alloc(eq)),
        });
    }
    acc.unwrap_or(RExpr::Const {
        val: 1,
        width: 1,
        care: 1,
    })
}

fn inject_safe_state_mux<'a>(
    rhs: &RExpr<'a>,
    state_sig: &'a str,
    valid_codes: &[u128],
    safe_code: u128,
    width: usize,
    arena: &'a Bump,
) -> RExpr<'a> {
    let valid_cond = build_valid_state_expr(state_sig, valid_codes, width, arena);
    let safe_const = RExpr::Const {
        val: safe_code,
        width,
        care: care_mask(width),
    };
    // If the outer node is `if (rst) state <= RESET else next_body`, wrap `next_body`.
    if let RExpr::Mux(c, t, f) = rhs {
        if matches!(*t, RExpr::Const { .. }) && !matches!(*f, RExpr::Const { .. }) {
            let safe_f = RExpr::Mux(
                arena.alloc(valid_cond),
                arena.alloc((*f).clone()),
                arena.alloc(safe_const),
            );
            return RExpr::Mux(c, t, arena.alloc(safe_f));
        }
        if matches!(*f, RExpr::Const { .. }) && !matches!(*t, RExpr::Const { .. }) {
            let safe_t = RExpr::Mux(
                arena.alloc(valid_cond),
                arena.alloc((*t).clone()),
                arena.alloc(safe_const),
            );
            return RExpr::Mux(c, arena.alloc(safe_t), f);
        }
    }
    RExpr::Mux(
        arena.alloc(valid_cond),
        arena.alloc(rhs.clone()),
        arena.alloc(safe_const),
    )
}

pub(crate) fn apply_ug901_fsm_pass<'a>(
    rtl: &mut Rtl<'a>,
    d: &mut Design,
    ug: &Ug901ModuleAttrs,
    arena: &'a Bump,
) -> Vec<(String, Option<FsmEncodingMode>, Option<FsmSafeStateMode>)> {
    let mut modified_fsms = Vec::new();
    let nba_signals: Vec<&'a str> = rtl
        .nbas
        .iter()
        .filter(|(lhs, bit, _)| matches!(bit, None | Some(0)) && sig_depth(rtl, lhs) == 0)
        .map(|(lhs, _, _)| *lhs)
        .collect();

    for state_sig in nba_signals {
        let eff = ug.effective_for(state_sig);
        if eff.fsm_encoding.is_none() && eff.fsm_safe_state.is_none() {
            continue;
        }
        if modified_fsms
            .iter()
            .any(|(s, _, _): &(String, _, _)| s == state_sig)
        {
            continue;
        }

        // Collect original state values.
        let mut orig_states: Vec<u128> = Vec::new();
        if let Some(ty) = ug.signal_types.get(state_sig) {
            if let Some(members) = ug.enum_types.get(ty) {
                for (_, val) in members {
                    if !orig_states.contains(val) {
                        orig_states.push(*val);
                    }
                }
            }
        }
        for (_, _, rhs) in rtl.nbas.iter().chain(rtl.assigns.iter()) {
            collect_state_consts_from_rexpr(rhs, state_sig, &mut orig_states);
        }
        for (lhs, _, rhs) in &rtl.nbas {
            if *lhs == state_sig {
                collect_leaf_consts(rhs, &mut orig_states);
            }
        }
        // Check if `state_sig` is driven by a companion combinational `next_state` signal.
        let mut companion_next: Option<&'a str> = None;
        for (lhs, _, rhs) in &rtl.nbas {
            if *lhs == state_sig {
                match rhs {
                    RExpr::Ident(ns) if *ns != state_sig => companion_next = Some(*ns),
                    RExpr::Mux(_, t, f) => match (*t, *f) {
                        (RExpr::Const { .. }, RExpr::Ident(ns)) if *ns != state_sig => {
                            companion_next = Some(*ns);
                        }
                        (RExpr::Ident(ns), RExpr::Const { .. }) if *ns != state_sig => {
                            companion_next = Some(*ns);
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        if let Some(ns) = companion_next {
            for (lhs, _, rhs) in &rtl.assigns {
                if *lhs == ns {
                    collect_leaf_consts(rhs, &mut orig_states);
                }
            }
        }
        if orig_states.len() < 2 {
            continue;
        }
        orig_states.sort_unstable();
        orig_states.dedup();
        let n_states = orig_states.len();
        let orig_w = sig_width(rtl, state_sig).max(1);
        let enc_mode = eff.fsm_encoding.unwrap_or(FsmEncodingMode::Sequential);
        let effective_enc = match enc_mode {
            FsmEncodingMode::Auto => {
                if n_states >= 5 {
                    FsmEncodingMode::OneHot
                } else {
                    FsmEncodingMode::Gray
                }
            }
            other => other,
        };
        let new_w = match effective_enc {
            FsmEncodingMode::OneHot => n_states.max(2),
            FsmEncodingMode::Johnson => ((n_states + 1) / 2).max(orig_w).max(2),
            FsmEncodingMode::Gray | FsmEncodingMode::Sequential => {
                (clog2_u(n_states as u128) as usize).max(orig_w).max(1)
            }
            FsmEncodingMode::None | FsmEncodingMode::Auto => orig_w,
        };

        let mut code_map: HashMap<u128, u128> = HashMap::new();
        let mut valid_codes: Vec<u128> = Vec::with_capacity(n_states);
        for (idx, &orig_val) in orig_states.iter().enumerate() {
            let new_code = match effective_enc {
                FsmEncodingMode::OneHot => 1u128 << idx,
                FsmEncodingMode::Gray => (idx ^ (idx >> 1)) as u128,
                FsmEncodingMode::Sequential => idx as u128,
                FsmEncodingMode::Johnson => johnson_code(idx, new_w),
                FsmEncodingMode::None | FsmEncodingMode::Auto => orig_val,
            };
            code_map.insert(orig_val, new_code);
            valid_codes.push(new_code);
        }

        let mut state_group: HashSet<String> = HashSet::new();
        state_group.insert(state_sig.to_string());
        if let Some(ns) = companion_next {
            state_group.insert(ns.to_string());
        }

        // Update signal and port widths for `state_sig` (and `companion_next`).
        for sig in rtl.signals.iter_mut() {
            if state_group.contains(sig.name) {
                sig.width = new_w;
            }
        }
        for (pname, _, pw) in rtl.ports.iter_mut() {
            if state_group.contains(*pname) {
                *pw = new_w;
            }
        }

        // Collapse any `normalize_nbas`-split `(state_sig, Some(i), ...)` entries back to
        // a single whole-word `(state_sig, None, remapped_rhs)` so `synth_rtl_inner`
        // emits `new_w` flip-flops (e.g. N flip-flops for one-hot).
        let mut state_rhs_0: Option<RExpr<'a>> = None;
        for (lhs, bit, rhs) in &rtl.nbas {
            if *lhs == state_sig && matches!(bit, None | Some(0)) {
                state_rhs_0 = Some(rhs.clone());
                break;
            }
        }
        let safe_code = valid_codes.first().copied().unwrap_or(0);
        if let Some(raw_s_rhs) = state_rhs_0 {
            let mut new_s_rhs = remap_fsm_rhs(&raw_s_rhs, &code_map, new_w, &state_group, arena);
            if eff.fsm_safe_state.is_some() && companion_next.is_none() {
                new_s_rhs = inject_safe_state_mux(
                    &new_s_rhs,
                    state_sig,
                    &valid_codes,
                    safe_code,
                    new_w,
                    arena,
                );
            }
            let mut replaced = false;
            rtl.nbas.retain_mut(|(lhs, bit, rhs)| {
                if *lhs == state_sig {
                    if !replaced {
                        *bit = None;
                        *rhs = new_s_rhs.clone();
                        replaced = true;
                        true
                    } else {
                        false
                    }
                } else {
                    *rhs = remap_fsm_cond(rhs, &code_map, new_w, &state_group, arena);
                    true
                }
            });
        }
        for (lhs, _, rhs) in rtl.assigns.iter_mut() {
            if state_group.contains(*lhs) {
                *rhs = remap_fsm_rhs(rhs, &code_map, new_w, &state_group, arena);
            } else {
                *rhs = remap_fsm_cond(rhs, &code_map, new_w, &state_group, arena);
            }
        }

        if let Some(safe_mode) = eff.fsm_safe_state {
            if let Some(ns) = companion_next {
                for (lhs, bit, rhs) in rtl.assigns.iter_mut() {
                    if *lhs == ns && matches!(bit, None | Some(0)) {
                        *rhs = inject_safe_state_mux(
                            rhs,
                            state_sig,
                            &valid_codes,
                            safe_code,
                            new_w,
                            arena,
                        );
                    }
                }
            }
            d.attrs.set("FSM_SAFE_STATE", safe_mode.as_attr_str());
        }

        if let Some(enc) = eff.fsm_encoding {
            d.attrs.set("FSM_ENCODING", enc.as_attr_str());
        }
        modified_fsms.push((state_sig.to_string(), eff.fsm_encoding, eff.fsm_safe_state));
    }
    modified_fsms
}

pub(crate) fn tag_fsm_state_cells(
    d: &mut Design,
    fsms: &[(String, Option<FsmEncodingMode>, Option<FsmSafeStateMode>)],
) {
    if fsms.is_empty() {
        return;
    }
    let pins = d.pin_index();
    let mut cell_tags: Vec<(String, Option<&'static str>, Option<&'static str>)> = Vec::new();
    for cell in &d.cells {
        if !matches!(cell.kind, CellKind::Hff) {
            continue;
        }
        let cname = resolve(cell.name);
        let Some(qnet) = pins.net_on(cname, "Q") else {
            continue;
        };
        let qstr = resolve(qnet);
        for (state_sig, enc, safe) in fsms {
            if qstr == state_sig
                || qstr
                    .strip_prefix(state_sig.as_str())
                    .is_some_and(|rest| rest.starts_with('_'))
            {
                cell_tags.push((
                    cname.to_string(),
                    enc.map(|e| e.as_attr_str()),
                    safe.map(|s| s.as_attr_str()),
                ));
            }
        }
    }
    for (cname, enc, safe) in cell_tags {
        if let Some(e) = enc {
            let _ = d.set_cell_attr(&cname, "FSM_ENCODING", e);
        }
        if let Some(s) = safe {
            let _ = d.set_cell_attr(&cname, "FSM_SAFE_STATE", s);
        }
    }
}

// ============================================================================
// Domain 1: DSP48E1 Inference (`use_dsp = "yes|no|simd|logic"`, MREG/PREG, PCOUT->PCIN)
// ============================================================================

fn lut6_madd_inits() -> (u64, u64) {
    let mut sum_init = 0u64;
    let mut cout_init = 0u64;
    for addr in 0..64u64 {
        let i0 = (addr & 1) != 0;
        let i1 = ((addr >> 1) & 1) != 0;
        let i2 = ((addr >> 2) & 1) != 0;
        let i3 = ((addr >> 3) & 1) != 0;
        let p = i0 && i1;
        let s = p ^ i2 ^ i3;
        let c = (p && i2) || (p && i3) || (i2 && i3);
        if s {
            sum_init |= 1u64 << addr;
        }
        if c {
            cout_init |= 1u64 << addr;
        }
    }
    (sum_init, cout_init)
}

/// Emit a hardware array multiplier of 4-input LUT6 cells when `use_dsp = "no" | "logic"`.
fn emit_lut_array_multiplier(
    d: &mut Design,
    a_sig: &str,
    wa: usize,
    b_sig: &str,
    wb: usize,
    lhs: &str,
    out_w: usize,
    clk_opt: Option<&str>,
) {
    const AND2_INIT: u64 = 0x8888_8888_8888_8888;
    let (sum_init, cout_init) = lut6_madd_inits();
    let wa = wa.max(1).min(16);
    let wb = wb.max(1).min(16);
    let out_w = out_w.max(1).min(32);

    let mut row_sums: Vec<String> = Vec::with_capacity(wa + 1);
    let mut prod_nets: Vec<String> = Vec::with_capacity(out_w);

    // Row 0: partial products a[i] & b[0]
    let b0 = bit_name(b_sig, wb, 0);
    for i in 0..wa {
        let ai = bit_name(a_sig, wa, i);
        let snet = if i == 0 && clk_opt.is_none() {
            bit_name(lhs, out_w, 0)
        } else {
            format!("u_lmul_{lhs}_r0_{i}")
        };
        let cell = format!("u_lmul_pp_{lhs}_0_{i}");
        d.add_cell(&cell, CellKind::Lut6 { init: AND2_INIT });
        let _ = d.dont_touch(&cell);
        d.connect(&ai, &cell, "I0");
        d.connect(&b0, &cell, "I1");
        d.connect(&snet, &cell, "O");
        row_sums.push(snet);
    }
    prod_nets.push(row_sums[0].clone());

    // Rows 1..wb
    for j in 1..wb {
        let bj = bit_name(b_sig, wb, j);
        let mut next_row: Vec<String> = Vec::with_capacity(wa + 1);
        let mut cin_net: Option<String> = None;
        for i in 0..wa {
            let ai = bit_name(a_sig, wa, i);
            let sin_opt = row_sums.get(i + 1).cloned();
            let is_out_bit = i == 0 && prod_nets.len() < out_w;
            let snet = if is_out_bit && clk_opt.is_none() {
                bit_name(lhs, out_w, prod_nets.len())
            } else {
                format!("u_lmul_{lhs}_s_{j}_{i}")
            };
            let scell = format!("u_lmul_sum_{lhs}_{j}_{i}");
            d.add_cell(&scell, CellKind::Lut6 { init: sum_init });
            let _ = d.dont_touch(&scell);
            d.connect(&ai, &scell, "I0");
            d.connect(&bj, &scell, "I1");
            if let Some(sin) = &sin_opt {
                d.connect(sin, &scell, "I2");
            }
            if let Some(cin) = &cin_net {
                d.connect(cin, &scell, "I3");
            }
            d.connect(&snet, &scell, "O");
            next_row.push(snet.clone());
            if is_out_bit {
                prod_nets.push(snet);
            }

            let cnet = format!("u_lmul_{lhs}_c_{j}_{i}");
            let ccell = format!("u_lmul_cout_{lhs}_{j}_{i}");
            d.add_cell(&ccell, CellKind::Lut6 { init: cout_init });
            let _ = d.dont_touch(&ccell);
            d.connect(&ai, &ccell, "I0");
            d.connect(&bj, &ccell, "I1");
            if let Some(sin) = &sin_opt {
                d.connect(sin, &ccell, "I2");
            }
            if let Some(cin) = &cin_net {
                d.connect(cin, &ccell, "I3");
            }
            d.connect(&cnet, &ccell, "O");
            cin_net = Some(cnet);
        }
        if let Some(last_c) = cin_net {
            next_row.push(last_c);
        }
        row_sums = next_row;
    }
    for k in 1..row_sums.len() {
        if prod_nets.len() >= out_w {
            break;
        }
        let bit_idx = prod_nets.len();
        let src = &row_sums[k];
        if clk_opt.is_none() {
            let dst = bit_name(lhs, out_w, bit_idx);
            let bcell = format!("u_lmul_out_{lhs}_{bit_idx}");
            d.add_cell(&bcell, CellKind::Lut6 { init: 0xAAAA_AAAA_AAAA_AAAA });
            let _ = d.dont_touch(&bcell);
            d.connect(src, &bcell, "I0");
            d.connect(&dst, &bcell, "O");
            prod_nets.push(dst);
        } else {
            prod_nets.push(src.clone());
        }
    }
    if let Some(clk) = clk_opt {
        for (k, pnet) in prod_nets.iter().take(out_w).enumerate() {
            let qnet = bit_name(lhs, out_w, k);
            let ff = format!("u_lmul_ff_{lhs}_{k}");
            d.add_cell(&ff, CellKind::Hff);
            d.connect(clk, &ff, "CLK");
            d.connect(pnet, &ff, "D");
            d.connect(&qnet, &ff, "Q");
        }
    }
}

fn peel_sync_reset_or_en<'a, 'e>(rhs: &'e RExpr<'a>, lhs: &str) -> &'e RExpr<'a> {
    let mut cur = rhs;
    while let RExpr::Mux(_, t, f) = cur {
        if matches!(*t, RExpr::Const { val: 0, .. }) {
            cur = f;
        } else if matches!(*f, RExpr::Const { val: 0, .. }) {
            cur = t;
        } else if matches!(*f, RExpr::Ident(s) if *s == lhs) {
            cur = t;
        } else if matches!(*t, RExpr::Ident(s) if *s == lhs) {
            cur = f;
        } else {
            break;
        }
    }
    cur
}

fn extract_mul_pair<'a>(e: &RExpr<'a>) -> Option<(&'a str, &'a str)> {
    match e {
        RExpr::Mul(a, b) => {
            let an = match *a {
                RExpr::Ident(s) | RExpr::Range(s, _, _) => s,
                _ => return None,
            };
            let bn = match *b {
                RExpr::Ident(s) | RExpr::Range(s, _, _) => s,
                _ => return None,
            };
            Some((an, bn))
        }
        _ => None,
    }
}

fn extract_add_sub_operands<'a>(e: &RExpr<'a>) -> Option<(&'a str, &'a str, Option<&'a str>, bool)> {
    match e {
        RExpr::Add(l, r) => {
            if let RExpr::Add(ll, lr) = *l {
                if let (RExpr::Ident(a), RExpr::Ident(b), RExpr::Ident(c)) = (*ll, *lr, *r) {
                    return Some((a, b, Some(c), false));
                }
            }
            let a = match *l {
                RExpr::Ident(s) | RExpr::Range(s, _, _) => s,
                _ => return None,
            };
            let b = match *r {
                RExpr::Ident(s) | RExpr::Range(s, _, _) => s,
                _ => return None,
            };
            Some((a, b, None, false))
        }
        RExpr::Sub(l, r) => {
            let a = match *l {
                RExpr::Ident(s) | RExpr::Range(s, _, _) => s,
                _ => return None,
            };
            let b = match *r {
                RExpr::Ident(s) | RExpr::Range(s, _, _) => s,
                _ => return None,
            };
            Some((a, b, None, true))
        }
        _ => None,
    }
}

pub(crate) fn apply_ug901_dsp_pass<'a>(
    rtl: &mut Rtl<'a>,
    d: &mut Design,
    clk: &str,
    ug: &Ug901ModuleAttrs,
    arena: &'a Bump,
    n_mac: &mut usize,
) {
    // 1. Handle `use_dsp = "no"` and `use_dsp = "logic"`: emit a LUT6 array multiplier
    //    directly into `d` so `Dsp48e1` is inhibited without tripping `wide_cone`.
    let mut wmap: HashMap<&'a str, usize> = HashMap::new();
    for s in &rtl.signals {
        wmap.insert(s.name, s.width.max(1));
    }
    for (pn, _, pw) in &rtl.ports {
        wmap.entry(*pn).or_insert((*pw).max(1));
    }
    let mut no_dsp_nbas: HashSet<&'a str> = HashSet::new();
    for (lhs, bit, rhs) in &rtl.nbas {
        if !matches!(bit, None | Some(0)) {
            continue;
        }
        let mode = ug.effective_for(lhs).use_dsp;
        if matches!(mode, Some(UseDspMode::No | UseDspMode::Logic)) {
            let core = peel_sync_reset_or_en(rhs, lhs);
            let norm = normalize_mul_operands(core, arena);
            if let Some((a, b)) = extract_mul_pair(&norm) {
                let wa = wmap.get(a).copied().unwrap_or(4);
                let wb = wmap.get(b).copied().unwrap_or(4);
                let out_w = wmap.get(lhs).copied().unwrap_or(wa + wb);
                emit_lut_array_multiplier(d, a, wa, b, wb, lhs, out_w, Some(clk));
                let tag = if mode == Some(UseDspMode::Logic) {
                    "LOGIC"
                } else {
                    "NO"
                };
                d.attrs.set("USE_DSP", tag);
                no_dsp_nbas.insert(*lhs);
            }
        }
    }
    if !no_dsp_nbas.is_empty() {
        rtl.nbas.retain(|(lhs, _, _)| !no_dsp_nbas.contains(lhs));
    }
    let mut no_dsp_assigns: HashSet<&'a str> = HashSet::new();
    for (lhs, bit, rhs) in &rtl.assigns {
        if bit.is_some() {
            continue;
        }
        let mode = ug.effective_for(lhs).use_dsp;
        if matches!(mode, Some(UseDspMode::No | UseDspMode::Logic)) {
            let norm = normalize_mul_operands(rhs, arena);
            if let Some((a, b)) = extract_mul_pair(&norm) {
                let wa = wmap.get(a).copied().unwrap_or(4);
                let wb = wmap.get(b).copied().unwrap_or(4);
                let out_w = wmap.get(lhs).copied().unwrap_or(wa + wb);
                emit_lut_array_multiplier(d, a, wa, b, wb, lhs, out_w, None);
                let tag = if mode == Some(UseDspMode::Logic) {
                    "LOGIC"
                } else {
                    "NO"
                };
                d.attrs.set("USE_DSP", tag);
                no_dsp_assigns.insert(*lhs);
            }
        }
    }
    if !no_dsp_assigns.is_empty() {
        rtl.assigns
            .retain(|(lhs, _, _)| !no_dsp_assigns.contains(lhs));
    }

    // 2. Detect simple input pipeline registers (`a_reg <= a_in`) for AREG/BREG absorption.
    let mut simple_reg_src: HashMap<&'a str, &'a str> = HashMap::new();
    for (lhs, bit, rhs) in &rtl.nbas {
        if bit.is_none() {
            let core = peel_sync_reset_or_en(rhs, lhs);
            if let RExpr::Ident(src) = core {
                if *src != *lhs {
                    simple_reg_src.insert(*lhs, *src);
                }
            }
        }
    }

    // 3. Detect `MREG` (`m_reg <= a * b`) feeding `PREG` (`p_reg <= m_reg` / `p_reg <= m_reg + c`
    //    / `p_reg <= p_reg + m_reg`) and fold them into a single pipelined `Dsp48e1` (`MREG=1, PREG=1`).
    let mut mul_regs: HashMap<&'a str, (&'a str, &'a str)> = HashMap::new();
    for (lhs, bit, rhs) in &rtl.nbas {
        if bit.is_none() && !matches!(ug.effective_for(lhs).use_dsp, Some(UseDspMode::No | UseDspMode::Logic)) {
            let core = peel_sync_reset_or_en(rhs, lhs);
            if let Some((a, b)) = extract_mul_pair(core) {
                mul_regs.insert(*lhs, (a, b));
            }
        }
    }

    let mut absorbed_nbas: HashSet<&'a str> = HashSet::new();
    let mut dsp_by_out: HashMap<String, String> = HashMap::new();

    for (lhs, bit, rhs) in &rtl.nbas {
        if bit.is_some() || mul_regs.contains_key(lhs) {
            continue;
        }
        if matches!(
            ug.effective_for(lhs).use_dsp,
            Some(UseDspMode::No | UseDspMode::Logic)
        ) {
            continue;
        }
        let core = peel_sync_reset_or_en(rhs, lhs);
        let matched: Option<(&'a str, &'a str, Option<&'a str>, bool)> = match core {
            RExpr::Ident(m) if mul_regs.contains_key(m) => {
                let (a, b) = mul_regs[m];
                absorbed_nbas.insert(*m);
                Some((a, b, None, false))
            }
            RExpr::Add(l, r) => match (*l, *r) {
                (RExpr::Ident(m), RExpr::Ident(c)) if mul_regs.contains_key(m) => {
                    let (a, b) = mul_regs[m];
                    absorbed_nbas.insert(*m);
                    Some((a, b, Some(*c), *c == *lhs))
                }
                (RExpr::Ident(c), RExpr::Ident(m)) if mul_regs.contains_key(m) => {
                    let (a, b) = mul_regs[m];
                    absorbed_nbas.insert(*m);
                    Some((a, b, Some(*c), *c == *lhs))
                }
                _ => None,
            },
            _ => None,
        };
        if let Some((raw_a, raw_b, c_opt, is_accum)) = matched {
            let (a_net, areg) = if let Some(&src) = simple_reg_src.get(raw_a) {
                absorbed_nbas.insert(raw_a);
                (src, "1")
            } else {
                (raw_a, "0")
            };
            let (b_net, breg) = if let Some(&src) = simple_reg_src.get(raw_b) {
                absorbed_nbas.insert(raw_b);
                (src, "1")
            } else {
                (raw_b, "0")
            };
            let cell = format!("u_mac{}", *n_mac);
            *n_mac += 1;
            let opmode = if is_accum { 0b0100101 } else { 0b0000101 };
            d.add_cell(
                &cell,
                CellKind::Dsp48e1 {
                    opmode,
                    alumode: 0b0000,
                },
            );
            d.connect(clk, &cell, "CLK");
            d.connect(a_net, &cell, "A");
            d.connect(b_net, &cell, "B");
            if let Some(c_net) = c_opt {
                if !is_accum {
                    d.connect(c_net, &cell, "C");
                }
            }
            d.connect(*lhs, &cell, "P");
            let _ = d.set_cell_attr(&cell, "MREG", "1");
            let _ = d.set_cell_attr(&cell, "PREG", "1");
            let _ = d.set_cell_attr(&cell, "AREG", areg);
            let _ = d.set_cell_attr(&cell, "BREG", breg);
            let _ = d.set_cell_attr(&cell, "USE_DSP", "YES");
            d.attrs.set("DSP_MREG", "1");
            d.attrs.set("DSP_PREG", "1");
            absorbed_nbas.insert(*lhs);
            dsp_by_out.insert((*lhs).to_string(), cell);
        }
    }

    // 4. Detect single-expression sum-of-products `(a0 * b0) + (a1 * b1)` (in NBAs or assigns)
    //    and emit a cascaded DSP pair connected via `PCOUT` -> `PCIN` (`opmode = 0b0010101`).
    let mut absorbed_assigns: HashSet<&'a str> = HashSet::new();
    for (lhs, bit, rhs) in &rtl.nbas {
        if bit.is_some() || absorbed_nbas.contains(lhs) {
            continue;
        }
        if matches!(
            ug.effective_for(lhs).use_dsp,
            Some(UseDspMode::No | UseDspMode::Logic)
        ) {
            continue;
        }
        let core = peel_sync_reset_or_en(rhs, lhs);
        if let RExpr::Add(l, r) = core {
            if let (Some((a0, b0)), Some((a1, b1))) = (extract_mul_pair(l), extract_mul_pair(r)) {
                let dsp0 = format!("u_mac{}", *n_mac);
                *n_mac += 1;
                let dsp1 = format!("u_mac{}", *n_mac);
                *n_mac += 1;
                let pcout_net = format!("{dsp0}_PCOUT");
                d.add_cell(
                    &dsp0,
                    CellKind::Dsp48e1 {
                        opmode: 0b0000101,
                        alumode: 0b0000,
                    },
                );
                d.connect(clk, &dsp0, "CLK");
                d.connect(a0, &dsp0, "A");
                d.connect(b0, &dsp0, "B");
                d.connect(&pcout_net, &dsp0, "PCOUT");
                d.connect(&pcout_net, &dsp0, "P");
                let _ = d.set_cell_attr(&dsp0, "MREG", "0");
                let _ = d.set_cell_attr(&dsp0, "PREG", "0");

                d.add_cell(
                    &dsp1,
                    CellKind::Dsp48e1 {
                        opmode: 0b0010101,
                        alumode: 0b0000,
                    },
                );
                d.connect(clk, &dsp1, "CLK");
                d.connect(a1, &dsp1, "A");
                d.connect(b1, &dsp1, "B");
                d.connect(&pcout_net, &dsp1, "PCIN");
                d.connect(*lhs, &dsp1, "P");
                let _ = d.set_cell_attr(&dsp1, "MREG", "0");
                let _ = d.set_cell_attr(&dsp1, "PREG", "1");
                let _ = d.set_cell_attr(&dsp1, "PCOUT_CASCADE", "1");
                d.attrs.set("DSP_CASCADE", "1");
                absorbed_nbas.insert(*lhs);
                dsp_by_out.insert((*lhs).to_string(), dsp1);
            }
        }
    }
    for (lhs, bit, rhs) in &rtl.assigns {
        if bit.is_some() || absorbed_assigns.contains(lhs) {
            continue;
        }
        if matches!(
            ug.effective_for(lhs).use_dsp,
            Some(UseDspMode::No | UseDspMode::Logic)
        ) {
            continue;
        }
        if let RExpr::Add(l, r) = rhs {
            if let (Some((a0, b0)), Some((a1, b1))) = (extract_mul_pair(l), extract_mul_pair(r)) {
                let dsp0 = format!("u_mac{}", *n_mac);
                *n_mac += 1;
                let dsp1 = format!("u_mac{}", *n_mac);
                *n_mac += 1;
                let pcout_net = format!("{dsp0}_PCOUT");
                d.add_cell(
                    &dsp0,
                    CellKind::Dsp48e1 {
                        opmode: 0b0000101,
                        alumode: 0b0000,
                    },
                );
                d.connect(a0, &dsp0, "A");
                d.connect(b0, &dsp0, "B");
                d.connect(&pcout_net, &dsp0, "PCOUT");
                d.connect(&pcout_net, &dsp0, "P");
                let _ = d.set_cell_attr(&dsp0, "MREG", "0");
                let _ = d.set_cell_attr(&dsp0, "PREG", "0");

                d.add_cell(
                    &dsp1,
                    CellKind::Dsp48e1 {
                        opmode: 0b0010101,
                        alumode: 0b0000,
                    },
                );
                d.connect(a1, &dsp1, "A");
                d.connect(b1, &dsp1, "B");
                d.connect(&pcout_net, &dsp1, "PCIN");
                d.connect(*lhs, &dsp1, "P");
                let _ = d.set_cell_attr(&dsp1, "MREG", "0");
                let _ = d.set_cell_attr(&dsp1, "PREG", "0");
                let _ = d.set_cell_attr(&dsp1, "PCOUT_CASCADE", "1");
                d.attrs.set("DSP_CASCADE", "1");
                absorbed_assigns.insert(*lhs);
                dsp_by_out.insert((*lhs).to_string(), dsp1);
            }
        }
    }

    // 5. Handle `use_dsp = "simd"` and `use_dsp = "yes"` on standalone add/sub/accum
    //    (expressions without `Mul` that were explicitly marked `use_dsp = "yes|simd"`).
    for (lhs, bit, rhs) in &rtl.nbas {
        if bit.is_some() || absorbed_nbas.contains(lhs) {
            continue;
        }
        let mode = ug.effective_for(lhs).use_dsp;
        if !matches!(mode, Some(UseDspMode::Yes | UseDspMode::Simd)) {
            continue;
        }
        let core = peel_sync_reset_or_en(rhs, lhs);
        if let Some((a, b, c_opt, is_sub)) = extract_add_sub_operands(core) {
            let cell = format!("u_mac{}", *n_mac);
            *n_mac += 1;
            let alumode = if is_sub { 0b0011 } else { 0b0000 };
            d.add_cell(
                &cell,
                CellKind::Dsp48e1 {
                    opmode: 0b0110011,
                    alumode,
                },
            );
            d.connect(clk, &cell, "CLK");
            d.connect(a, &cell, "A");
            d.connect(b, &cell, "C");
            if let Some(c) = c_opt {
                d.connect(c, &cell, "B");
            }
            d.connect(*lhs, &cell, "P");
            let _ = d.set_cell_attr(&cell, "USE_MULT", "NONE");
            let _ = d.set_cell_attr(&cell, "MREG", "0");
            let _ = d.set_cell_attr(&cell, "PREG", "1");
            if mode == Some(UseDspMode::Simd) {
                let w = sig_width(rtl, lhs);
                let simd_mode = if w <= 12 { "FOUR12" } else { "TWO24" };
                let _ = d.set_cell_attr(&cell, "USE_SIMD", simd_mode);
                let _ = d.set_cell_attr(&cell, "USE_DSP", "SIMD");
                d.attrs.set("USE_SIMD", simd_mode);
            } else {
                let _ = d.set_cell_attr(&cell, "USE_DSP", "YES");
                d.attrs.set("USE_DSP", "YES");
            }
            absorbed_nbas.insert(*lhs);
            dsp_by_out.insert((*lhs).to_string(), cell);
        }
    }
    for (lhs, bit, rhs) in &rtl.assigns {
        if bit.is_some() || absorbed_assigns.contains(lhs) {
            continue;
        }
        let mode = ug.effective_for(lhs).use_dsp;
        if !matches!(mode, Some(UseDspMode::Yes | UseDspMode::Simd)) {
            continue;
        }
        if let Some((a, b, c_opt, is_sub)) = extract_add_sub_operands(rhs) {
            let cell = format!("u_mac{}", *n_mac);
            *n_mac += 1;
            let alumode = if is_sub { 0b0011 } else { 0b0000 };
            d.add_cell(
                &cell,
                CellKind::Dsp48e1 {
                    opmode: 0b0110011,
                    alumode,
                },
            );
            d.connect(a, &cell, "A");
            d.connect(b, &cell, "C");
            if let Some(c) = c_opt {
                d.connect(c, &cell, "B");
            }
            d.connect(*lhs, &cell, "P");
            let _ = d.set_cell_attr(&cell, "USE_MULT", "NONE");
            let _ = d.set_cell_attr(&cell, "MREG", "0");
            let _ = d.set_cell_attr(&cell, "PREG", "0");
            if mode == Some(UseDspMode::Simd) {
                let w = sig_width(rtl, lhs);
                let simd_mode = if w <= 12 { "FOUR12" } else { "TWO24" };
                let _ = d.set_cell_attr(&cell, "USE_SIMD", simd_mode);
                let _ = d.set_cell_attr(&cell, "USE_DSP", "SIMD");
                d.attrs.set("USE_SIMD", simd_mode);
            } else {
                let _ = d.set_cell_attr(&cell, "USE_DSP", "YES");
                d.attrs.set("USE_DSP", "YES");
            }
            absorbed_assigns.insert(*lhs);
            dsp_by_out.insert((*lhs).to_string(), cell);
        }
    }

    rtl.nbas.retain(|(lhs, _, _)| !absorbed_nbas.contains(lhs));
    rtl.assigns
        .retain(|(lhs, _, _)| !absorbed_assigns.contains(lhs));
}

/// Post-pass on emitted `Dsp48e1` cells: wire `PCOUT` -> `PCIN` when one DSP's `C` input
/// is driven by another DSP's `P` output, and populate `MREG`/`PREG`/`CLK` on clocked DSPs.
pub(crate) fn finalize_ug901_dsp_cascades(d: &mut Design, rtl: &Rtl, clk: &str) {
    let nba_lhs: HashSet<&str> = rtl.nbas.iter().map(|(l, _, _)| *l).collect();
    let pins = d.pin_index();

    // Map net -> DSP cell that drives it via `P`.
    let mut p_driver_of_net: HashMap<String, String> = HashMap::new();
    let mut dsp_cells: Vec<String> = Vec::new();
    for cell in &d.cells {
        if matches!(cell.kind, CellKind::Dsp48e1 { .. }) {
            let cname = resolve(cell.name).to_string();
            if let Some(pnet) = pins.net_on(&cname, "P") {
                p_driver_of_net.insert(resolve(pnet).to_string(), cname.clone());
            }
            dsp_cells.push(cname);
        }
    }

    for cname in &dsp_cells {
        // Attach PREG/MREG defaults if not already set.
        let p_net = pins.net_on(cname, "P").map(|s| resolve(s).to_string());
        let is_clocked = p_net.as_deref().is_some_and(|n| nba_lhs.contains(n));
        let has_preg = d
            .cells
            .iter()
            .find(|c| resolve(c.name) == cname)
            .and_then(|c| c.attrs.get("PREG"))
            .is_some();
        if !has_preg {
            if is_clocked {
                d.connect(clk, cname, "CLK");
                let _ = d.set_cell_attr(cname, "PREG", "1");
                let _ = d.set_cell_attr(cname, "MREG", "0");
            } else {
                let _ = d.set_cell_attr(cname, "PREG", "0");
                let _ = d.set_cell_attr(cname, "MREG", "0");
            }
        }
        // Check if this DSP's `C` pin is driven by an upstream DSP's `P` output.
        if let Some(cnet) = pins.net_on(cname, "C") {
            let cnet_str = resolve(cnet).to_string();
            if let Some(up_dsp) = p_driver_of_net.get(&cnet_str) {
                if up_dsp != cname {
                    let pcout_net = format!("{up_dsp}_PCOUT");
                    d.connect(&pcout_net, up_dsp, "PCOUT");
                    d.connect(&pcout_net, cname, "PCIN");
                    if let Some(cell_mut) = d.cells.iter_mut().find(|c| resolve(c.name) == cname) {
                        if let CellKind::Dsp48e1 { opmode, .. } = &mut cell_mut.kind {
                            *opmode = 0b0010101; // Z = PCIN, X:Y = M
                        }
                    }
                    let _ = d.set_cell_attr(cname, "PCOUT_CASCADE", "1");
                    d.attrs.set("DSP_CASCADE", "1");
                }
            }
        }
    }
}

// ============================================================================
// Domain 3: `shreg_extract` & `srl_style` (`SRL16E` / `SRLC32E`) Inference
// ============================================================================

struct DetectedSrl<'a> {
    sr_name: &'a str,
    din_name: &'a str,
    depth: usize,
    width: usize,
    ce_expr: Option<RExpr<'a>>,
    out_lhs: Option<&'a str>,
    tap_addr: Option<&'a str>,
    tap_const: Option<usize>,
    is_unpacked: bool,
}

fn detect_1d_srl<'a>(rtl: &Rtl<'a>, sr_name: &'a str, arena: &'a Bump) -> Option<DetectedSrl<'a>> {
    let depth = sig_width(rtl, sr_name);
    if depth < 2 || sig_depth(rtl, sr_name) > 0 {
        return None;
    }
    let (_, bit, rhs) = rtl
        .nbas
        .iter()
        .find(|(lhs, b, _)| *lhs == sr_name && matches!(b, None | Some(0)))?;
    let _ = bit;
    let (ce_expr, core) = match rhs {
        RExpr::Mux(c, t, f) => {
            if matches!(*f, RExpr::Ident(s) if *s == sr_name) {
                (Some((**c).clone()), *t)
            } else if matches!(*t, RExpr::Ident(s) if *s == sr_name) {
                (
                    Some(RExpr::Not(arena.alloc((**c).clone()))),
                    *f,
                )
            } else {
                // Has reset mux -> not a pure resetless SRL unless explicitly forced.
                return None;
            }
        }
        other => (None, other),
    };
    let RExpr::Concat(parts) = core else {
        return None;
    };
    if parts.len() != 2 {
        return None;
    }
    let din_name = match (&parts[0], &parts[1]) {
        // `{sr[DEPTH-2:0], din}`
        (RExpr::Range(s, 0, hi), RExpr::Ident(din)) if *s == sr_name && *hi + 2 == depth => *din,
        // `{din, sr[DEPTH-1:1]}`
        (RExpr::Ident(din), RExpr::Range(s, 1, hi)) if *s == sr_name && *hi + 1 == depth => *din,
        _ => return None,
    };

    // Find tap read in `rtl.assigns` or `rtl.nbas`: `dout = sr[addr]` or `dout = sr[DEPTH-1]`.
    let mut out_lhs = None;
    let mut tap_addr = None;
    let mut tap_const = None;
    for (lhs, b, arhs) in rtl.assigns.iter().chain(rtl.nbas.iter()) {
        if *lhs == sr_name || b.is_some() {
            continue;
        }
        match arhs {
            RExpr::IndexPart {
                name,
                base,
                width: 1,
                ..
            } if *name == sr_name => {
                if let RExpr::Ident(addr_id) = **base {
                    out_lhs = Some(*lhs);
                    tap_addr = Some(addr_id);
                    break;
                }
            }
            RExpr::Bit(name, idx) if *name == sr_name => {
                out_lhs = Some(*lhs);
                tap_const = Some(*idx);
                break;
            }
            _ => {}
        }
    }

    Some(DetectedSrl {
        sr_name,
        din_name,
        depth,
        width: 1,
        ce_expr,
        out_lhs,
        tap_addr,
        tap_const,
        is_unpacked: false,
    })
}

fn detect_unpacked_srl<'a>(rtl: &Rtl<'a>, sr_name: &'a str) -> Option<DetectedSrl<'a>> {
    let depth = sig_depth(rtl, sr_name);
    let width = sig_width(rtl, sr_name).max(1);
    if depth < 2 {
        return None;
    }
    let mut din_name: Option<&'a str> = None;
    let mut ce_expr: Option<RExpr<'a>> = None;
    let mut shift_count = 0usize;
    for (lhs, bit, rhs) in &rtl.nbas {
        if *lhs != sr_name {
            continue;
        }
        let Some(word) = *bit else {
            return None;
        };
        let (ce_opt, core) = match rhs {
            RExpr::Mux(c, t, f) => match *f {
                RExpr::Ident(s) | RExpr::Bit(s, _) if *s == sr_name => (Some((**c).clone()), *t),
                _ => return None,
            },
            other => (None, other),
        };
        if ce_opt.is_some() {
            ce_expr = ce_opt;
        }
        if word == 0 {
            if let RExpr::Ident(d) = core {
                din_name = Some(*d);
            } else {
                return None;
            }
        } else if let RExpr::Bit(s, prev) = core {
            if *s == sr_name && *prev + 1 == word {
                shift_count += 1;
            } else {
                return None;
            }
        } else {
            return None;
        }
    }
    let din_name = din_name?;
    if shift_count == 0 {
        return None;
    }

    let mut out_lhs = None;
    let mut tap_addr = None;
    let mut tap_const = None;
    for (lhs, b, arhs) in rtl.assigns.iter().chain(rtl.nbas.iter()) {
        if *lhs == sr_name || b.is_some() {
            continue;
        }
        match arhs {
            RExpr::IndexPart { name, base, .. } if *name == sr_name => {
                if let RExpr::Ident(addr_id) = **base {
                    out_lhs = Some(*lhs);
                    tap_addr = Some(addr_id);
                    break;
                }
            }
            RExpr::Bit(name, idx) if *name == sr_name => {
                out_lhs = Some(*lhs);
                tap_const = Some(*idx);
                break;
            }
            _ => {}
        }
    }

    Some(DetectedSrl {
        sr_name,
        din_name,
        depth,
        width,
        ce_expr,
        out_lhs,
        tap_addr,
        tap_const,
        is_unpacked: true,
    })
}

pub(crate) fn apply_ug901_srl_pass<'a>(
    rtl: &mut Rtl<'a>,
    d: &mut Design,
    clk: &str,
    ug: &Ug901ModuleAttrs,
    arena: &'a Bump,
    lowered_unpacked: &mut HashSet<String>,
    n_bram: &mut usize,
) {
    let cand_names: Vec<&'a str> = rtl.signals.iter().map(|s| s.name).collect();
    let mut removed_signals: HashSet<&'a str> = HashSet::new();
    let mut removed_out_assigns: HashSet<&'a str> = HashSet::new();

    for sr_name in cand_names {
        let eff = ug.effective_for(sr_name);
        let det = detect_1d_srl(rtl, sr_name, arena).or_else(|| detect_unpacked_srl(rtl, sr_name));
        let Some(srl) = det else {
            continue;
        };

        // Only intercept when UG901 `shreg_extract` or `srl_style` is specified, OR when
        // it is an unpacked dynamic-tap shift register (`dout = sr[addr]`).
        let has_explicit_attr = eff.shreg_extract.is_some() || eff.srl_style.is_some();
        let is_dynamic_unpacked = srl.is_unpacked && srl.tap_addr.is_some();
        if !has_explicit_attr && !is_dynamic_unpacked {
            continue;
        }

        // Case 1: `shreg_extract = "no"` or `srl_style = "register"` -> force HFF chain.
        if eff.shreg_extract == Some(false) || eff.srl_style == Some(SrlStyleMode::Register) {
            d.attrs.set("SHREG_EXTRACT", "NO");
            if eff.srl_style == Some(SrlStyleMode::Register) {
                d.attrs.set("SRL_STYLE", "REGISTER");
            }
            if srl.is_unpacked {
                let ce_e = srl
                    .ce_expr
                    .as_ref()
                    .and_then(|er| simple_word_enable(er, rtl, arena));
                for bit in 0..srl.width {
                    for stage in 0..srl.depth {
                        let qn = unpacked_word_q(srl.sr_name, stage, srl.width, bit);
                        let prev = if stage == 0 {
                            bit_name(srl.din_name, srl.width, bit)
                        } else {
                            unpacked_word_q(srl.sr_name, stage - 1, srl.width, bit)
                        };
                        let dnet = if let Some(ce) = &ce_e {
                            let mux_e = word_ce_mux(
                                ce.clone(),
                                Expr::Var(arena.alloc_str(&prev)),
                                Expr::Var(arena.alloc_str(&qn)),
                                arena,
                            );
                            let lcell = format!("u_shreg_ce_{}_{stage}_{bit}", srl.sr_name);
                            let mnet = format!("{qn}_d");
                            let _ = emit_small_lut(d, &lcell, &mnet, &mux_e);
                            mnet
                        } else {
                            prev
                        };
                        let ff = format!("u_shreg_ff_{}_{stage}_{bit}", srl.sr_name);
                        d.add_cell(&ff, CellKind::Hff);
                        d.connect(clk, &ff, "CLK");
                        d.connect(&dnet, &ff, "D");
                        d.connect(&qn, &ff, "Q");
                        let _ = d.set_cell_attr(&ff, "SHREG_EXTRACT", "NO");
                    }
                }
                if let (Some(out_lhs), Some(addr_id)) = (srl.out_lhs, srl.tap_addr) {
                    let addr_w = sig_width(rtl, addr_id).max(1);
                    for bit in 0..srl.width {
                        let mut acc: Option<Expr> = None;
                        for stage in 0..srl.depth.min(16) {
                            let m = addr_match_expr(addr_id, addr_w, stage, None, arena);
                            let qn = unpacked_word_q(srl.sr_name, stage, srl.width, bit);
                            let term =
                                Expr::And(arena.alloc(m), arena.alloc(Expr::Var(arena.alloc_str(&qn))));
                            acc = Some(match acc {
                                None => term,
                                Some(p) => Expr::Or(arena.alloc(p), arena.alloc(term)),
                            });
                        }
                        if let Some(mux_e) = acc {
                            let out_net = bit_name(out_lhs, srl.width, bit);
                            let lcell = format!("u_shreg_mux_{}_{bit}", srl.sr_name);
                            let _ = emit_small_lut(d, &lcell, &out_net, &mux_e);
                        }
                    }
                    removed_out_assigns.insert(out_lhs);
                }
                lowered_unpacked.insert(srl.sr_name.to_string());
                removed_signals.insert(srl.sr_name);
            }
            continue;
        }

        // Case 2: `srl_style = "block"` -> emit `Bram18e1` circular shift buffer.
        if eff.srl_style == Some(SrlStyleMode::Block) {
            let cell = format!("u_bram{}", *n_bram);
            *n_bram += 1;
            let bw = clamp_bram_width(srl.width);
            d.add_cell(
                &cell,
                CellKind::Bram18e1 {
                    read_width_a: bw,
                    read_width_b: bw,
                    write_width_a: bw,
                    write_width_b: bw,
                },
            );
            d.connect(clk, &cell, "CLKARDCLK");
            d.connect(srl.din_name, &cell, "DIADI");
            if let Some(addr_id) = srl.tap_addr {
                d.connect(addr_id, &cell, "ADDRARDADDR");
            }
            let out_net = srl.out_lhs.unwrap_or(srl.sr_name);
            d.connect(out_net, &cell, "DOADO");
            let _ = d.set_cell_attr(&cell, "SRL_STYLE", "BLOCK");
            d.attrs.set("SRL_STYLE", "BLOCK");
            lowered_unpacked.insert(srl.sr_name.to_string());
            removed_signals.insert(srl.sr_name);
            if let Some(o) = srl.out_lhs {
                removed_out_assigns.insert(o);
            }
            continue;
        }

        // Case 3: Extract `SRL16E` (depth <= 16) or `SRLC32E` (depth > 16, cascading via Q31->D).
        let style = eff.srl_style.unwrap_or(SrlStyleMode::Srl);
        let use_head_reg = matches!(style, SrlStyleMode::RegSrl | SrlStyleMode::RegSrlReg);
        let use_tail_reg = matches!(style, SrlStyleMode::SrlReg | SrlStyleMode::RegSrlReg);
        let style_str = match style {
            SrlStyleMode::Srl => "SRL",
            SrlStyleMode::SrlReg => "SRL_REG",
            SrlStyleMode::RegSrl => "REG_SRL",
            SrlStyleMode::RegSrlReg => "REG_SRL_REG",
            SrlStyleMode::Register => "REGISTER",
            SrlStyleMode::Block => "BLOCK",
        };
        let eff_depth = srl
            .depth
            .saturating_sub(usize::from(use_head_reg))
            .saturating_sub(usize::from(use_tail_reg))
            .max(1);
        let srl_prim = if eff_depth <= 16 { "SRL16E" } else { "SRLC32E" };
        let n_cascade = ((eff_depth + 31) / 32).max(1);
        let ce_net = srl.ce_expr.as_ref().and_then(|e| match e {
            RExpr::Ident(s) => Some((*s).to_string()),
            _ => None,
        });

        for bit in 0..srl.width {
            let raw_din = bit_name(srl.din_name, srl.width, bit);
            let srl_din = if use_head_reg {
                let hff = format!("u_srl_head_ff_{}_{bit}", srl.sr_name);
                let hq = format!("{}_{bit}_head_q", srl.sr_name);
                d.add_cell(&hff, CellKind::Hff);
                d.connect(clk, &hff, "CLK");
                d.connect(&raw_din, &hff, "D");
                d.connect(&hq, &hff, "Q");
                if let Some(ce) = &ce_net {
                    d.connect(ce, &hff, "CE");
                }
                let _ = d.set_cell_attr(&hff, "SRL_STYLE", style_str);
                hq
            } else {
                raw_din
            };

            let final_out = if let Some(out_lhs) = srl.out_lhs {
                bit_name(out_lhs, srl.width, bit)
            } else if srl.is_unpacked {
                unpacked_word_q(srl.sr_name, srl.depth - 1, srl.width, bit)
            } else {
                bit_name(srl.sr_name, srl.depth, srl.depth - 1)
            };
            let srl_q = if use_tail_reg {
                format!("{}_{bit}_srl_q", srl.sr_name)
            } else {
                final_out.clone()
            };

            let mut stage_in = srl_din;
            for c_idx in 0..n_cascade {
                let cname = if n_cascade == 1 {
                    format!(
                        "u_{}_{}_{bit}",
                        srl_prim.to_ascii_lowercase(),
                        srl.sr_name
                    )
                } else {
                    format!(
                        "u_srlc32e_{}_{bit}_c{c_idx}",
                        srl.sr_name
                    )
                };
                let stage_out = if c_idx + 1 == n_cascade {
                    srl_q.clone()
                } else {
                    format!("{}_{bit}_q31_{c_idx}", srl.sr_name)
                };
                d.add_cell(&cname, CellKind::Lut6 { init: 0xAAAA_AAAA_AAAA_AAAA });
                let _ = d.dont_touch(&cname);
                d.connect(clk, &cname, "CLK");
                d.connect(&stage_in, &cname, "D");
                d.connect(&stage_in, &cname, "I0");
                d.connect(&stage_out, &cname, "Q");
                d.connect(&stage_out, &cname, "O");
                if c_idx + 1 < n_cascade {
                    d.connect(&stage_out, &cname, "Q31");
                }
                if let Some(ce) = &ce_net {
                    d.connect(ce, &cname, "CE");
                }
                if let Some(addr_id) = srl.tap_addr {
                    let aw = sig_width(rtl, addr_id).max(1);
                    let max_a = if srl_prim == "SRL16E" { 4 } else { 5 };
                    for a_i in 0..aw.min(max_a) {
                        let anet = bit_name(addr_id, aw, a_i);
                        d.connect(&anet, &cname, &format!("A{a_i}"));
                        d.connect(&anet, &cname, &format!("I{}", a_i + 1));
                    }
                } else if let Some(tap_idx) = srl.tap_const {
                    let _ = d.set_cell_attr(&cname, "TAP_INDEX", tap_idx.to_string());
                }
                let _ = d.set_cell_attr(&cname, "SRL_TYPE", srl_prim);
                let _ = d.set_cell_attr(&cname, "BEL_TYPE", srl_prim);
                let _ = d.set_cell_attr(&cname, "SHREG_EXTRACT", "YES");
                let _ = d.set_cell_attr(&cname, "SRL_STYLE", style_str);
                let _ = d.set_cell_attr(&cname, "DEPTH", eff_depth.to_string());
                stage_in = stage_out;
            }

            if use_tail_reg {
                let tff = format!("u_srl_tail_ff_{}_{bit}", srl.sr_name);
                d.add_cell(&tff, CellKind::Hff);
                d.connect(clk, &tff, "CLK");
                d.connect(&srl_q, &tff, "D");
                d.connect(&final_out, &tff, "Q");
                if let Some(ce) = &ce_net {
                    d.connect(ce, &tff, "CE");
                }
                let _ = d.set_cell_attr(&tff, "SRL_STYLE", style_str);
            }
        }

        d.attrs.set("SHREG_EXTRACT", "YES");
        d.attrs.set("SRL_STYLE", style_str);
        d.attrs.set("SRL_TYPE", srl_prim);
        lowered_unpacked.insert(srl.sr_name.to_string());
        removed_signals.insert(srl.sr_name);
        if let Some(o) = srl.out_lhs {
            removed_out_assigns.insert(o);
        }
    }

    if !removed_signals.is_empty() {
        rtl.nbas
            .retain(|(lhs, _, _)| !removed_signals.contains(lhs) && !removed_out_assigns.contains(lhs));
    }
    if !removed_out_assigns.is_empty() {
        rtl.assigns
            .retain(|(lhs, _, _)| !removed_out_assigns.contains(lhs));
    }
}

// ============================================================================
// Domain 2: `ram_style` / `rom_style`, Asymmetric Aspect Ratios & Byte-Write Enables
// ============================================================================

pub(crate) fn clamp_bram_width(width: usize) -> u8 {
    if width <= 1 {
        1
    } else if width <= 2 {
        2
    } else if width <= 4 {
        4
    } else if width <= 9 {
        9
    } else if width <= 18 {
        18
    } else if width <= 36 {
        36
    } else {
        72
    }
}

fn count_mem_reads_in_rexpr(e: &RExpr<'_>, mem: &str) -> usize {
    match e {
        RExpr::IndexPart { name, .. } | RExpr::Bit(name, _) if *name == mem => 1,
        RExpr::Concat(parts) => parts.iter().map(|p| count_mem_reads_in_rexpr(p, mem)).sum(),
        RExpr::Mux(c, t, f) => {
            count_mem_reads_in_rexpr(c, mem)
                .max(count_mem_reads_in_rexpr(t, mem))
                .max(count_mem_reads_in_rexpr(f, mem))
        }
        _ => 0,
    }
}

fn extract_rom_table_from_mux(
    e: &RExpr<'_>,
    addr_out: &mut Option<String>,
    table: &mut BTreeMap<usize, u128>,
) -> bool {
    match e {
        RExpr::Const { val, .. } => {
            table.entry(0).or_insert(*val);
            true
        }
        RExpr::Mux(c, t, f) => {
            if let RExpr::Eq(l, r) = *c {
                let matched = match (*l, *r) {
                    (RExpr::Ident(a), RExpr::Const { val, .. })
                    | (RExpr::Const { val, .. }, RExpr::Ident(a)) => Some((*a, *val as usize)),
                    _ => None,
                };
                if let (Some((a_name, idx)), RExpr::Const { val: tval, .. }) = (matched, *t) {
                    if addr_out.is_none() {
                        *addr_out = Some(a_name.to_string());
                    }
                    table.insert(idx, *tval);
                    return extract_rom_table_from_mux(f, addr_out, table);
                }
            }
            false
        }
        _ => false,
    }
}

fn find_read_port_for_mem<'a>(rtl: &Rtl<'a>, mem: &str) -> (Option<String>, Option<&'a str>, usize) {
    let base_w = sig_width(rtl, mem).max(1);
    for (lhs, bit, rhs) in rtl.nbas.iter().chain(rtl.assigns.iter()) {
        if *lhs == mem || bit.is_some() {
            continue;
        }
        let n_reads = count_mem_reads_in_rexpr(rhs, mem);
        if n_reads > 0 {
            let lhs_w = sig_width(rtl, lhs).max(base_w * n_reads);
            let raddr = find_first_mem_index_ident(rhs, mem);
            return (raddr, Some(*lhs), lhs_w);
        }
    }
    (None, None, base_w)
}

fn find_first_mem_index_ident(e: &RExpr<'_>, mem: &str) -> Option<String> {
    match e {
        RExpr::IndexPart { name, base, .. } if *name == mem => match *base {
            RExpr::Ident(a) => Some((*a).to_string()),
            RExpr::Concat(parts) => parts.iter().find_map(|p| match p {
                RExpr::Ident(a) => Some((*a).to_string()),
                _ => None,
            }),
            _ => None,
        },
        RExpr::Concat(parts) => parts.iter().find_map(|p| find_first_mem_index_ident(p, mem)),
        RExpr::Mux(c, t, f) => find_first_mem_index_ident(c, mem)
            .or_else(|| find_first_mem_index_ident(t, mem))
            .or_else(|| find_first_mem_index_ident(f, mem)),
        _ => None,
    }
}

pub(crate) fn apply_ug901_ram_rom_pass<'a>(
    rtl: &mut Rtl<'a>,
    d: &mut Design,
    clk: &str,
    ug: &Ug901ModuleAttrs,
    arena: &'a Bump,
    lowered_unpacked: &mut HashSet<String>,
    n_bram: &mut usize,
) {
    // 1. Check for synchronous case-based ROMs with `rom_style = "block" | "distributed"` or `ram_style`.
    let mut rom_removed_nbas: HashSet<&'a str> = HashSet::new();
    for (lhs, bit, rhs) in &rtl.nbas {
        if !matches!(bit, None | Some(0)) || sig_depth(rtl, lhs) > 0 {
            continue;
        }
        let eff = ug.effective_for(lhs);
        if eff.rom_style.is_none() && eff.ram_style.is_none() {
            continue;
        }
        let core = peel_sync_reset_or_en(rhs, lhs);
        let mut addr_name = None;
        let mut table = BTreeMap::new();
        if extract_rom_table_from_mux(core, &mut addr_name, &mut table) && table.len() >= 2 {
            if eff.rom_style == Some(RomStyleMode::Distributed)
                || eff.ram_style == Some(RamStyleMode::Distributed)
            {
                d.attrs.set("ROM_STYLE", "DISTRIBUTED");
                continue;
            }
            if eff.rom_style == Some(RomStyleMode::Block)
                || eff.ram_style == Some(RamStyleMode::Block)
            {
                let cell = format!("u_bram{}", *n_bram);
                *n_bram += 1;
                let w = sig_width(rtl, lhs).max(1);
                let bw = clamp_bram_width(w);
                let depth = table.keys().max().copied().unwrap_or(0) + 1;
                let mut words = vec![0u64; depth.max(1).min(1024)];
                for (k, v) in &table {
                    if *k < words.len() {
                        words[*k] = *v as u64;
                    }
                }
                let hex = words
                    .iter()
                    .map(|x| format!("{x:x}"))
                    .collect::<Vec<_>>()
                    .join(",");
                d.add_cell(
                    &cell,
                    CellKind::Bram18e1 {
                        read_width_a: bw,
                        read_width_b: bw,
                        write_width_a: bw,
                        write_width_b: bw,
                    },
                );
                d.connect(clk, &cell, "CLKARDCLK");
                if let Some(a) = &addr_name {
                    d.connect(a, &cell, "ADDRARDADDR");
                }
                d.connect(*lhs, &cell, "DOADO");
                let _ = d.set_cell_attr(&cell, "INIT", hex);
                let _ = d.set_cell_attr(&cell, "ROM_STYLE", "BLOCK");
                d.attrs.set("ROM_STYLE", "BLOCK");
                rom_removed_nbas.insert(*lhs);
            }
        }
    }
    if !rom_removed_nbas.is_empty() {
        rtl.nbas.retain(|(lhs, _, _)| !rom_removed_nbas.contains(lhs));
    }

    // 2. Process unpacked memories (`depth > 0`) according to `ram_style` / `rom_style`,
    //    byte-write enables, and asymmetric aspect ratios.
    let mem_sigs: Vec<Signal<'a>> = rtl
        .signals
        .iter()
        .filter(|s| s.depth > 0 && !lowered_unpacked.contains(s.name))
        .cloned()
        .collect();

    for sig in mem_sigs {
        let mem = sig.name;
        let eff = ug.effective_for(mem);
        let depth = sig.depth.max(1);
        let base_w = sig.width.max(1);

        // 2a. `ram_style = "distributed"` or `rom_style = "distributed"` -> LUTRAM / LUTROM (no Bram18e1).
        if eff.ram_style == Some(RamStyleMode::Distributed)
            || eff.rom_style == Some(RomStyleMode::Distributed)
        {
            if eff.rom_style == Some(RomStyleMode::Distributed) {
                d.attrs.set("ROM_STYLE", "DISTRIBUTED");
            } else {
                d.attrs.set("RAM_STYLE", "DISTRIBUTED");
            }
            let (raddr_opt, dout_opt, _) = find_read_port_for_mem(rtl, mem);
            let out_sig = dout_opt.unwrap_or(mem);
            let raddr = raddr_opt.unwrap_or_else(|| "addr".to_string());
            let addr_w = sig_width(rtl, &raddr).max(1);
            let init_map = rtl.mem_inits.get(mem);

            for bit in 0..base_w.min(32) {
                let mut acc: Option<Expr> = None;
                for word in 0..depth.min(16) {
                    let m = addr_match_expr(&raddr, addr_w, word, None, arena);
                    let src_e = if let Some(im) = init_map {
                        let v = im.get(&word).copied().unwrap_or(0);
                        Expr::Const((v >> bit) & 1 == 1)
                    } else {
                        let qn = unpacked_word_q(mem, word, base_w, bit);
                        Expr::Var(arena.alloc_str(&qn))
                    };
                    let term = Expr::And(arena.alloc(m), arena.alloc(src_e));
                    acc = Some(match acc {
                        None => term,
                        Some(p) => Expr::Or(arena.alloc(p), arena.alloc(term)),
                    });
                }
                if let Some(mux_e) = acc {
                    let out_net = bit_name(out_sig, base_w, bit);
                    let lcell = format!("u_lutram_{mem}_{bit}");
                    if !emit_small_lut(d, &lcell, &out_net, &mux_e) {
                        let aig = Aig::from_expr(&mux_e);
                        let init = aig.flowmap_lut6();
                        d.add_cell(&lcell, CellKind::Lut6 { init });
                        d.connect(&out_net, &lcell, "O");
                        for (pin, pi) in aig.pis.iter().take(6).enumerate() {
                            d.connect(pi, &lcell, &format!("I{pin}"));
                        }
                    }
                    let _ = d.set_cell_attr(&lcell, "RAM_STYLE", "DISTRIBUTED");
                }
            }
            lowered_unpacked.insert(mem.to_string());
            if let Some(dout) = dout_opt {
                rtl.nbas.retain(|(l, _, _)| *l != dout && *l != mem);
                rtl.assigns.retain(|(l, _, _)| *l != dout);
            } else {
                rtl.nbas.retain(|(l, _, _)| *l != mem);
            }
            continue;
        }

        // 2b. `ram_style = "registers"` -> synthesize HFFs per word/bit + LUT read mux (no Bram18e1).
        if eff.ram_style == Some(RamStyleMode::Registers) {
            d.attrs.set("RAM_STYLE", "REGISTERS");
            let (raddr_opt, dout_opt, _) = find_read_port_for_mem(rtl, mem);
            let out_sig = dout_opt.unwrap_or(mem);
            let raddr = raddr_opt.unwrap_or_else(|| "addr".to_string());
            let addr_w = sig_width(rtl, &raddr).max(1);

            for word in 0..depth.min(32) {
                for bit in 0..base_w.min(32) {
                    let qn = unpacked_word_q(mem, word, base_w, bit);
                    let ff = format!("u_reg_ram_{mem}_w{word}_{bit}");
                    d.add_cell(&ff, CellKind::Hff);
                    d.connect(clk, &ff, "CLK");
                    d.connect(&qn, &ff, "D");
                    d.connect(&qn, &ff, "Q");
                    let _ = d.set_cell_attr(&ff, "RAM_STYLE", "REGISTERS");
                }
            }
            for bit in 0..base_w.min(32) {
                let mut acc: Option<Expr> = None;
                for word in 0..depth.min(16) {
                    let m = addr_match_expr(&raddr, addr_w, word, None, arena);
                    let qn = unpacked_word_q(mem, word, base_w, bit);
                    let term = Expr::And(arena.alloc(m), arena.alloc(Expr::Var(arena.alloc_str(&qn))));
                    acc = Some(match acc {
                        None => term,
                        Some(p) => Expr::Or(arena.alloc(p), arena.alloc(term)),
                    });
                }
                if let Some(mux_e) = acc {
                    let out_net = bit_name(out_sig, base_w, bit);
                    let lcell = format!("u_reg_ram_mux_{mem}_{bit}");
                    let _ = emit_small_lut(d, &lcell, &out_net, &mux_e);
                }
            }
            lowered_unpacked.insert(mem.to_string());
            if let Some(dout) = dout_opt {
                rtl.nbas.retain(|(l, _, _)| *l != dout && *l != mem);
                rtl.assigns.retain(|(l, _, _)| *l != dout);
            } else {
                rtl.nbas.retain(|(l, _, _)| *l != mem);
            }
            continue;
        }

        // 2c. Inspect writes to `mem` for byte-write enables (`bit = Some(lane_lo)`) and multi-writes.
        let mut waddr_net: Option<String> = None;
        let mut wdata_net: Option<String> = None;
        let mut byte_we_nets: Vec<String> = Vec::new();
        let mut write_lanes = 0usize;
        let mut lane_width = base_w;

        for (lhs, bit, rhs) in &rtl.nbas {
            if *lhs != mem {
                continue;
            }
            let (we_opt, payload) = match rhs {
                RExpr::Mux(c, t, _) => (Some(*c), *t),
                other => (None, other),
            };
            if let RExpr::WordAt { addr, data } = payload {
                write_lanes += 1;
                if waddr_net.is_none() {
                    waddr_net = match *addr {
                        RExpr::Ident(a) => Some((*a).to_string()),
                        RExpr::Concat(parts) => parts.iter().find_map(|p| match p {
                            RExpr::Ident(a) => Some((*a).to_string()),
                            _ => None,
                        }),
                        _ => None,
                    };
                }
                match *data {
                    RExpr::Ident(dn) => {
                        if wdata_net.is_none() {
                            wdata_net = Some((*dn).to_string());
                        }
                        lane_width = sig_width(rtl, dn).max(1);
                    }
                    RExpr::Range(dn, lo, hi) => {
                        if wdata_net.is_none() {
                            wdata_net = Some((*dn).to_string());
                        }
                        lane_width = hi.saturating_sub(*lo).saturating_add(1).max(1);
                    }
                    RExpr::IndexPart {
                        name: dn, width, ..
                    } => {
                        if wdata_net.is_none() {
                            wdata_net = Some((*dn).to_string());
                        }
                        lane_width = (*width).max(1);
                    }
                    _ => {}
                }
                if let Some(we_e) = we_opt {
                    let we_name = match we_e {
                        RExpr::Ident(w) => Some((*w).to_string()),
                        RExpr::Bit(w, idx) => Some(format!("{w}_{idx}")),
                        _ => None,
                    };
                    if let Some(wn) = we_name {
                        if !byte_we_nets.contains(&wn) {
                            byte_we_nets.push(wn);
                        }
                    }
                } else if bit.is_some() {
                    byte_we_nets.push(format!("{mem}_we_{}", bit.unwrap_or(0)));
                }
            }
        }

        let (raddr_opt, dout_opt, read_port_w) = find_read_port_for_mem(rtl, mem);
        let write_total_w = if write_lanes > 1 {
            write_lanes * lane_width
        } else {
            base_w
        };
        let is_asymmetric = read_port_w != write_total_w;
        let has_byte_we = byte_we_nets.len() > 1 || (write_lanes > 1);
        let is_ultra = eff.ram_style == Some(RamStyleMode::Ultra);
        let is_block = eff.ram_style == Some(RamStyleMode::Block)
            || eff.rom_style == Some(RomStyleMode::Block)
            || sig.force_bram;

        if is_ultra || is_block || has_byte_we || is_asymmetric {
            let cell = format!("u_bram{}", *n_bram);
            *n_bram += 1;
            let r_bw = clamp_bram_width(read_port_w);
            let w_bw = clamp_bram_width(write_total_w);
            d.add_cell(
                &cell,
                CellKind::Bram18e1 {
                    read_width_a: r_bw,
                    read_width_b: r_bw,
                    write_width_a: w_bw,
                    write_width_b: w_bw,
                },
            );
            d.connect(clk, &cell, "CLKARDCLK");
            if let Some(wa) = &waddr_net {
                d.connect(wa, &cell, "ADDRARDADDR");
            } else if let Some(ra) = &raddr_opt {
                d.connect(ra, &cell, "ADDRARDADDR");
            }
            if let Some(ra) = &raddr_opt {
                d.connect(ra, &cell, "ADDRBWRADDR");
            }
            if let Some(wd) = &wdata_net {
                d.connect(wd, &cell, "DIADI");
            }
            let dout_net = dout_opt
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("{mem}_dout"));
            d.connect(&dout_net, &cell, "DOADO");

            for (lane_i, we_net) in byte_we_nets.iter().enumerate() {
                if lane_i < 2 {
                    d.connect(we_net, &cell, &format!("WEA[{lane_i}]"));
                }
                if lane_i < 4 {
                    d.connect(we_net, &cell, &format!("WEBWE[{lane_i}]"));
                }
            }
            if has_byte_we {
                let _ = d.set_cell_attr(&cell, "BYTE_WRITE_WIDTH", lane_width.to_string());
                let _ = d.set_cell_attr(&cell, "WE_WIDTH", byte_we_nets.len().max(write_lanes).to_string());
                d.attrs.set("BYTE_WRITE_ENABLE", "1");
            }
            if is_asymmetric {
                let _ = d.set_cell_attr(&cell, "READ_WIDTH_A", r_bw.to_string());
                let _ = d.set_cell_attr(&cell, "WRITE_WIDTH_A", w_bw.to_string());
                d.attrs.set("ASYMMETRIC_BRAM", "1");
            }
            if is_ultra {
                let _ = d.set_cell_attr(&cell, "RAM_STYLE", "ULTRA");
                d.attrs.set("RAM_STYLE", "ULTRA");
            } else if eff.rom_style == Some(RomStyleMode::Block) {
                let _ = d.set_cell_attr(&cell, "ROM_STYLE", "BLOCK");
                d.attrs.set("ROM_STYLE", "BLOCK");
            } else {
                let _ = d.set_cell_attr(&cell, "RAM_STYLE", "BLOCK");
                d.attrs.set("RAM_STYLE", "BLOCK");
            }

            let mut words = vec![0u64; depth.max(1).min(1024)];
            if let Some(init) = rtl.mem_inits.get(mem) {
                for (addr, val) in init {
                    if *addr < words.len() {
                        words[*addr] = *val as u64;
                    }
                }
            }
            let hex = words
                .iter()
                .map(|w| format!("{w:x}"))
                .collect::<Vec<_>>()
                .join(",");
            let _ = d.set_cell_attr(&cell, "INIT", hex);

            lowered_unpacked.insert(mem.to_string());
            rtl.nbas.retain(|(l, _, _)| *l != mem);
            if let Some(dout) = dout_opt {
                rtl.nbas.retain(|(l, _, _)| *l != dout);
                rtl.assigns.retain(|(l, _, _)| *l != dout);
            }
        }
    }
}
