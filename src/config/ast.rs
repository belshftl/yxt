// SPDX-License-Identifier: MIT

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub(crate) usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCtx {
    pub file: FileId,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub ctx: LineCtx,
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn at(ctx: LineCtx, pos: usize) -> Self {
        Self {
            ctx,
            start: pos,
            end: pos,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Bool(bool),
    Int(i32),
    String(String),
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairSide {
    Unshifted,
    Shifted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Ident {
        name: String,
        span: Span,
    },
    Literal {
        value: Literal,
        span: Span,
    },
    Call {
        name: String,
        args: Vec<Expr>,
        span: Span,
    },
    Pair {
        unshifted: Box<Expr>,
        shifted: Box<Expr>,
        span: Span,
    },
    InferPair {
        known: Box<Expr>,
        side: PairSide,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Ident { span, .. }
            | Expr::Literal { span, .. }
            | Expr::Call { span, .. }
            | Expr::Pair { span, .. }
            | Expr::InferPair { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingAttr {
    pub name: String,
    pub args: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingOp {
    Right, // =>
    Left,  // <=
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    Directive {
        name: String,
        args: Vec<Expr>,
        span: Span,
    },
    Definition {
        kind: String,
        args: Vec<Expr>,
        span: Span,
    },
    Mapping {
        attrs: Vec<MappingAttr>,
        lhs: Expr,
        op: MappingOp,
        rhs: Expr,
        span: Span,
    },
    OptionAssignment {
        name: String,
        val: Literal,
        span: Span,
    },
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Directive { span, .. }
            | Stmt::Definition { span, .. }
            | Stmt::Mapping { span, .. }
            | Stmt::OptionAssignment { span, .. } => *span,
        }
    }
}
