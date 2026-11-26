//! VHDL abstract syntax (synthesis subset). Identifiers are lowercased.

pub type Line = u32;

#[derive(Clone, Debug)]
pub enum Expr {
    Name(Box<Name>),
    Int(i128),
    Real(f64),
    Char(char),
    Str(String),
    BitStr(String),
    /// Physical literal (`10 ns`): value and unit; only legal in ignored contexts.
    Phys(f64, String),
    Agg(Vec<AggElem>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Un(UnOp, Box<Expr>),
    Qual(Box<Name>, Box<Expr>),
    Open,
    Null,
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
    MEq,
    MNe,
    MLt,
    MLe,
    MGt,
    MGe,
    Sll,
    Srl,
    Sla,
    Sra,
    Rol,
    Ror,
    Add,
    Sub,
    Cat,
    Mul,
    Div,
    Mod,
    Rem,
    Pow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Pos,
    Not,
    Abs,
    /// VHDL-2008 unary reduction operators.
    RAnd,
    ROr,
    RXor,
    RNand,
    RNor,
    RXnor,
    /// `??` condition operator.
    Cond,
}

#[derive(Clone, Debug)]
pub struct AggElem {
    /// Empty for a positional element.
    pub choices: Vec<Choice>,
    pub val: Expr,
}

#[derive(Clone, Debug)]
pub enum Choice {
    Expr(Expr),
    Range(Range),
    Others,
}

#[derive(Clone, Debug)]
pub enum Name {
    Id(String, Line),
    Sel(Box<Name>, String),
    /// Index, function call, type conversion or (when the argument is a
    /// range) a slice; resolved during elaboration.
    Call(Box<Name>, Vec<Assoc>),
    Slice(Box<Name>, Range),
    Attr(Box<Name>, String, Option<Box<Expr>>),
    /// `'0'` used as a name prefix (e.g. in a selected/qualified context) or
    /// an operator symbol name such as `"and"`.
    OpSym(String),
}

impl Name {
    pub fn line(&self) -> Line {
        match self {
            Name::Id(_, l) => *l,
            Name::Sel(p, _) | Name::Call(p, _) | Name::Slice(p, _) | Name::Attr(p, _, _) => {
                p.line()
            }
            Name::OpSym(_) => 0,
        }
    }
    pub fn simple(&self) -> Option<&str> {
        match self {
            Name::Id(s, _) => Some(s),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Assoc {
    pub formal: Option<Name>,
    pub actual: Expr,
}

#[derive(Clone, Debug)]
pub enum Range {
    Dir(Box<Expr>, bool /* downto */, Box<Expr>),
    /// `x'range`, `x'reverse_range`, or a (sub)type name used as a range.
    Name(Box<Name>),
}

#[derive(Clone, Debug)]
pub enum DiscRange {
    Range(Range),
    Sub(SubInd),
}

#[derive(Clone, Debug)]
pub struct SubInd {
    pub mark: Name,
    pub cons: Option<Constraint>,
    pub line: Line,
}

#[derive(Clone, Debug)]
pub enum Constraint {
    Range(Range),
    /// Index constraint; one entry per dimension, then optional element
    /// constraints (VHDL-2008 `t(open)(7 downto 0)`), `None` for `open`.
    Index(Vec<Option<DiscRange>>, Vec<Vec<Option<DiscRange>>>),
}

#[derive(Clone, Debug)]
pub enum EnumLit {
    Id(String),
    Char(char),
}

#[derive(Clone, Debug)]
pub enum TypeDef {
    Enum(Vec<EnumLit>),
    Int(Range),
    Real(Range),
    Array { idx: Vec<IdxDef>, elem: SubInd },
    Record(Vec<(Vec<String>, SubInd)>),
    Unsupported(String),
}

#[derive(Clone, Debug)]
pub enum IdxDef {
    Unbounded(Name),
    Cons(DiscRange),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    In,
    Out,
    InOut,
    Buffer,
    Linkage,
}

#[derive(Clone, Debug)]
pub struct IfDecl {
    /// (lowercased, original spelling)
    pub names: Vec<(String, String)>,
    pub class: Option<String>,
    pub mode: Mode,
    pub ty: SubInd,
    pub default: Option<Expr>,
    pub line: Line,
}

#[derive(Clone, Debug)]
pub struct Subprog {
    pub name: String,
    pub is_func: bool,
    pub params: Vec<IfDecl>,
    pub ret: Option<Name>,
    pub decls: Vec<Decl>,
    pub body: Option<Vec<Seq>>,
    pub line: Line,
}

#[derive(Clone, Debug)]
pub enum Decl {
    Signal {
        names: Vec<(String, String)>,
        ty: SubInd,
        init: Option<Expr>,
        line: Line,
    },
    Constant {
        names: Vec<(String, String)>,
        ty: SubInd,
        val: Option<Expr>,
        line: Line,
    },
    Variable {
        names: Vec<(String, String)>,
        ty: SubInd,
        init: Option<Expr>,
        shared: bool,
        line: Line,
    },
    Type {
        name: String,
        def: TypeDef,
        line: Line,
    },
    Subtype {
        name: String,
        ind: SubInd,
        line: Line,
    },
    Component {
        name: String,
        generics: Vec<IfDecl>,
        ports: Vec<IfDecl>,
        line: Line,
    },
    Subprog(Subprog),
    Alias {
        name: String,
        ty: Option<SubInd>,
        target: Name,
        line: Line,
    },
    /// Attribute declarations/specifications, use clauses, file declarations
    /// and anything else that does not affect the netlist.
    Ignored {
        what: String,
        line: Line,
    },
    /// `file f : text open read_mode is "name";` (read mode only).
    File {
        name: String,
        path: Expr,
        line: Line,
    },
    /// A declaration Helion cannot represent; an error only if it is used.
    Unsupported {
        name: Option<String>,
        what: String,
        line: Line,
    },
}

#[derive(Clone, Debug)]
pub struct CondVal {
    pub val: Expr,
    pub cond: Option<Expr>,
}

#[derive(Clone, Debug)]
pub enum Seq {
    SigAsg {
        target: Expr,
        rhs: Vec<CondVal>,
        line: Line,
    },
    VarAsg {
        target: Expr,
        rhs: Vec<CondVal>,
        line: Line,
    },
    SelAsg {
        sel: Expr,
        target: Expr,
        var: bool,
        arms: Vec<(Expr, Vec<Choice>)>,
        line: Line,
    },
    If {
        arms: Vec<(Expr, Vec<Seq>)>,
        els: Option<Vec<Seq>>,
        line: Line,
    },
    Case {
        sel: Expr,
        arms: Vec<(Vec<Choice>, Vec<Seq>)>,
        matching: bool,
        line: Line,
    },
    For {
        label: Option<String>,
        var: String,
        range: DiscRange,
        body: Vec<Seq>,
        line: Line,
    },
    While {
        label: Option<String>,
        cond: Option<Expr>,
        body: Vec<Seq>,
        line: Line,
    },
    Next {
        label: Option<String>,
        cond: Option<Expr>,
        line: Line,
    },
    Exit {
        label: Option<String>,
        cond: Option<Expr>,
        line: Line,
    },
    Return {
        val: Option<Expr>,
        line: Line,
    },
    Wait {
        until: Option<Expr>,
        has_for: bool,
        has_on: bool,
        line: Line,
    },
    Call {
        name: Name,
        line: Line,
    },
    Null,
}

#[derive(Clone, Debug)]
pub enum Sens {
    None,
    All,
    List(Vec<Name>),
}

#[derive(Clone, Debug)]
pub enum InstUnit {
    Entity { name: String, arch: Option<String> },
    Comp(String),
    Config(String),
}

#[derive(Clone, Debug)]
pub enum Conc {
    Process {
        label: Option<String>,
        sens: Sens,
        decls: Vec<Decl>,
        body: Vec<Seq>,
        line: Line,
    },
    CondAsg {
        target: Expr,
        arms: Vec<CondVal>,
        line: Line,
    },
    SelAsg {
        sel: Expr,
        target: Expr,
        arms: Vec<(Expr, Vec<Choice>)>,
        line: Line,
    },
    Inst {
        label: String,
        unit: InstUnit,
        gmap: Vec<Assoc>,
        pmap: Vec<Assoc>,
        line: Line,
    },
    ForGen {
        label: String,
        var: String,
        range: DiscRange,
        decls: Vec<Decl>,
        body: Vec<Conc>,
        line: Line,
    },
    IfGen {
        label: String,
        arms: Vec<(Expr, Vec<Decl>, Vec<Conc>)>,
        els: Option<(Vec<Decl>, Vec<Conc>)>,
        line: Line,
    },
    CaseGen {
        label: String,
        sel: Expr,
        arms: Vec<(Vec<Choice>, Vec<Decl>, Vec<Conc>)>,
        line: Line,
    },
    Block {
        label: String,
        decls: Vec<Decl>,
        body: Vec<Conc>,
        line: Line,
    },
    Call {
        name: Name,
        line: Line,
    },
    Ignored,
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub name: String,
    pub orig: String,
    pub generics: Vec<IfDecl>,
    pub ports: Vec<IfDecl>,
    pub decls: Vec<Decl>,
    pub line: Line,
}

#[derive(Clone, Debug)]
pub struct Arch {
    pub name: String,
    pub entity: String,
    pub decls: Vec<Decl>,
    pub body: Vec<Conc>,
    pub line: Line,
}

#[derive(Clone, Debug)]
pub enum Unit {
    Entity(Entity),
    Arch(Arch),
    Package { name: String, decls: Vec<Decl> },
    PackageBody { name: String, decls: Vec<Decl> },
}

#[derive(Clone, Debug)]
pub struct DesignUnit {
    /// Lowercased `lib.pkg` names from the context clause (`use` items).
    pub uses: Vec<String>,
    pub unit: Unit,
}
