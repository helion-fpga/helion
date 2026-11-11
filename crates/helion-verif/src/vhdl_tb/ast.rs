//! Syntax tree for the VHDL simulation subset.

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Int(i64),
    Real(f64),
    Char(u8),
    Str(String),
    BitStr(String),
    /// Physical literal (`10 ns`): value in femtoseconds.
    Time(i64),
    Name(String),
    /// `prefix.field` (record element or expanded name such as `std.env.stop`).
    Select(Box<Expr>, String),
    /// `prefix(args)`: function call, index, slice, or type conversion.
    Call(Box<Expr>, Vec<Arg>),
    /// `prefix'attr` with optional `(args)`.
    Attr(Box<Expr>, String, Vec<Expr>),
    /// `type'(expr)`.
    Qualified(String, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Aggregate(Vec<Assoc>),
    /// A discrete range used as an argument (`x(7 downto 0)`).
    Range(Box<Expr>, bool, Box<Expr>),
    /// `open` in a port map.
    Open,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub formal: Option<String>,
    pub actual: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    Expr(Expr),
    Range(Expr, bool, Expr),
    Others,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Assoc {
    pub choices: Vec<Choice>,
    pub value: Expr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Plus,
    Not,
    Abs,
    /// VHDL-2008 condition operator `??`.
    Cond,
    /// Reduction operators (`and x`, `or x`, `xor x`).
    RedAnd,
    RedOr,
    RedXor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    And,
    Or,
    Nand,
    Nor,
    Xor,
    Xnor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Sll,
    Srl,
    Sla,
    Sra,
    Rol,
    Ror,
    Add,
    Sub,
    Concat,
    Mul,
    Div,
    Mod,
    Rem,
    Pow,
}

/// Subtype indication: a type mark with an optional constraint.
#[derive(Clone, Debug, PartialEq)]
pub struct SubtypeInd {
    pub mark: String,
    /// `(7 downto 0)` / `range 0 to 15` / record element constraints are
    /// simplified to one optional discrete range.
    pub range: Option<(Expr, bool, Expr)>,
    /// Constrained element subtype for `array_t(0 to 3)(7 downto 0)` style;
    /// rarely used.
    pub elem_range: Option<(Expr, bool, Expr)>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeDef {
    Enum(Vec<String>),
    Int(Expr, bool, Expr),
    Real,
    /// `array (idx range) of elem`; `None` range means unconstrained `<>`.
    Array {
        range: Option<(Expr, bool, Expr)>,
        /// Index type mark for `array (natural range <>)` / enumerated index.
        index_mark: Option<String>,
        elem: SubtypeInd,
    },
    Record(Vec<(String, SubtypeInd)>),
    /// `access` / `file` / protected types: declared, not simulated.
    Opaque,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ObjClass {
    Signal,
    Variable,
    Constant,
    File,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decl {
    Object {
        class: ObjClass,
        names: Vec<String>,
        ty: SubtypeInd,
        init: Option<Expr>,
        line: usize,
    },
    Type {
        name: String,
        def: TypeDef,
        line: usize,
    },
    Subtype {
        name: String,
        ty: SubtypeInd,
        line: usize,
    },
    Subprogram(Box<Subprogram>),
    Alias {
        name: String,
        target: Expr,
        line: usize,
    },
    /// Component declarations, attributes, use clauses inside regions:
    /// accepted and ignored.
    Ignored,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    In,
    Out,
    InOut,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub class: Option<ObjClass>,
    pub name: String,
    pub mode: Mode,
    pub ty: SubtypeInd,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Subprogram {
    pub name: String,
    pub is_function: bool,
    pub params: Vec<Param>,
    pub ret: Option<SubtypeInd>,
    pub decls: Vec<Decl>,
    /// `None` for a declaration without a body.
    pub body: Option<Vec<Stmt>>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Waveform {
    /// `(value, after)`; `unaffected` is an empty list.
    pub elems: Vec<(Expr, Option<Expr>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    SigAssign {
        target: Expr,
        /// `(waveform, condition)`; a plain assignment has one arm with no
        /// condition; `a when c else b` has several.
        arms: Vec<(Waveform, Option<Expr>)>,
        transport: bool,
        line: usize,
    },
    VarAssign {
        target: Expr,
        arms: Vec<(Expr, Option<Expr>)>,
        line: usize,
    },
    If {
        arms: Vec<(Expr, Vec<Stmt>)>,
        else_b: Vec<Stmt>,
        line: usize,
    },
    Case {
        sel: Expr,
        arms: Vec<(Vec<Choice>, Vec<Stmt>)>,
        line: usize,
    },
    Loop {
        label: Option<String>,
        kind: LoopKind,
        body: Vec<Stmt>,
        line: usize,
    },
    Exit {
        label: Option<String>,
        cond: Option<Expr>,
        line: usize,
    },
    Next {
        label: Option<String>,
        cond: Option<Expr>,
        line: usize,
    },
    Wait {
        on: Vec<Expr>,
        until: Option<Expr>,
        timeout: Option<Expr>,
        line: usize,
    },
    Report {
        msg: Expr,
        severity: Option<Expr>,
        line: usize,
    },
    Assert {
        cond: Expr,
        msg: Option<Expr>,
        severity: Option<Expr>,
        line: usize,
    },
    Call {
        name: Expr,
        line: usize,
    },
    Return {
        value: Option<Expr>,
        line: usize,
    },
    Null,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LoopKind {
    Forever,
    While(Expr),
    For(String, ForRange),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ForRange {
    Range(Expr, bool, Expr),
    /// `x'range` / `x'reverse_range` / a subtype name.
    Attr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Concurrent {
    Process {
        label: Option<String>,
        /// `None` = no sensitivity list; `Some(vec![])` with `all` = true.
        sens: Option<Vec<Expr>>,
        sens_all: bool,
        decls: Vec<Decl>,
        body: Vec<Stmt>,
        line: usize,
    },
    /// Concurrent signal assignment (incl. conditional `when/else`).
    Assign {
        target: Expr,
        arms: Vec<(Waveform, Option<Expr>)>,
        line: usize,
    },
    /// `with sel select target <= w when c, ...;`
    Select {
        sel: Expr,
        target: Expr,
        arms: Vec<(Waveform, Vec<Choice>)>,
        line: usize,
    },
    Assert {
        cond: Expr,
        msg: Option<Expr>,
        severity: Option<Expr>,
        line: usize,
    },
    Instance {
        label: String,
        /// Entity or component name (library prefix stripped).
        unit: String,
        generics: Vec<Arg>,
        ports: Vec<Arg>,
        line: usize,
    },
    /// Procedure call as a concurrent statement.
    Call {
        name: Expr,
        line: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PortDecl {
    pub name: String,
    pub mode: Mode,
    pub ty: SubtypeInd,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenericDecl {
    pub name: String,
    pub ty: SubtypeInd,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    pub name: String,
    pub generics: Vec<GenericDecl>,
    pub ports: Vec<PortDecl>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Architecture {
    pub name: String,
    pub entity: String,
    pub decls: Vec<Decl>,
    pub stmts: Vec<Concurrent>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    pub name: String,
    pub decls: Vec<Decl>,
    pub is_body: bool,
}

/// One parsed design file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesignFile {
    pub entities: Vec<Entity>,
    pub archs: Vec<Architecture>,
    pub packages: Vec<Package>,
    /// `use work.<pkg>.all` names referenced by the file.
    pub uses: Vec<String>,
}
