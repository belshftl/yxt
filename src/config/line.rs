// SPDX-License-Identifier: MIT

use std::borrow::Cow;

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
    fn at(ctx: LineCtx, pos: usize) -> Self {
        Self {
            ctx,
            start: pos,
            end: pos,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingOp {
    Right, // =>
    Left,  // <=
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Bool(bool),
    Int(i32),
    String(String),
    Char(char),
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
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Ident { span, .. }
            | Expr::Literal { span, .. }
            | Expr::Call { span, .. }
            | Expr::Pair { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingAttr {
    pub name: String,
    pub args: Vec<Expr>,
    pub span: Span,
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

#[derive(Debug, Clone, thiserror::Error)]
pub enum ErrorKind {
    #[error("expected {0}")]
    Expected(&'static str),

    #[error("unknown statement")]
    UnknownStatement,

    #[error("unterminated string")]
    UnterminatedString,

    #[error("unterminated character literal")]
    UnterminatedChar,

    #[error("empty character literal")]
    EmptyChar,

    #[error("character literal contains more than one character")]
    CharTooLong,

    #[error("invalid escape sequence \\{}", .0.escape_default())]
    InvalidEscape(char),

    #[error("invalid hex escape")]
    InvalidHexEscape,

    #[error("invalid unicode escape")]
    InvalidUnicodeEscape,

    #[error("invalid identifier")]
    InvalidIdentifier,

    #[error("invalid integer")]
    InvalidInteger,

    #[error("integer out of range")]
    IntegerOutOfRange,

    #[error("trailing input")]
    TrailingInput,

    #[error("multiple mapping operators")]
    MultipleMappingOperators,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{kind} at byte {}..{}", .span.start, .span.end)]
pub struct ParseError {
    pub kind: ErrorKind,
    pub span: Span,
}

pub fn parse_line(line: &str, ctx: LineCtx) -> Result<Option<Stmt>, ParseError> {
    let line = strip_line_ending(line, ctx)?;
    let st = scan_line_structure(line, ctx, 0)?;
    let content = &line[..st.content_end];
    let (s, base) = trim_ascii_span(content);

    if s.is_empty() {
        Ok(None)
    } else if s.starts_with('@') {
        parse_directive(s, ctx, base).map(Some)
    } else if starts_with_keyword(s, "define") {
        parse_definition(s, ctx, base).map(Some)
    } else if let Some((pos, op)) = st.mapping_op {
        debug_assert!(pos >= base && pos < base + s.len());
        let rel = pos - base;

        let (attrs, lhs) = parse_mapping_lhs_full(&s[..rel], ctx, base)?;
        let rhs = parse_expr_full(&s[rel + 2..], ctx, pos + 2)?;

        Ok(Some(Stmt::Mapping {
            attrs,
            lhs,
            op,
            rhs,
            span: Span { ctx, start: base, end: base + s.len() },
        }))
    } else if let Some(pos) = st.assignment_eq {
        debug_assert!(pos >= base && pos < base + s.len());
        let rel = pos - base;

        let (name_str, name_off) = trim_ascii_span(&s[..rel]);
        let name = parse_ident_full(name_str, ctx, base + name_off)?;

        let (val_str, val_off) = trim_ascii_span(&s[rel + 1..]);
        let val = parse_literal_full(val_str, ctx, base + rel + 1 + val_off)?;

        Ok(Some(Stmt::OptionAssignment {
            name,
            val,
            span: Span { ctx, start: base, end: base + s.len() },
        }))
    } else {
        Err(ParseError {
            kind: ErrorKind::UnknownStatement,
            span: Span { ctx, start: base, end: base + s.len() },
        })
    }
}

struct Cursor<'a> {
    s: &'a str,
    ctx: LineCtx,
    base: usize,
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(s: &'a str, ctx: LineCtx, base: usize) -> Self {
        Self {
            s,
            ctx,
            base,
            pos: 0,
        }
    }

    fn eof(&self) -> bool {
        self.pos >= self.s.len()
    }

    fn remaining(&self) -> &'a str {
        &self.s[self.pos..]
    }

    fn peek_byte(&self) -> Option<u8> {
        self.s.as_bytes().get(self.pos).copied()
    }

    fn cons_byte(&mut self) -> Option<u8> {
        let b = self.peek_byte()?;
        self.pos += 1;
        Some(b)
    }

    fn span_here(&self) -> Span {
        Span::at(self.ctx, self.base + self.pos)
    }

    fn span_from(&self, start: usize) -> Span {
        Span {
            ctx: self.ctx,
            start: self.base + start,
            end: self.base + self.pos,
        }
    }

    fn err_here(&self, kind: ErrorKind) -> ParseError {
        ParseError {
            kind,
            span: self.span_here(),
        }
    }

    fn expect_byte(&mut self, expect: u8, what: &'static str) -> Result<(), ParseError> {
        match self.peek_byte() {
            Some(b) if b == expect => {
                self.pos += 1;
                Ok(())
            }
            _ => Err(self.err_here(ErrorKind::Expected(what))),
        }
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek_byte() {
            if !is_ascii_ws(b) {
                break;
            }
            self.pos += 1;
        }
    }

    fn parse_ident(&mut self) -> Result<&'a str, ParseError> {
        let start = self.pos;

        let Some(b) = self.peek_byte() else {
            return Err(self.err_here(ErrorKind::Expected("identifier")));
        };
        if !is_ident_start(b) {
            return Err(self.err_here(ErrorKind::InvalidIdentifier));
        }
        self.pos += 1;

        while let Some(b) = self.peek_byte() {
            if !is_ident_rest(b) {
                break;
            }
            self.pos += 1;
        }

        Ok(&self.s[start..self.pos])
    }

    fn parse_escape_after_backslash(
        &mut self,
        escape_pos: usize,
        literal_start: usize,
        unterminated: ErrorKind,
    ) -> Result<char, ParseError> {
        let Some(esc) = self.cons_byte() else {
            return Err(ParseError {
                kind: unterminated,
                span: Span { ctx: self.ctx, start: self.base + literal_start, end: self.base + self.pos },
            });
        };

        match esc {
            b'\\' => Ok('\\'),
            b'"' => Ok('"'),
            b'\'' => Ok('\''),
            b'n' => Ok('\n'),
            b'r' => Ok('\r'),
            b't' => Ok('\t'),
            b'x' => {
                let start = self.pos;

                let Some(hi_ch) = self.cons_byte() else {
                    return Err(ParseError {
                        kind: ErrorKind::InvalidHexEscape,
                        span: Span { ctx: self.ctx, start: self.base + escape_pos, end: self.base + self.pos },
                    });
                };
                let Some(lo_ch) = self.cons_byte() else {
                    return Err(ParseError {
                        kind: ErrorKind::InvalidHexEscape,
                        span: Span { ctx: self.ctx, start: self.base + escape_pos, end: self.base + self.pos },
                    });
                };
                let Some(hi) = hex_value(hi_ch) else {
                    return Err(ParseError {
                        kind: ErrorKind::InvalidHexEscape,
                        span: Span { ctx: self.ctx, start: self.base + start, end: self.base + start + 1 },
                    });
                };
                let Some(lo) = hex_value(lo_ch) else {
                    return Err(ParseError {
                        kind: ErrorKind::InvalidHexEscape,
                        span: Span { ctx: self.ctx, start: self.base + start + 1, end: self.base + start + 2 },
                    });
                };

                Ok(char::from((hi << 4) | lo))
            }
            b'u' => {
                let escape_body_start = self.pos;

                if self.cons_byte() != Some(b'{') {
                    return Err(ParseError {
                        kind: ErrorKind::InvalidUnicodeEscape,
                        span: Span { ctx: self.ctx, start: self.base + escape_pos, end: self.base + self.pos },
                    });
                }

                let digits_start = self.pos;
                let mut value = 0u32;
                let mut digits = 0usize;

                loop {
                    let Some(b) = self.cons_byte() else {
                        return Err(ParseError {
                            kind: ErrorKind::InvalidUnicodeEscape,
                            span: Span { ctx: self.ctx, start: self.base + escape_body_start, end: self.base + self.pos },
                        });
                    };

                    if b == b'}' {
                        break;
                    }

                    let Some(v) = hex_value(b) else {
                        return Err(ParseError {
                            kind: ErrorKind::InvalidUnicodeEscape,
                            span: Span { ctx: self.ctx, start: self.base + self.pos - 1, end: self.base + self.pos },
                        });
                    };

                    digits += 1;

                    if digits > 6 {
                        return Err(ParseError {
                            kind: ErrorKind::InvalidUnicodeEscape,
                            span: Span { ctx: self.ctx, start: self.base + digits_start, end: self.base + self.pos },
                        });
                    }

                    value = (value << 4) | u32::from(v);
                }

                if digits == 0 {
                    return Err(ParseError {
                        kind: ErrorKind::InvalidUnicodeEscape,
                        span: Span { ctx: self.ctx, start: self.base + digits_start, end: self.base + self.pos },
                    });
                }

                char::from_u32(value).ok_or_else(|| ParseError {
                    kind: ErrorKind::InvalidUnicodeEscape,
                    span: Span { ctx: self.ctx, start: self.base + escape_body_start, end: self.base + self.pos },
                })
            }
            _ => Err(ParseError {
                kind: ErrorKind::InvalidEscape(esc as char),
                span: Span { ctx: self.ctx, start: self.base + escape_pos, end: self.base + escape_pos + 1 },
            }),
        }
    }

    fn parse_strlit(&mut self) -> Result<Cow<'a, str>, ParseError> {
        let quote_start = self.pos;
        self.expect_byte(b'"', "string literal")?;

        let content_start = self.pos;
        let mut tail_start = self.pos;
        let mut out: Option<String> = None;
        while let Some(b) = self.peek_byte() {
            match b {
                b'"' => {
                    let content_end = self.pos;
                    self.pos += 1;
                    if let Some(mut out) = out {
                        out.push_str(&self.s[tail_start..content_end]);
                        return Ok(Cow::Owned(out));
                    }
                    return Ok(Cow::Borrowed(&self.s[content_start..content_end]));
                }
                b'\\' => {
                    let bksl_pos = self.pos;
                    let out = out.get_or_insert_with(String::new);
                    out.push_str(&self.s[tail_start..bksl_pos]);
                    self.pos += 1;

                    let esc_pos = self.pos;
                    let ch = self.parse_escape_after_backslash(
                        esc_pos,
                        quote_start,
                        ErrorKind::UnterminatedString,
                    )?;

                    out.push(ch);
                    tail_start = self.pos;
                }
                _ => self.pos += 1,
            }
        }

        Err(ParseError {
            kind: ErrorKind::UnterminatedString,
            span: Span {
                ctx: self.ctx,
                start: self.base + quote_start,
                end: self.base + self.pos,
            },
        })
    }

    fn parse_charlit(&mut self) -> Result<char, ParseError> {
        let quote_start = self.pos;
        self.expect_byte(b'\'', "char literal")?;

        let ch = match self.peek_byte() {
            None => {
                return Err(ParseError {
                    kind: ErrorKind::UnterminatedChar,
                    span: Span { ctx: self.ctx, start: self.base + quote_start, end: self.base + self.pos },
                });
            }
            Some(b'\'') => {
                return Err(ParseError {
                    kind: ErrorKind::EmptyChar,
                    span: Span {
                        ctx: self.ctx,
                        start: self.base + quote_start,
                        end: self.base + self.pos + 1,
                    },
                });
            }
            Some(b'\\') => {
                self.pos += 1;
                let esc_pos = self.pos;
                self.parse_escape_after_backslash(esc_pos, quote_start, ErrorKind::UnterminatedChar)?
            }
            Some(_) => {
                let Some(ch) = self.remaining().chars().next() else {
                    unreachable!();
                };
                self.pos += ch.len_utf8();
                ch
            }
        };

        match self.peek_byte() {
            Some(b'\'') => {
                self.pos += 1;
                Ok(ch)
            }
            Some(_) => Err(ParseError {
                kind: ErrorKind::CharTooLong,
                span: Span { ctx: self.ctx, start: self.base + quote_start, end: self.base + self.pos },
            }),
            None => Err(ParseError {
                kind: ErrorKind::UnterminatedChar,
                span: Span { ctx: self.ctx, start: self.base + quote_start, end: self.base + self.pos },
            }),
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();
        let lhs = self.parse_expr_atom()?;
        let after_lhs = self.pos;
        self.skip_ws();

        if self.peek_byte() != Some(b'~') {
            self.pos = after_lhs;
            return Ok(lhs);
        }
        self.pos += 1;
        self.skip_ws();

        let rhs = self.parse_expr_atom()?;
        let lhs_span = lhs.span();
        let rhs_span = rhs.span();
        Ok(Expr::Pair {
            unshifted: Box::new(lhs),
            shifted: Box::new(rhs),
            span: Span { ctx: self.ctx, start: lhs_span.start, end: rhs_span.end },
        })
    }

    fn parse_expr_atom(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();
        match self.peek_byte() {
            Some(b'"') => {
                let start = self.pos;
                let value = self.parse_strlit()?.into_owned();
                Ok(Expr::Literal {
                    value: Literal::String(value),
                    span: self.span_from(start),
                })
            }
            Some(b'\'') => {
                let start = self.pos;
                let value = self.parse_charlit()?;
                Ok(Expr::Literal {
                    value: Literal::Char(value),
                    span: self.span_from(start),
                })
            }
            Some(b) if is_ident_start(b) => {
                let start = self.pos;
                let name = self.parse_ident()?.to_owned();
                let after_ident = self.pos;

                self.skip_ws();
                if self.peek_byte() == Some(b'(') {
                    let args = self.parse_call_args_after_open_paren()?;
                    return Ok(Expr::Call { name, args, span: self.span_from(start) });
                }
                self.pos = after_ident;

                match name.as_str() {
                    "true" => Ok(Expr::Literal {
                        value: Literal::Bool(true),
                        span: self.span_from(start),
                    }),
                    "false" => Ok(Expr::Literal {
                        value: Literal::Bool(false),
                        span: self.span_from(start),
                    }),
                    _ => Ok(Expr::Ident {
                        name,
                        span: self.span_from(start),
                    }),
                }
            }
            Some(b'+') | Some(b'-') | Some(b'0'..=b'9') => {
                let start = self.pos;
                while let Some(b) = self.peek_byte() {
                    if is_expr_delim(b) {
                        break;
                    }
                    self.pos += 1;
                }

                let text = &self.s[start..self.pos];
                let span = self.span_from(start);
                let value = parse_i32(text, span)?;
                Ok(Expr::Literal {
                    value: Literal::Int(value),
                    span,
                })
            }
            _ => Err(self.err_here(ErrorKind::Expected("expression"))),
        }
    }

    fn parse_call_args_after_open_paren(&mut self) -> Result<Vec<Expr>, ParseError> {
        self.expect_byte(b'(', "'('")?;

        let mut args = Vec::new();
        loop {
            self.skip_ws();
            if self.peek_byte() == Some(b')') {
                self.cons_byte();
                break;
            }

            args.push(self.parse_expr()?);

            self.skip_ws();
            match self.peek_byte() {
                Some(b',') => _ = self.cons_byte(),
                Some(b')') => {
                    self.cons_byte();
                    break;
                }
                _ => return Err(self.err_here(ErrorKind::Expected("',' or ')'"))),
            }
        }

        Ok(args)
    }

    fn parse_call_expr(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();

        let start = self.pos;
        let name = self.parse_ident()?.to_owned();

        self.skip_ws();

        let args = self.parse_call_args_after_open_paren()?;

        Ok(Expr::Call {
            name,
            args,
            span: self.span_from(start),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TopLevelByte {
    pub pos: usize,
    pub byte: u8,
}

struct TopLevelBytes<'a> {
    s: &'a str,
    ctx: LineCtx,
    base: usize,
    pos: usize,
    quote: Option<u8>,
    quote_start: usize,
    done: bool,
}

impl<'a> TopLevelBytes<'a> {
    fn new(s: &'a str, ctx: LineCtx, base: usize) -> Self {
        Self {
            s,
            ctx,
            base,
            pos: 0,
            quote: None,
            quote_start: 0,
            done: false,
        }
    }

    fn err_unterminated(&self) -> ParseError {
        let kind = match self.quote {
            Some(b'"') => ErrorKind::UnterminatedString,
            Some(b'\'') => ErrorKind::UnterminatedChar,
            _ => panic!("self.quote not set or bad value"),
        };
        ParseError {
            kind,
            span: Span {
                ctx: self.ctx,
                start: self.base + self.quote_start,
                end: self.base + self.s.len(),
            },
        }
    }
}

impl<'a> Iterator for TopLevelBytes<'a> {
    type Item = Result<TopLevelByte, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        let bytes = self.s.as_bytes();
        while self.pos < self.s.len() {
            let b = bytes[self.pos];

            if let Some(quote) = self.quote {
                match b {
                    b if b == quote => {
                        self.quote = None;
                        self.pos += 1;
                    }
                    b'\\' => {
                        self.pos += 1;
                        if self.pos >= self.s.len() {
                            self.done = true;
                            return Some(Err(self.err_unterminated()));
                        }
                        self.pos += 1;
                    }
                    _ => self.pos += 1,
                }
            } else {
                match b {
                    b'"' | b'\'' => {
                        self.quote = Some(b);
                        self.quote_start = self.pos;
                        self.pos += 1;
                    }
                    _ => {
                        let pos = self.pos;
                        self.pos += 1;
                        return Some(Ok(TopLevelByte {
                            pos,
                            byte: b,
                        }));
                    }
                }
            }
        }

        self.done = true;
        if self.quote.is_some() {
            Some(Err(self.err_unterminated()))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineStructure {
    content_end: usize,
    mapping_op: Option<(usize, MappingOp)>,
    assignment_eq: Option<usize>,
}

fn matches_toplv_byte(
    item: Option<&Result<TopLevelByte, ParseError>>,
    pos: usize,
    byte: u8,
) -> Result<bool, ParseError> {
    match item {
        Some(Ok(item)) => Ok(item.pos == pos && item.byte == byte),
        Some(Err(e)) => Err(e.clone()),
        None => Ok(false),
    }
}

fn scan_line_structure(s: &str, ctx: LineCtx, base: usize) -> Result<LineStructure, ParseError> {
    let mut it = TopLevelBytes::new(s, ctx, base).peekable();

    let mut content_end = s.len();
    let mut mapping_op = None;
    let mut assignment_eq = None;
    let mut paren_depth = 0usize;

    while let Some(item) = it.next() {
        let item = item?;

        match item.byte {
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'#' if paren_depth == 0 => {
                content_end = item.pos;
                break;
            }
            b'=' if paren_depth == 0 => {
                if matches_toplv_byte(it.peek(), item.pos + 1, b'>')? {
                    _ = it.next().transpose()?;
                    if mapping_op.replace((item.pos, MappingOp::Right)).is_some() {
                        return Err(ParseError {
                            kind: ErrorKind::MultipleMappingOperators,
                            span: Span { ctx, start: base + item.pos, end: base + item.pos + 2 },
                        });
                    }
                    continue;
                }
                if assignment_eq.is_none() {
                    assignment_eq = Some(item.pos);
                }
            }
            b'<' if paren_depth == 0 => {
                if matches_toplv_byte(it.peek(), item.pos + 1, b'=')? {
                    _ = it.next().transpose()?;

                    if mapping_op.replace((item.pos, MappingOp::Left)).is_some() {
                        return Err(ParseError {
                            kind: ErrorKind::MultipleMappingOperators,
                            span: Span { ctx, start: base + item.pos, end: base + item.pos + 2 },
                        });
                    }
                    continue;
                }
            }
            _ => {}
        }
    }

    Ok(LineStructure { content_end, mapping_op, assignment_eq })
}

fn parse_directive(s: &str, ctx: LineCtx, base: usize) -> Result<Stmt, ParseError> {
    let mut c = Cursor::new(s, ctx, base);

    c.expect_byte(b'@', "'@'")?;
    let name = c.parse_ident()?;
    if !c.eof() && !is_ascii_ws(c.peek_byte().unwrap()) {
        return Err(c.err_here(ErrorKind::Expected("whitespace")));
    }

    let args = parse_ws_exprs(&mut c)?;
    Ok(Stmt::Directive {
        name: name.to_owned(),
        args,
        span: Span { ctx, start: base, end: base + s.len() },
    })
}

fn parse_definition(s: &str, ctx: LineCtx, base: usize) -> Result<Stmt, ParseError> {
    let mut c = Cursor::new(s, ctx, base);
    let kw = c.parse_ident()?;
    debug_assert_eq!(kw, "define");
    if c.eof() || !is_ascii_ws(c.peek_byte().unwrap()) {
        return Err(c.err_here(ErrorKind::Expected("whitespace")));
    }

    c.skip_ws();
    let kind = c.parse_ident()?;
    if !c.eof() && !is_ascii_ws(c.peek_byte().unwrap()) {
        return Err(c.err_here(ErrorKind::Expected("whitespace")));
    }

    let args = parse_ws_exprs(&mut c)?;
    Ok(Stmt::Definition {
        kind: kind.to_owned(),
        args,
        span: Span { ctx, start: base, end: base + s.len() },
    })
}

fn parse_ws_exprs(c: &mut Cursor<'_>) -> Result<Vec<Expr>, ParseError> {
    let mut args = Vec::new();
    loop {
        c.skip_ws();
        if c.eof() {
            break;
        }

        args.push(c.parse_expr()?);
        if !c.eof() && !is_ascii_ws(c.peek_byte().unwrap()) {
            return Err(c.err_here(ErrorKind::Expected("whitespace")));
        }
    }
    Ok(args)
}

fn parse_mapping_lhs_full(
    s: &str,
    ctx: LineCtx,
    base: usize,
) -> Result<(Vec<MappingAttr>, Expr), ParseError> {
    let (s, off) = trim_ascii_span(s);

    if s.is_empty() {
        return Err(ParseError {
            kind: ErrorKind::Expected("expression"),
            span: Span::at(ctx, base + off),
        });
    }

    let mut c = Cursor::new(s, ctx, base + off);
    let attrs = parse_mapping_attrs(&mut c)?;
    let lhs = c.parse_expr()?;

    c.skip_ws();
    if !c.eof() {
        return Err(c.err_here(ErrorKind::TrailingInput));
    }

    Ok((attrs, lhs))
}

fn parse_mapping_attrs(c: &mut Cursor<'_>) -> Result<Vec<MappingAttr>, ParseError> {
    let mut attrs = Vec::new();

    loop {
        c.skip_ws();

        let saved = c.pos;
        let start = c.pos;

        let Ok(name) = c.parse_ident() else {
            c.pos = saved;
            break;
        };

        if c.peek_byte() != Some(b'!') {
            c.pos = saved;
            break;
        }

        let name = name.to_owned();

        c.pos += 1;
        let args = if c.peek_byte() == Some(b'(') {
            c.parse_call_args_after_open_paren()?
        } else {
            Vec::new()
        };

        attrs.push(MappingAttr {
            name,
            args,
            span: c.span_from(start),
        });
    }

    Ok(attrs)
}

fn parse_expr_full(s: &str, ctx: LineCtx, base: usize) -> Result<Expr, ParseError> {
    let (s, off) = trim_ascii_span(s);

    if s.is_empty() {
        return Err(ParseError {
            kind: ErrorKind::Expected("expression"),
            span: Span::at(ctx, base + off),
        });
    }

    let mut c = Cursor::new(s, ctx, base + off);
    let expr = c.parse_expr()?;

    c.skip_ws();
    if !c.eof() {
        return Err(c.err_here(ErrorKind::TrailingInput));
    }

    Ok(expr)
}

fn parse_literal_full(s: &str, ctx: LineCtx, base: usize) -> Result<Literal, ParseError> {
    if s.is_empty() {
        return Err(ParseError {
            kind: ErrorKind::Expected("literal"),
            span: Span::at(ctx, base),
        });
    }

    let mut c = Cursor::new(s, ctx, base);

    if c.peek_byte() == Some(b'"') {
        let v = c.parse_strlit()?;
        c.skip_ws();
        if !c.eof() {
            return Err(c.err_here(ErrorKind::TrailingInput));
        }
        return Ok(Literal::String(v.into_owned()));
    }

    if c.peek_byte() == Some(b'\'') {
        let v = c.parse_charlit()?;
        c.skip_ws();
        if !c.eof() {
            return Err(c.err_here(ErrorKind::TrailingInput));
        }
        return Ok(Literal::Char(v));
    }

    if s == "true" {
        return Ok(Literal::Bool(true));
    } else if s == "false" {
        return Ok(Literal::Bool(false));
    }

    parse_i32(s, Span { ctx, start: base, end: base + s.len() }).map(Literal::Int)
}

fn parse_ident_full(s: &str, ctx: LineCtx, base: usize) -> Result<String, ParseError> {
    if s.is_empty() {
        return Err(ParseError {
            kind: ErrorKind::Expected("identifier"),
            span: Span::at(ctx, base),
        });
    }

    let mut c = Cursor::new(s, ctx, base);
    let ident = c.parse_ident()?;
    if !c.eof() {
        return Err(c.err_here(ErrorKind::TrailingInput));
    }

    Ok(ident.to_owned())
}

fn parse_i32(s: &str, span: Span) -> Result<i32, ParseError> {
    let mut s = s;
    let negative = match s.as_bytes().first() {
        Some(b'+') => {
            s = &s[1..];
            false
        }
        Some(b'-') => {
            s = &s[1..];
            true
        }
        _ => false,
    };

    if s.is_empty() {
        return Err(ParseError { kind: ErrorKind::InvalidInteger, span });
    }

    let (digits, radix) = if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ParseError { kind: ErrorKind::InvalidInteger, span });
        }
        (rest, 16)
    } else if s == "0" {
        ("0", 10)
    } else if let Some(rest) = s.strip_prefix('0') {
        if rest.is_empty() {
            ("0", 10)
        } else {
            if rest.as_bytes()[0] == b'0' || !rest.bytes().all(|b| matches!(b, b'0'..=b'7')) {
                return Err(ParseError { kind: ErrorKind::InvalidInteger, span });
            }
            (s, 8)
        }
    } else {
        if s.as_bytes()[0] == b'0' || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseError { kind: ErrorKind::InvalidInteger, span });
        }
        (s, 10)
    };

    let mag = i64::from_str_radix(digits, radix).map_err(|_| ParseError {
        kind: ErrorKind::IntegerOutOfRange,
        span,
    })?;

    let v = if negative { -mag } else { mag };
    if v < i32::MIN as i64 || v > i32::MAX as i64 {
        return Err(ParseError { kind: ErrorKind::IntegerOutOfRange, span });
    }
    Ok(v as i32)
}

fn strip_line_ending(s: &str, ctx: LineCtx) -> Result<&str, ParseError> {
    if let Some(pos) = s.find('\n') {
        if pos + 1 != s.len() {
            return Err(ParseError {
                kind: ErrorKind::TrailingInput,
                span: Span { ctx, start: pos + 1, end: s.len() },
            });
        }

        let before_lf = &s[..pos];
        return Ok(before_lf.strip_suffix('\r').unwrap_or(before_lf));
    }
    Ok(s)
}

fn trim_ascii_span(s: &str) -> (&str, usize) {
    let bytes = s.as_bytes();
    let mut start = 0;
    let mut end = bytes.len();
    while start < end && is_ascii_ws(bytes[start]) {
        start += 1;
    }
    while end > start && is_ascii_ws(bytes[end - 1]) {
        end -= 1;
    }
    (&s[start..end], start)
}

fn starts_with_keyword(s: &str, kw: &str) -> bool {
    let Some(rest) = s.strip_prefix(kw) else {
        return false;
    };
    rest.is_empty() || rest.as_bytes().first().is_some_and(|b| is_ascii_ws(*b))
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn is_ascii_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t')
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_rest(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

fn is_expr_delim(b: u8) -> bool {
    is_ascii_ws(b) || matches!(b, b',' | b')' | b'~')
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMMY_CTX: LineCtx = LineCtx {
        file: FileId(0),
        line: 0,
    };

    fn opt(src: &str, ctx: LineCtx) -> (String, Literal) {
        match parse_line(src, ctx).unwrap() {
            Some(Stmt::OptionAssignment { name, val, .. }) => (name, val),
            other => panic!("expected option assignment, got {other:?}"),
        }
    }

    fn directive(src: &str, ctx: LineCtx) -> (String, Vec<Expr>) {
        match parse_line(src, ctx).unwrap() {
            Some(Stmt::Directive { name, args, .. }) => (name, args),
            other => panic!("expected directive, got {other:?}"),
        }
    }

    fn definition(src: &str, ctx: LineCtx) -> (String, Vec<Expr>) {
        match parse_line(src, ctx).unwrap() {
            Some(Stmt::Definition { kind, args, .. }) => (kind, args),
            other => panic!("expected definition, got {other:?}"),
        }
    }

    fn mapping(src: &str, ctx: LineCtx) -> (Vec<MappingAttr>, Expr, MappingOp, Expr) {
        match parse_line(src, ctx).unwrap() {
            Some(Stmt::Mapping { attrs, lhs, op, rhs, .. }) => (attrs, lhs, op, rhs),
            other => panic!("expected mapping, got {other:?}"),
        }
    }

    fn call_name(expr: &Expr) -> &str {
        match expr {
            Expr::Call { name, .. } => name,
            other => panic!("expected call, got {other:?}"),
        }
    }

    fn call_args(expr: &Expr) -> &[Expr] {
        match expr {
            Expr::Call { args, .. } => args,
            other => panic!("expected call, got {other:?}"),
        }
    }

    fn ident_name(expr: &Expr) -> &str {
        match expr {
            Expr::Ident { name, .. } => name,
            other => panic!("expected ident, got {other:?}"),
        }
    }

    fn lit(expr: &Expr) -> &Literal {
        match expr {
            Expr::Literal { value, .. } => value,
            other => panic!("expected literal, got {other:?}"),
        }
    }

    fn pair(expr: &Expr) -> (char, char) {
        match expr {
            Expr::Pair { unshifted, shifted, .. } => {
                let Literal::Char(a) = lit(unshifted) else {
                    panic!("expected char literal on pair lhs, got {unshifted:?}");
                };
                let Literal::Char(b) = lit(shifted) else {
                    panic!("expected char literal on pair rhs, got {shifted:?}");
                };
                (*a, *b)
            }
            other => panic!("expected pair, got {other:?}"),
        }
    }

    #[test]
    fn ws_or_comment_only_parse_as_none() {
        assert!(matches!(parse_line("", DUMMY_CTX).unwrap(), None));
        assert!(matches!(parse_line("   \t  ", DUMMY_CTX).unwrap(), None));
        assert!(matches!(parse_line("# comment", DUMMY_CTX).unwrap(), None));
        assert!(matches!(parse_line("   # comment", DUMMY_CTX).unwrap(), None));
    }

    #[test]
    fn accepts_trailing_lf_crlf() {
        assert!(matches!(
            parse_line("foo = true\n", DUMMY_CTX).unwrap(),
            Some(Stmt::OptionAssignment { .. })
        ));

        assert!(matches!(
            parse_line("foo = true\r\n", DUMMY_CTX).unwrap(),
            Some(Stmt::OptionAssignment { .. })
        ));
    }

    #[test]
    fn rejects_multiline() {
        assert!(parse_line("foo = true\nbar = false", DUMMY_CTX).is_err());
    }

    #[test]
    fn hash_inside_strlit_or_charlit_isnt_a_comment() {
        let (_, value) = opt(r#"x = "a#b" # comment"#, DUMMY_CTX);
        assert_eq!(value, Literal::String("a#b".to_owned()));

        let (_, value) = opt("x = '#' # comment", DUMMY_CTX);
        assert_eq!(value, Literal::Char('#'));

        let (_, value) = opt(r#"x = 123 # comment"#, DUMMY_CTX);
        assert_eq!(value, Literal::Int(123));
    }

    #[test]
    fn parses_strlit_escapes() {
        let (_, value) = opt(r#"x = "a\"b\\c\n\t\r\u{e5}\x21""#, DUMMY_CTX);
        assert_eq!(value, Literal::String("a\"b\\c\n\t\rå!".to_owned()));
    }

    #[test]
    fn rejects_bad_strlit_escapes() {
        for src in [
            r#"x = "bad\q""#,
            r#"x = "bad\x4""#,
            r#"x = "bad\xzz""#,
            r#"x = "bad\u{}""#,
            r#"x = "bad\u{110000}""#,
        ] {
            assert!(parse_line(src, DUMMY_CTX).is_err(), "accepted {src:?}");
        }
    }

    #[test]
    fn rejects_unterminated_strlits() {
        assert!(parse_line(r#"x = "abc"#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"utf8("x"#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"define group "x"#, DUMMY_CTX).is_err());
    }

    #[test]
    fn parses_char_literals() {
        assert_eq!(opt("x = 'a'", DUMMY_CTX).1, Literal::Char('a'));
        assert_eq!(opt("x = 'å'", DUMMY_CTX).1, Literal::Char('å'));
        assert_eq!(opt(r"x = '\''", DUMMY_CTX).1, Literal::Char('\''));
        assert_eq!(opt(r"x = '\\'", DUMMY_CTX).1, Literal::Char('\\'));
        assert_eq!(opt(r"x = '\n'", DUMMY_CTX).1, Literal::Char('\n'));
        assert_eq!(opt(r"x = '\t'", DUMMY_CTX).1, Literal::Char('\t'));
        assert_eq!(opt(r"x = '\r'", DUMMY_CTX).1, Literal::Char('\r'));
        assert_eq!(opt(r"x = '\x1b'", DUMMY_CTX).1, Literal::Char('\x1b'));
        assert_eq!(opt(r"x = '\u{e5}'", DUMMY_CTX).1, Literal::Char('å'));
    }

    #[test]
    fn rejects_bad_char_literals() {
        for src in [
            "x = ''",
            "x = 'ab'",
            "x = 'a",
            r"x = '\q'",
            r"x = '\x4'",
            r"x = '\xzz'",
            r"x = '\u{}'",
            r"x = '\u{110000}'",
        ] {
            assert!(parse_line(src, DUMMY_CTX).is_err(), "accepted {src:?}");
        }
    }

    #[test]
    fn parses_bool_literals() {
        assert_eq!(opt("x = true", DUMMY_CTX).1, Literal::Bool(true));
        assert_eq!(opt("x = false", DUMMY_CTX).1, Literal::Bool(false));
    }

    #[test]
    fn parses_int_literals() {
        assert_eq!(opt("x = 0", DUMMY_CTX).1, Literal::Int(0));
        assert_eq!(opt("x = +0", DUMMY_CTX).1, Literal::Int(0));
        assert_eq!(opt("x = -0", DUMMY_CTX).1, Literal::Int(0));

        assert_eq!(opt("x = 123", DUMMY_CTX).1, Literal::Int(123));
        assert_eq!(opt("x = -123", DUMMY_CTX).1, Literal::Int(-123));
        assert_eq!(opt("x = +123", DUMMY_CTX).1, Literal::Int(123));

        assert_eq!(opt("x = 0123", DUMMY_CTX).1, Literal::Int(0o123));
        assert_eq!(opt("x = +0123", DUMMY_CTX).1, Literal::Int(0o123));
        assert_eq!(opt("x = -0123", DUMMY_CTX).1, Literal::Int(-0o123));

        assert_eq!(opt("x = 0x10", DUMMY_CTX).1, Literal::Int(16));
        assert_eq!(opt("x = 0X10", DUMMY_CTX).1, Literal::Int(16));
        assert_eq!(opt("x = -0x10", DUMMY_CTX).1, Literal::Int(-16));

        assert_eq!(opt("x = 2147483647", DUMMY_CTX).1, Literal::Int(i32::MAX));
        assert_eq!(opt("x = -2147483648", DUMMY_CTX).1, Literal::Int(i32::MIN));
    }

    #[test]
    fn rejects_invalid_int_literals() {
        for src in [
            "x = +",
            "x = -",
            "x = 00",
            "x = 00123",
            "x = +00123",
            "x = -00123",
            "x = 08",
            "x = 0x",
            "x = 0xz",
            "x = 2147483648",
            "x = -2147483649",
        ] {
            assert!(parse_line(src, DUMMY_CTX).is_err(), "accepted {src:?}");
        }
    }

    #[test]
    fn parses_directive() {
        let (name, args) = directive(r#"@version 1"#, DUMMY_CTX);
        assert_eq!(name, "version");
        assert_eq!(args.len(), 1);
        assert_eq!(lit(&args[0]), &Literal::Int(1));

        let (name, args) = directive(r#"@include "foo.conf""#, DUMMY_CTX);
        assert_eq!(name, "include");
        assert_eq!(args.len(), 1);
        assert_eq!(lit(&args[0]), &Literal::String("foo.conf".to_owned()));
    }

    #[test]
    fn parses_definition() {
        let (kind, args) = definition(r#"define group "x""#, DUMMY_CTX);

        assert_eq!(kind, "group");
        assert_eq!(args.len(), 1);
        assert_eq!(lit(&args[0]), &Literal::String("x".to_owned()));
    }

    #[test]
    fn define_is_matched_as_keyword() {
        assert!(matches!(
            parse_line(r#"define group "x""#, DUMMY_CTX).unwrap(),
            Some(Stmt::Definition { .. })
        ));

        let (name, value) = opt("define.foo = true", DUMMY_CTX);

        assert_eq!(name, "define.foo");
        assert_eq!(value, Literal::Bool(true));

        assert!(parse_line(r#"defined group "x""#, DUMMY_CTX).is_err());
    }

    #[test]
    fn parses_right_mapping() {
        let (attrs, lhs, op, rhs) = mapping(r#"group("x") => send_utf8('r')"#, DUMMY_CTX);

        assert!(attrs.is_empty());
        assert_eq!(op, MappingOp::Right);

        assert_eq!(call_name(&lhs), "group");
        assert_eq!(call_args(&lhs).len(), 1);
        assert_eq!(lit(&call_args(&lhs)[0]), &Literal::String("x".to_owned()));

        assert_eq!(call_name(&rhs), "send_utf8");
        assert_eq!(call_args(&rhs).len(), 1);
        assert_eq!(lit(&call_args(&rhs)[0]), &Literal::Char('r'));
    }

    #[test]
    fn parses_left_mapping() {
        let (attrs, lhs, op, rhs) = mapping(r#"send_utf8('r') <= group("x")"#, DUMMY_CTX);

        assert!(attrs.is_empty());
        assert_eq!(op, MappingOp::Left);
        assert_eq!(call_name(&lhs), "send_utf8");
        assert_eq!(call_name(&rhs), "group");
    }

    #[test]
    fn operators_inside_literals_dont_count() {
        let (_, value) = opt(r#"x = "=> <= = #""#, DUMMY_CTX);
        assert_eq!(value, Literal::String("=> <= = #".to_owned()));

        let (_, value) = opt("x = '='", DUMMY_CTX);
        assert_eq!(value, Literal::Char('='));

        let (attrs, lhs, op, rhs) = mapping(r#"group("a=>b") => send_utf8('=')"#, DUMMY_CTX);
        assert!(attrs.is_empty());
        assert_eq!(op, MappingOp::Right);
        assert_eq!(lit(&call_args(&lhs)[0]), &Literal::String("a=>b".to_owned()));
        assert_eq!(lit(&call_args(&rhs)[0]), &Literal::Char('='));
    }

    #[test]
    fn mapping_operator_inside_call_args_does_not_count() {
        let (attrs, lhs, op, rhs) = mapping(
            r#"outer(inner("=>")) => send_utf8('x')"#,
            DUMMY_CTX,
        );

        assert!(attrs.is_empty());
        assert_eq!(op, MappingOp::Right);
        assert_eq!(call_name(&lhs), "outer");
        assert_eq!(call_name(&rhs), "send_utf8");
    }

    #[test]
    fn rejects_multiple_mapping_operators() {
        assert!(parse_line(r#"group("a") => group("b") => group("c")"#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"group("a") <= group("b") <= group("c")"#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"group("a") => group("b") <= group("c")"#, DUMMY_CTX).is_err());
    }

    #[test]
    fn parses_call_args() {
        let (_, lhs, _, _) = mapping(r#"key(up, ctrl, alt) => send_utf8('x')"#, DUMMY_CTX);
        assert_eq!(call_name(&lhs), "key");
        let args = call_args(&lhs);

        assert_eq!(args.len(), 3);
        assert_eq!(ident_name(&args[0]), "up");
        assert_eq!(ident_name(&args[1]), "ctrl");
        assert_eq!(ident_name(&args[2]), "alt");
    }

    #[test]
    fn parses_nested_call_arg_if_supported() {
        let (name, args) = directive(r#"@service exec("foo", "bar")"#, DUMMY_CTX);
        assert_eq!(name, "service");
        assert_eq!(args.len(), 1);

        let expr = &args[0];
        assert_eq!(call_name(expr), "exec");
        let args = call_args(expr);

        assert_eq!(args.len(), 2);
        assert_eq!(lit(&args[0]), &Literal::String("foo".to_owned()));
        assert_eq!(lit(&args[1]), &Literal::String("bar".to_owned()));
    }

    #[test]
    fn parses_char_pair_args() {
        let (attrs, lhs, op, rhs) = mapping(
            r"utf8('d'~'D', any) => inherit_pair_utf8('w'~'W')",
            DUMMY_CTX,
        );

        assert!(attrs.is_empty());
        assert_eq!(op, MappingOp::Right);

        assert_eq!(call_name(&lhs), "utf8");

        let lhs_args = call_args(&lhs);

        assert_eq!(lhs_args.len(), 2);
        assert_eq!(pair(&lhs_args[0]), ('d', 'D'));
        assert_eq!(ident_name(&lhs_args[1]), "any");

        assert_eq!(call_name(&rhs), "inherit_pair_utf8");

        let rhs_args = call_args(&rhs);

        assert_eq!(rhs_args.len(), 1);
        assert_eq!(pair(&rhs_args[0]), ('w', 'W'));
    }

    #[test]
    fn parses_char_pair_with_escapes() {
        let (_, lhs, _, rhs) = mapping(
            r"utf8('\t'~'\n') => inherit_pair_utf8('\x20'~'\u{21}')",
            DUMMY_CTX,
        );

        assert_eq!(pair(&call_args(&lhs)[0]), ('\t', '\n'));
        assert_eq!(pair(&call_args(&rhs)[0]), (' ', '!'));
    }

    #[test]
    fn rejects_trailing_pair_input() {
        for src in [
            r"utf8('a'~'A'~'B') => send_utf8('b')",
            r"utf8('a'~) => send_utf8('b')",
        ] {
            assert!(parse_line(src, DUMMY_CTX).is_err(), "accepted {src:?}");
        }
    }

    #[test]
    fn parses_mapping_attrs() {
        let (attrs, lhs, op, rhs) = mapping(
            r#"passthrough! unique_src! timeout!(100) key(space) => sh("x")"#,
            DUMMY_CTX,
        );

        assert_eq!(op, MappingOp::Right);
        assert_eq!(call_name(&lhs), "key");
        assert_eq!(call_name(&rhs), "sh");

        assert_eq!(attrs.len(), 3);

        assert_eq!(attrs[0].name, "passthrough");
        assert!(attrs[0].args.is_empty());

        assert_eq!(attrs[1].name, "unique_src");
        assert!(attrs[1].args.is_empty());

        assert_eq!(attrs[2].name, "timeout");
        assert_eq!(attrs[2].args.len(), 1);
        assert_eq!(lit(&attrs[2].args[0]), &Literal::Int(100));
    }

    #[test]
    fn mapping_attrs_may_touch_lhs_without_whitespace() {
        let (attrs, lhs, op, rhs) = mapping(
            r#"passthrough!key(space) => sh("x")"#,
            DUMMY_CTX,
        );

        assert_eq!(op, MappingOp::Right);
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0].name, "passthrough");
        assert_eq!(call_name(&lhs), "key");
        assert_eq!(call_name(&rhs), "sh");
    }

    #[test]
    fn rejects_trailing_input_after_literal() {
        assert!(parse_line(r#"x = true false"#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"x = "a" "b""#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"x = 1 2"#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"x = 'a' 'b'"#, DUMMY_CTX).is_err());
    }

    #[test]
    fn rejects_malformed_identifiers() {
        assert!(parse_line("1abc = true", DUMMY_CTX).is_err());
        assert!(parse_line("@1abc true", DUMMY_CTX).is_err());
        assert!(parse_line(r#"define 1abc "x""#, DUMMY_CTX).is_err());
    }

    #[test]
    fn parses_expr_idents_and_literals_in_directives() {
        let (name, args) = directive(r#"@protocol want kitty true 123 'x'"#, DUMMY_CTX);
        assert_eq!(name, "protocol");
        assert_eq!(args.len(), 5);
        assert_eq!(ident_name(&args[0]), "want");
        assert_eq!(ident_name(&args[1]), "kitty");
        assert_eq!(lit(&args[2]), &Literal::Bool(true));
        assert_eq!(lit(&args[3]), &Literal::Int(123));
        assert_eq!(lit(&args[4]), &Literal::Char('x'));
    }
}
