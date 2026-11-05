//! Le programme tel qu'il est, sans exécuteur.
//!
//! L'AST est volontairement **petit** : il ne peut pas représenter une boucle, ni
//! une allocation implicite, ni un appel réseau. C'est ce qui rend un module
//! généré à la volée exécutable sans crainte — on ne désactive pas les dangers,
//! on les rend inexprimables.
//!
//! Un programme est **une fonction** : des paramètres typés, des liaisons
//! `let`, et une expression finale qui est le résultat. Pas de `return` : le
//! langage est orienté expression, ce qui rend l'émission wasm directe (chaque
//! expression pousse une valeur sur la pile).

/// Les types du langage. Rien d'autre : pas de flottant, pas de pointeur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Int,
    Bool,
    /// Une chaîne n'existe qu'en littéral, et seulement comme argument d'hôte :
    /// la manipuler demande un modèle mémoire, qui viendra avec les
    /// enregistrements.
    Str,
    Unit,
}

impl Type {
    pub fn name(self) -> &'static str {
        match self {
            Type::Int => "Int",
            Type::Bool => "Bool",
            Type::Str => "Str",
            Type::Unit => "Unit",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Int(i64),
    Bool(bool),
    Str(String),
    Var(String),
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    Call {
        name: String,
        args: Vec<Expr>,
    },
    /// Des expressions évaluées dans l'ordre ; seule la dernière rend une
    /// valeur. Les précédentes doivent être `Unit` : sans mutation, une valeur
    /// jetée n'aurait aucun sens.
    Seq(Vec<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub name: String,
    pub ty: Type,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: Vec<Param>,
    pub returns: Type,
    /// Les `let`, dans l'ordre. Pas de portée imbriquée en v1 : une liaison vit
    /// jusqu'à la fin du corps, ce qui suffit et évite les pièges d'ombre.
    pub bindings: Vec<Binding>,
    pub body: Expr,
}

/// Une position dans la source, pour que toute erreur soit localisable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

pub type SpannedExpr = Spanned<Expr>;
