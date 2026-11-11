//! Resolved (elaborated) form of VHDL processes and subprograms: names are
//! bound to signal ids / variable slots / constants, statements are flat
//! op lists with jumps, so a process can suspend at any `wait`.

use std::rc::Rc;

use super::ast::{BinOp, UnOp};
use super::value::{Ty, Value};

pub type SigId = usize;

/// Source location: index into the design's file list, 1-based line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Loc {
    pub file: u16,
    pub line: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bi {
    RisingEdge,
    FallingEdge,
    ToUnsigned,
    ToSigned,
    ToInteger,
    Resize,
    ShiftLeft,
    ShiftRight,
    RotateLeft,
    RotateRight,
    ToString,
    ToHString,
    ToBString,
    StdMatch,
    IsX,
    ToX01,
    Minimum,
    Maximum,
    ToReal,
    ToIntegerFromReal,
    Floor,
    Round,
    Now,
    ToSlv,
    Ceil,
    Sqrt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrAttr {
    Length,
    High,
    Low,
    Left,
    Right,
    Ascending,
}

#[derive(Clone, Debug)]
pub enum RChoice {
    Val(RExpr),
    Range(RExpr, bool, RExpr),
    Others,
}

#[derive(Clone, Debug)]
pub enum RExpr {
    Const(Value),
    Sig(SigId),
    Var(usize),
    Index(Box<RExpr>, Box<RExpr>),
    Slice(Box<RExpr>, Box<RExpr>, bool, Box<RExpr>),
    Field(Box<RExpr>, usize),
    Un(UnOp, Box<RExpr>),
    Bin(BinOp, Box<RExpr>, Box<RExpr>),
    /// User function call (function id, arguments).
    Call(usize, Vec<RExpr>),
    Bi(Bi, Vec<RExpr>),
    Event(SigId),
    LastValue(SigId),
    LastEvent(SigId),
    /// Array aggregate of a known (or context-derived) array type.
    ArrAgg {
        ty: Option<Ty>,
        pos: Vec<RExpr>,
        named: Vec<(Vec<RChoice>, RExpr)>,
        others: Option<Box<RExpr>>,
    },
    /// Record aggregate, fields in declaration order.
    RecAgg(Vec<RExpr>),
    /// Type conversion / qualification to `Ty`.
    Conv(Ty, Box<RExpr>),
    /// `T'image(x)`.
    Image(Ty, Box<RExpr>),
    /// `T'value(s)` for integer / enumeration types.
    ValueOf(Ty, Box<RExpr>),
    /// `T'pos(x)` / `T'val(n)` / `'succ` / `'pred` on scalars.
    Pos(Box<RExpr>),
    Val(Ty, Box<RExpr>),
    ArrAttr(ArrAttr, Box<RExpr>),
}

#[derive(Clone, Debug)]
pub enum RSel {
    Index(RExpr),
    Slice(RExpr, bool, RExpr),
    Field(usize),
}

#[derive(Clone, Debug)]
pub enum RTarget {
    Sig(SigId, Vec<RSel>),
    Var(usize, Vec<RSel>),
    Agg(Vec<RTarget>),
}

#[derive(Clone, Debug)]
pub enum RRange {
    Explicit(RExpr, bool, RExpr),
    /// `x'range` (`reverse` for `'reverse_range`).
    Of(RExpr, bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinProc {
    /// `std.env.stop` / `std.env.finish`.
    Stop,
    /// `textio.write(L, value [, justified, field])`.
    Write,
    /// `textio.writeline(file, L)`.
    WriteLine,
    /// `ieee.math_real.uniform(seed1, seed2, x)`.
    Uniform,
    Deallocate,
}

#[derive(Clone, Debug)]
pub enum Op {
    SigAssign {
        target: RTarget,
        wave: Vec<(RExpr, Option<RExpr>)>,
        transport: bool,
        /// Declared type of the target (for literal coercion).
        ty: Option<Ty>,
        loc: Loc,
    },
    VarAssign {
        target: RTarget,
        value: RExpr,
        ty: Option<Ty>,
        loc: Loc,
    },
    Jump(usize),
    JumpIfNot(RExpr, usize, Loc),
    Case {
        sel: RExpr,
        arms: Vec<(Vec<RChoice>, usize)>,
        end: usize,
        loc: Loc,
    },
    Wait {
        on: Vec<SigId>,
        until: Option<RExpr>,
        timeout: Option<RExpr>,
        loc: Loc,
    },
    Report {
        msg: RExpr,
        sev: Option<RExpr>,
        loc: Loc,
    },
    Assert {
        cond: RExpr,
        msg: Option<RExpr>,
        sev: Option<RExpr>,
        loc: Loc,
    },
    /// Loop variable `var` = range start; `end_slot` / `dir_slot` hold the
    /// bound and direction; jump to `exit` when the range is null.
    ForInit {
        var: usize,
        end_slot: usize,
        dir_slot: usize,
        range: RRange,
        exit: usize,
        loc: Loc,
    },
    /// Advance `var`; jump to `body` unless it was the last value.
    ForNext {
        var: usize,
        end_slot: usize,
        dir_slot: usize,
        body: usize,
    },
    Builtin {
        proc_: BuiltinProc,
        args: Vec<RExpr>,
        outs: Vec<RTarget>,
        loc: Loc,
    },
    Return(Option<RExpr>, Loc),
    Nop,
}

#[derive(Clone, Debug, Default)]
pub struct Code {
    pub ops: Vec<Op>,
    /// Where a process loops back to (after one-time variable inits).
    pub body_start: usize,
    pub var_tys: Vec<Option<Ty>>,
}

#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub nparams: usize,
    pub param_tys: Vec<Option<Ty>>,
    pub ret: Option<Ty>,
    pub code: Rc<Code>,
}

#[derive(Clone, Debug)]
pub struct ProcDef {
    pub name: String,
    pub code: Rc<Code>,
    pub loc: Loc,
}

#[derive(Clone, Debug)]
pub struct SignalDef {
    pub name: String,
    pub ty: Ty,
    pub init: Value,
}
