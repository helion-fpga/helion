//! Syntax tree of the synthesizable SystemVerilog subset.

use super::lex::Num;

#[derive(Clone, Debug)]
pub enum Expr {
    Num(Num),
    Str(String),
    /// `a`, `pkg::a`
    Id(Vec<String>),
    Member(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// `a[hi:lo]`
    Range(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `a[base +: w]` (`up`) or `a[base -: w]`
    PlusRange(Box<Expr>, Box<Expr>, Box<Expr>, bool),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// MSB first.
    Concat(Vec<Expr>),
    Repl(Box<Expr>, Vec<Expr>),
    Call(Vec<String>, Vec<Expr>),
    SysCall(String, Vec<Expr>),
    Cast(Box<CastTo>, Box<Expr>),
    /// `'{...}`
    Pattern(Vec<(PatKey, Expr)>),
    Inside(Box<Expr>, Vec<InsideItem>),
    /// A data type where an expression is expected (`$bits(logic [3:0])`).
    Type(Box<DataType>),
}

#[derive(Clone, Debug)]
pub enum CastTo {
    Type(DataType),
    /// `N'(e)`
    Width(Expr),
    Signed,
    Unsigned,
}

#[derive(Clone, Debug)]
pub enum PatKey {
    Default,
    /// Positional.
    None,
    /// Member name or index expression.
    Key(Expr),
    /// `{n{value}}` inside a pattern.
    Repl(Expr),
}

#[derive(Clone, Debug)]
pub enum InsideItem {
    Value(Expr),
    Range(Expr, Expr),
}

#[derive(Clone, Debug)]
pub struct Dim(pub Expr, pub Expr);

#[derive(Clone, Debug)]
pub enum DataType {
    /// `logic`, `reg`, `wire`, `bit` with packed dimensions (outermost first).
    Vec { signed: bool, dims: Vec<Dim> },
    /// `int`, `integer`, `byte`, ...
    Int { width: u32, signed: bool },
    Named { path: Vec<String>, dims: Vec<Dim> },
    Enum { base: Option<Box<DataType>>, items: Vec<(String, Option<Expr>)>, dims: Vec<Dim> },
    Struct { fields: Vec<(DataType, String)>, dims: Vec<Dim>, signed: bool, union: bool },
    /// `string`: only as the type of a constant.
    Str,
}

#[derive(Clone, Debug)]
pub struct Declarator {
    pub name: String,
    /// Unpacked dimensions; a single expression is a size (`[N]` = `[0:N-1]`).
    pub unpacked: Vec<(Expr, Option<Expr>)>,
    pub init: Option<Expr>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    In,
    Out,
    Inout,
}

#[derive(Clone, Debug)]
pub struct PortDecl {
    pub dir: Dir,
    pub ty: DataType,
    pub decl: Declarator,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct ParamDecl {
    pub local: bool,
    pub ty: Option<DataType>,
    pub name: String,
    pub unpacked: Vec<(Expr, Option<Expr>)>,
    pub value: Option<Expr>,
    /// `parameter type T = ...`
    pub type_value: Option<DataType>,
    pub is_type: bool,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub enum Sens {
    Star,
    /// (posedge, signal)
    Edges(Vec<(bool, Expr)>),
    Levels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlwaysKind {
    Ff,
    Comb,
    Latch,
    Plain,
}

#[derive(Clone, Debug)]
pub struct Conn {
    /// `None`: positional.
    pub port: Option<String>,
    /// `None`: left open (`.p()`).
    pub expr: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct InstDecl {
    pub module: String,
    pub params: Vec<(Option<String>, ParamArg)>,
    pub name: String,
    pub conns: Vec<Conn>,
    /// `.*`
    pub wildcard: bool,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub enum ParamArg {
    Expr(Expr),
    Type(DataType),
}

#[derive(Clone, Debug)]
pub struct FuncDecl {
    pub name: String,
    pub ret: Option<DataType>,
    pub args: Vec<PortDecl>,
    pub decls: Vec<Item>,
    pub body: Vec<Stmt>,
    pub is_task: bool,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct GenBlock {
    pub label: Option<String>,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Decl { ty: DataType, names: Vec<Declarator>, line: u32 },
    Param(ParamDecl),
    Typedef { name: String, ty: DataType, unpacked: Vec<(Expr, Option<Expr>)>, line: u32 },
    Assign { lhs: Expr, rhs: Expr, line: u32 },
    Always { kind: AlwaysKind, sens: Sens, body: Stmt, line: u32 },
    Inst(InstDecl),
    GenFor { var: String, init: Expr, cond: Expr, step: Stmt, body: GenBlock, line: u32 },
    GenIf { cond: Expr, then: GenBlock, els: Option<GenBlock>, line: u32 },
    GenCase { expr: Expr, arms: Vec<(Vec<Expr>, GenBlock)>, line: u32 },
    Block(GenBlock),
    Genvar(Vec<String>),
    Func(std::rc::Rc<FuncDecl>),
    Import { pkg: String, name: Option<String> },
    /// Non-ANSI port direction declaration in the body.
    PortDir(PortDecl),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseKind {
    Case,
    Casez,
    Casex,
    Inside,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Block { decls: Vec<Item>, stmts: Vec<Stmt> },
    Assign { lhs: Expr, rhs: Expr, nonblocking: bool, line: u32 },
    If { cond: Expr, then: Box<Stmt>, els: Option<Box<Stmt>>, line: u32 },
    /// `full`: the author states that one arm always matches (`unique`,
    /// `priority`, `full_case`), so no value falls through.
    Case { kind: CaseKind, expr: Expr, arms: Vec<(Vec<InsideItem>, Stmt)>, full: bool, line: u32 },
    For { init: Vec<Stmt>, decls: Vec<Item>, cond: Expr, step: Vec<Stmt>, body: Box<Stmt>, line: u32 },
    While { cond: Expr, body: Box<Stmt>, line: u32 },
    DoWhile { cond: Expr, body: Box<Stmt>, line: u32 },
    Repeat { count: Expr, body: Box<Stmt>, line: u32 },
    Return(Option<Expr>, u32),
    Break(u32),
    Continue(u32),
    /// A call evaluated for its effect (a void function or task).
    Call(Expr, u32),
    Null,
}

#[derive(Clone, Debug)]
pub struct Module {
    pub name: String,
    pub params: Vec<ParamDecl>,
    pub ports: Vec<PortDecl>,
    /// Non-ANSI header: port names only; directions come from the body.
    pub port_names: Vec<String>,
    pub items: Vec<Item>,
    pub is_interface: bool,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default)]
pub struct Source {
    pub modules: Vec<Module>,
    pub packages: Vec<Package>,
    /// Items in the compilation-unit scope (`$unit`).
    pub unit: Vec<Item>,
    /// Modules that could not be parsed: (name, message). Using one is an error.
    pub broken: Vec<(String, String)>,
}
