//! Abstract syntax of the testbench language.

use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Loc {
    pub file: u32,
    pub line: u32,
}

/// A statement list (all bodies are shared slices so process frames can
/// hold them cheaply).
pub type Body = Rc<[S]>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntKind {
    Bit,
    Logic,
    Reg,
    Int,
    Integer,
    Byte,
    Shortint,
    Longint,
    Time,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DataType {
    Int { kind: IntKind, signed: Option<bool>, packed: Vec<(Expr, Expr)> },
    Real,
    Str,
    Event,
    Void,
    Chandle,
    /// Implicit (`input [3:0] x`, `parameter N = 4`): packed dims only.
    Implicit { signed: bool, packed: Vec<(Expr, Expr)> },
    /// User type: `pkg::name`, `cls#(args)::name`, `name#(args)`.
    Named { path: Vec<Seg> },
    Virtual { iface: String, modport: Option<String> },
    Enum { base: Box<DataType>, items: Vec<(String, Option<Expr>)> },
    /// `type(expr)` is not supported; parameter type placeholders resolve by name.
    Struct { packed: bool, fields: Vec<(DataType, String)> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seg {
    pub name: String,
    pub params: Option<Vec<ParamArg>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParamArg {
    Type(DataType),
    Expr(Expr),
    /// Named: `.T(int)`.
    Named(String, Box<ParamArg>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum UDim {
    Fixed(Expr, Option<Expr>),
    Dynamic,
    Queue(Option<Expr>),
    /// `[string]`, `[int]`, `[*]`, `[cls]`.
    Assoc(Option<DataType>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RandKind {
    #[default]
    None,
    Rand,
    Randc,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VarDecl {
    pub ty: DataType,
    pub name: String,
    pub udims: Vec<UDim>,
    pub init: Option<Expr>,
    pub rand: RandKind,
    pub is_static: bool,
    pub is_const: bool,
    pub loc: Loc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Input,
    Output,
    Inout,
    Ref,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArgDecl {
    pub dir: Dir,
    pub ty: DataType,
    pub name: String,
    pub udims: Vec<UDim>,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubQuals {
    pub is_virtual: bool,
    pub is_static: bool,
    pub is_pure: bool,
    pub is_local: bool,
    pub is_extern: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubDecl {
    pub name: String,
    pub is_task: bool,
    /// `None` for tasks and `new`.
    pub ret: Option<DataType>,
    pub args: Vec<ArgDecl>,
    pub body: Body,
    pub quals: SubQuals,
    pub loc: Loc,
    /// Out-of-class body: `function C::name`.
    pub class_scope: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CItem {
    Expr(Expr),
    Inside { e: Expr, items: Vec<InsideItem>, neg: bool },
    Dist { e: Expr, items: Vec<DistItem> },
    Implies(Expr, Vec<CItem>),
    IfElse(Expr, Vec<CItem>, Vec<CItem>),
    SolveBefore(Vec<Expr>, Vec<Expr>),
    Foreach { arr: Expr, vars: Vec<Option<String>>, body: Vec<CItem> },
    Soft(Box<CItem>),
    Unique(Vec<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct DistItem {
    pub lo: Expr,
    pub hi: Option<Expr>,
    /// `true` for `:/` (weight divided across the range).
    pub per_range: bool,
    pub weight: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstraintDecl {
    pub name: String,
    pub items: Vec<CItem>,
    pub is_static: bool,
    pub loc: Loc,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParamDecl {
    pub is_type: bool,
    pub ty: Option<DataType>,
    pub name: String,
    pub default: Option<ParamArg>,
    pub local: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Typedef {
    pub name: String,
    /// `None` for a forward declaration (`typedef class foo;`).
    pub ty: Option<DataType>,
    pub udims: Vec<UDim>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClassItem {
    Prop(VarDecl),
    Method(Rc<SubDecl>),
    Constraint(ConstraintDecl),
    Typedef(Typedef),
    Param(ParamDecl),
    Class(Rc<ClassDecl>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassDecl {
    pub name: String,
    pub is_virtual: bool,
    pub params: Vec<ParamDecl>,
    pub extends: Option<DataType>,
    pub items: Vec<ClassItem>,
    pub loc: Loc,
    /// Unique id assigned by the parser.
    pub uid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlwaysKind {
    Always,
    Comb,
    Ff,
    Latch,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PortDecl {
    pub dir: Dir,
    pub ty: DataType,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub module: String,
    pub params: Vec<(Option<String>, ParamArg)>,
    pub name: String,
    /// `(port, expr)`; port `None` = positional; `(".*", None)` = wildcard.
    pub conns: Vec<(Option<String>, Option<Expr>)>,
    pub loc: Loc,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClockingDecl {
    pub name: String,
    pub event: Vec<EvTerm>,
    /// `(name, is_input, is_output)`.
    pub signals: Vec<(String, bool, bool)>,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModItem {
    Var(VarDecl),
    Param(ParamDecl),
    Initial(Body, Loc),
    Final(Body, Loc),
    Always(AlwaysKind, Body, Loc),
    Assign(Expr, Expr, Loc),
    Inst(Instance),
    Sub(Rc<SubDecl>),
    Typedef(Typedef),
    Import(String, Option<String>),
    Clocking(ClockingDecl),
    Class(Rc<ClassDecl>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleDecl {
    pub name: String,
    pub is_interface: bool,
    pub is_program: bool,
    pub params: Vec<ParamDecl>,
    pub ports: Vec<PortDecl>,
    pub items: Vec<ModItem>,
    pub loc: Loc,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PkgItem {
    Var(VarDecl),
    Param(ParamDecl),
    Sub(Rc<SubDecl>),
    Class(Rc<ClassDecl>),
    Typedef(Typedef),
    Import(String, Option<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    pub name: String,
    pub items: Vec<PkgItem>,
    pub loc: Loc,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceUnit {
    pub modules: Vec<Rc<ModuleDecl>>,
    pub packages: Vec<Rc<Package>>,
    /// `$unit` items (outside any module/package).
    pub unit: Vec<PkgItem>,
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Pos,
    Neg,
    Any,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EvTerm {
    pub edge: Option<Edge>,
    pub e: Expr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinKind {
    All,
    Any,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseKind {
    Case,
    Casez,
    Casex,
}

#[derive(Clone, Debug, PartialEq)]
pub struct S {
    pub loc: Loc,
    pub k: SK,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SK {
    Null,
    Block(Body),
    Decl(VarDecl),
    /// `lhs op rhs` (`op` "=" or compound like "+="); `nb` for `<=`.
    Assign { lhs: Expr, op: &'static str, rhs: Expr, nb: bool },
    IncDec { e: Expr, inc: bool },
    Expr(Expr),
    If { cond: Expr, then: Body, els: Option<Body> },
    Case { kind: CaseKind, e: Expr, inside: bool, items: Vec<(Vec<InsideItem>, Body)>, default: Option<Body> },
    For { init: Body, cond: Option<Expr>, step: Body, body: Body },
    Foreach { arr: Expr, vars: Vec<Option<String>>, body: Body },
    While(Expr, Body),
    DoWhile(Body, Expr),
    Repeat(Expr, Body),
    Forever(Body),
    Fork { kind: JoinKind, decls: Body, branches: Vec<Body> },
    Delay(Expr, Body),
    Event(Vec<EvTerm>, Body),
    Wait(Expr, Body),
    WaitFork,
    DisableFork,
    Disable(String),
    Trigger(Expr),
    Return(Option<Expr>),
    Break,
    Continue,
    Assert { cond: Expr, pass: Option<Body>, fail: Option<Body> },
    RandCase(Vec<(Expr, Body)>),
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Plus,
    Not,
    BitNot,
    RedAnd,
    RedOr,
    RedXor,
    RedNand,
    RedNor,
    RedXnor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    And,
    Or,
    Xor,
    Xnor,
    LAnd,
    LOr,
    Shl,
    Shr,
    AShl,
    AShr,
    Eq,
    Ne,
    CaseEq,
    CaseNe,
    WildEq,
    WildNe,
    Lt,
    Le,
    Gt,
    Ge,
    Implies,
    Equiv,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InsideItem {
    One(Expr),
    Range(Expr, Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliceKind {
    Fixed,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WithClause {
    Constraints(Vec<CItem>),
    Expr(Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum CastTo {
    Type(DataType),
    Width(Box<Expr>),
    Signed(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num { val: u64, width: Option<u32>, signed: bool },
    Fill(u8),
    Real(f64),
    Time(u64),
    Str(String),
    Null,
    This,
    Super,
    Dollar,
    Ident(String),
    /// `a::b::c` (segments may carry `#(…)` parameters).
    Scoped(Vec<Seg>),
    Member(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Slice(Box<Expr>, Box<Expr>, Box<Expr>, SliceKind),
    Call { f: Box<Expr>, args: Vec<Option<Expr>>, named: Vec<(String, Option<Expr>)>, with: Option<WithClause>, loc: Loc },
    Sys { name: String, args: Vec<Option<Expr>>, loc: Loc },
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Inside(Box<Expr>, Vec<InsideItem>),
    Concat(Vec<Expr>),
    Repl(Box<Expr>, Vec<Expr>),
    /// `new(args)`, `new[n]`, `new[n](src)`, `new obj` (shallow copy).
    New { args: Vec<Option<Expr>>, size: Option<Box<Expr>>, src: Option<Box<Expr>>, copy: Option<Box<Expr>>, loc: Loc },
    Cast(CastTo, Box<Expr>),
    /// `'{a, b}` / `'{default: v}`.
    Pattern(Vec<Expr>, Option<Box<Expr>>),
    /// Prefix/postfix `++` / `--` used as an expression.
    IncDec { e: Box<Expr>, inc: bool, prefix: bool },
    /// Assignment used as an expression: `(x = y)`.
    AssignExpr(Box<Expr>, Box<Expr>),
    /// A data type used where an expression is expected (`$bits(int)`).
    Type(DataType),
}

impl Expr {
    pub fn num(v: u64) -> Self {
        Expr::Num { val: v, width: None, signed: true }
    }
}
