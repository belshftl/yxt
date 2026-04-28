// SPDX-License-Identifier: MIT

use std::{borrow::Cow, fmt};

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    String(String),
    Int(i32),
    Bool(bool),
    Ident(String),
    Call(Expr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Call {
        name: String,
        args: Vec<Arg>,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    Directive {
        name: String,
        args: Vec<Arg>,
        span: Span,
    },
    Definition {
        kind: String,
        args: Vec<Arg>,
        span: Span,
    },
    Mapping {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    Expected(&'static str),
    UnknownStatement,
    UnterminatedString,
    InvalidEscape(char),
    InvalidIdentifier,
    InvalidInteger,
    IntegerOutOfRange,
    TrailingInput,
    MultipleMappingOperators,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub kind: ErrorKind,
    pub span: Span,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} at byte {}..{}", self.kind, self.span.start, self.span.end)
    }
}

impl std::error::Error for ParseError {}

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

        let lhs = parse_expr_full(&s[..rel], ctx, base)?;
        let rhs = parse_expr_full(&s[rel + 2..], ctx, pos + 2)?;

        Ok(Some(Stmt::Mapping {
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

    fn parse_strlit(&mut self) -> Result<Cow<'a, str>, ParseError> {
        let quote_start = self.pos;
        self.expect_byte(b'"', "'\"'")?;

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
                    let Some(esc) = self.cons_byte() else {
                        return Err(ParseError {
                            kind: ErrorKind::UnterminatedString,
                            span: Span {
                                ctx: self.ctx,
                                start: self.base + quote_start,
                                end: self.base + self.pos,
                            },
                        });
                    };

                    match esc {
                        b'\\' => out.push('\\'),
                        b'"' => out.push('"'),
                        _ => return Err(ParseError {
                            kind: ErrorKind::InvalidEscape(esc as char),
                            span: Span {
                                ctx: self.ctx,
                                start: self.base + esc_pos,
                                end: self.base + esc_pos + 1,
                            },
                        })
                    }
                    tail_start = self.pos;
                }
                _ => self.pos += 1
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

    fn parse_arg(&mut self) -> Result<Arg, ParseError> {
        self.skip_ws();

        match self.peek_byte() {
            Some(b'"') => self.parse_strlit().map(Cow::into_owned).map(Arg::String),
            Some(b) if is_ident_start(b) => {
                let saved = self.pos;
                let ident = self.parse_ident()?;
                self.skip_ws();
                if self.peek_byte() == Some(b'(') {
                    self.pos = saved;
                    return self.parse_call_expr().map(Arg::Call);
                }
                match ident {
                    "true" => Ok(Arg::Bool(true)),
                    "false" => Ok(Arg::Bool(false)),
                    _ => Ok(Arg::Ident(ident.to_owned())),
                }
            }
            Some(b'+') | Some(b'-') | Some(b'0'..=b'9') => {
                let start = self.pos;
                while let Some(b) = self.peek_byte() {
                    if is_arg_delim(b) {
                        break;
                    }
                    self.pos += 1;
                }
                let text = &self.s[start..self.pos];
                let span = self.span_from(start);
                parse_i32(text, span).map(Arg::Int)
            }
            _ => Err(self.err_here(ErrorKind::Expected("argument")))
        }
    }

    fn parse_call_expr(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();

        let start = self.pos;
        let name = self.parse_ident()?;

        self.skip_ws();
        self.expect_byte(b'(', "'('")?;

        let mut args = Vec::new();
        loop {
            self.skip_ws();
            if self.peek_byte() == Some(b')') {
                self.cons_byte();
                break;
            }

            args.push(self.parse_arg()?);

            self.skip_ws();
            match self.peek_byte() {
                Some(b',') => {
                    self.cons_byte();
                }
                Some(b')') => {
                    self.cons_byte();
                    break;
                }
                _ => return Err(self.err_here(ErrorKind::Expected("',' or ')'"))),
            }
        }

        Ok(Expr::Call {
            name: name.to_owned(),
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
    in_strlit: bool,
    strlit_start: usize,
    done: bool,
}

impl<'a> TopLevelBytes<'a> {
    fn new(s: &'a str, ctx: LineCtx, base: usize) -> Self {
        Self {
            s,
            ctx,
            base,
            pos: 0,
            in_strlit: false,
            strlit_start: 0,
            done: false,
        }
    }

    fn err_unterminated(&self) -> ParseError {
        ParseError {
            kind: ErrorKind::UnterminatedString,
            span: Span {
                ctx: self.ctx,
                start: self.base + self.strlit_start,
                end: self.base + self.s.len()
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

            if self.in_strlit {
                match b {
                    b'"' => {
                        self.in_strlit = false;
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
                    _ => self.pos += 1
                }
            } else {
                match b {
                    b'"' => {
                        self.in_strlit = true;
                        self.strlit_start = self.pos;
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
        if self.in_strlit {
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
    assignment_eq: Option<usize>
}

fn matches_toplv_byte(item: Option<&Result<TopLevelByte, ParseError>>, pos: usize, byte: u8,) -> Result<bool, ParseError> {
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

    while let Some(item) = it.next() {
        let item = item?;
        match item.byte {
            b'#' => {
                content_end = item.pos;
                break;
            }
            b'=' => {
                if matches_toplv_byte(it.peek(), item.pos + 1, b'>')? {
                    _ = it.next().transpose()?; // swallow '>'
                    if mapping_op.replace((item.pos, MappingOp::Right)).is_some() {
                        return Err(ParseError {
                            kind: ErrorKind::MultipleMappingOperators,
                            span: Span { ctx, start: item.pos, end: item.pos + 2 },
                        });
                    }
                    continue;
                }
                if assignment_eq.is_none() {
                    assignment_eq = Some(item.pos);
                }
            }
            b'<' => {
                if matches_toplv_byte(it.peek(), item.pos + 1, b'=')? {
                    _ = it.next().transpose()?; // swallow '='
                    if mapping_op.replace((item.pos, MappingOp::Left)).is_some() {
                        return Err(ParseError {
                            kind: ErrorKind::MultipleMappingOperators,
                            span: Span { ctx, start: item.pos, end: item.pos + 2 },
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

    let args = parse_ws_args(&mut c)?;
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

    let args = parse_ws_args(&mut c)?;
    Ok(Stmt::Definition {
        kind: kind.to_owned(),
        args,
        span: Span { ctx, start: base, end: base + s.len() },
    })
}

fn parse_ws_args(c: &mut Cursor<'_>) -> Result<Vec<Arg>, ParseError> {
    let mut args = Vec::new();
    loop {
        c.skip_ws();
        if c.eof() {
            break;
        }

        args.push(c.parse_arg()?);
        if !c.eof() && !is_ascii_ws(c.peek_byte().unwrap()) {
            return Err(c.err_here(ErrorKind::Expected("whitespace")));
        }
    }
    Ok(args)
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
    let expr = c.parse_call_expr()?;

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
        _ => false
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
            if rest.as_bytes()[0] == b'0' || !rest.bytes().all(|b| matches!(b, b'0'..b'7')) {
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
                span: Span { ctx, start: pos + 1, end: s.len() }
            });
        }

        let before_lf = &s[..pos];
        return Ok(before_lf.strip_suffix('\r').unwrap_or(before_lf));
    }
    Ok(s)
}

fn strip_comment(line: &str, ctx: LineCtx) -> Result<&str, ParseError> {
    let mut quote = false;
    let mut bksl = false;
    let mut last_quote = 0;

    for (i, ch) in line.char_indices() {
        if !bksl && ch == '"' {
            last_quote = i;
            quote = !quote;
        } else if quote && !bksl && ch == '\\' {
            bksl = true;
            continue;
        } else if !quote && ch == '#' {
            return Ok(&line[..i]);
        }
        bksl = false;
    }

    if quote {
        return Err(ParseError {
            kind: ErrorKind::UnterminatedString,
            span: Span { ctx, start: last_quote, end: line.len() },
        });
    }
    Ok(line)
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

fn is_ascii_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t')
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_rest(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

fn is_arg_delim(b: u8) -> bool {
    is_ascii_ws(b) || matches!(b, b',' | b')')
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMMY_CTX: LineCtx = LineCtx { file: FileId(0), line: 0};

    fn opt(src: &str, ctx: LineCtx) -> (String, Literal) {
        match parse_line(src, ctx).unwrap() {
            Some(Stmt::OptionAssignment { name, val, .. }) => (name, val),
            other => panic!("expected option assignment, got {other:?}"),
        }
    }

    fn directive(src: &str, ctx: LineCtx) -> (String, Vec<Arg>) {
        match parse_line(src, ctx).unwrap() {
            Some(Stmt::Directive { name, args, .. }) => (name, args),
            other => panic!("expected directive, got {other:?}"),
        }
    }

    fn definition(src: &str, ctx: LineCtx) -> (String, Vec<Arg>) {
        match parse_line(src, ctx).unwrap() {
            Some(Stmt::Definition { kind, args, .. }) => (kind, args),
            other => panic!("expected definition, got {other:?}"),
        }
    }

    fn mapping(src: &str, ctx: LineCtx) -> (Expr, MappingOp, Expr) {
        match parse_line(src, ctx).unwrap() {
            Some(Stmt::Mapping { lhs, op, rhs, .. }) => (lhs, op, rhs),
            other => panic!("expected mapping, got {other:?}"),
        }
    }

    fn call_name(expr: &Expr) -> &str {
        match expr {
            Expr::Call { name, .. } => name,
        }
    }

    fn call_args(expr: &Expr) -> &[Arg] {
        match expr {
            Expr::Call { args, .. } => args,
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
    fn accepts_trailing_lf_cr() {
        assert!(matches!(parse_line("foo = true\n", DUMMY_CTX).unwrap(), Some(Stmt::OptionAssignment { .. })));
        assert!(matches!(parse_line("foo = true\r\n", DUMMY_CTX).unwrap(), Some(Stmt::OptionAssignment { .. })));
    }

    #[test]
    fn rejects_multiline() {
        assert!(parse_line("foo = true\nbar = false", DUMMY_CTX).is_err());
    }

    #[test]
    fn hash_inside_strlit_isnt_a_comment() {
        let (_, value) = opt(r#"x = "a#b" # comment"#, DUMMY_CTX);
        assert_eq!(value, Literal::String("a#b".to_owned()));

        let (_, value) = opt(r#"x = 123 # comment"#, DUMMY_CTX);
        assert_eq!(value, Literal::Int(123));
    }

    #[test]
    fn parses_strlit_escapes() {
        let (_, value) = opt(r#"x = "a\"b\\c""#, DUMMY_CTX);
        assert_eq!(value, Literal::String(r#"a"b\c"#.to_owned()));
    }

    #[test]
    fn rejects_bad_strlit_escapes() {
        assert!(parse_line(r#"x = "bad\n""#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"x = "bad\t""#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"x = "bad\x41""#, DUMMY_CTX).is_err());
    }

    #[test]
    fn rejects_unterminated_strlits() {
        assert!(parse_line(r#"x = "abc"#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"tok_utf8("x"#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"define group "x"#, DUMMY_CTX).is_err());
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
        assert_eq!(args, vec![Arg::Int(1)]);

        let (name, args) = directive(r#"@include "foo.conf""#, DUMMY_CTX);
        assert_eq!(name, "include");
        assert_eq!(args, vec![Arg::String("foo.conf".to_owned())]);
    }

    #[test]
    fn parses_definition() {
        let (kind, args) = definition(r#"define group "x""#, DUMMY_CTX);
        assert_eq!(kind, "group");
        assert_eq!(args, vec![Arg::String("x".to_owned())]);
    }

    #[test]
    fn define_is_matched_as_keyword() {
        assert!(matches!(parse_line(r#"define group "x""#, DUMMY_CTX).unwrap(), Some(Stmt::Definition { .. })));

        let (name, value) = opt("define.foo = true", DUMMY_CTX);
        assert_eq!(name, "define.foo");
        assert_eq!(value, Literal::Bool(true));

        assert!(parse_line(r#"defined group "x""#, DUMMY_CTX).is_err());
    }

    #[test]
    fn parses_right_mapping() {
        let (lhs, op, rhs) = mapping(r#"group("x") => tok_utf8("r")"#, DUMMY_CTX);

        assert_eq!(op, MappingOp::Right);
        assert_eq!(call_name(&lhs), "group");
        assert_eq!(call_args(&lhs), &[Arg::String("x".to_owned())]);

        assert_eq!(call_name(&rhs), "tok_utf8");
        assert_eq!(call_args(&rhs), &[Arg::String("r".to_owned())]);
    }

    #[test]
    fn parses_left_mapping() {
        let (lhs, op, rhs) = mapping(r#"tok_utf8("r") <= group("x")"#, DUMMY_CTX);
        assert_eq!(op, MappingOp::Left);
        assert_eq!(call_name(&lhs), "tok_utf8");
        assert_eq!(call_name(&rhs), "group");
    }

    #[test]
    fn operators_inside_strlits_dont_count() {
        let (_, value) = opt(r#"x = "=> <= = #""#, DUMMY_CTX);
        assert_eq!(value, Literal::String("=> <= = #".to_owned()));

        let (lhs, op, rhs) = mapping(r#"group("a=>b") => tok_utf8("=")"#, DUMMY_CTX);
        assert_eq!(op, MappingOp::Right);
        assert_eq!(call_args(&lhs), &[Arg::String("a=>b".to_owned())]);
        assert_eq!(call_args(&rhs), &[Arg::String("=".to_owned())]);
    }

    #[test]
    fn rejects_multiple_mapping_operators() {
        assert!(parse_line(r#"group("a") => group("b") => group("c")"#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"group("a") <= group("b") <= group("c")"#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"group("a") => group("b") <= group("c")"#, DUMMY_CTX).is_err());
    }

    #[test]
    fn parses_call_args() {
        let (lhs, _, _) = mapping(r#"tok_key(up, ctrl, alt) => tok_utf8("x")"#, DUMMY_CTX);
        assert_eq!(call_name(&lhs), "tok_key");
        assert_eq!(
            call_args(&lhs),
            &[
                Arg::Ident("up".to_owned()),
                Arg::Ident("ctrl".to_owned()),
                Arg::Ident("alt".to_owned()),
            ],
        );
    }

    #[test]
    fn parses_nested_call_arg_if_supported() {
        let (name, args) = directive(r#"@service exec("foo", "bar")"#, DUMMY_CTX);
        assert_eq!(name, "service");

        match &args[..] {
            [Arg::Call(expr)] => {
                assert_eq!(call_name(expr), "exec");
                assert_eq!(
                    call_args(expr),
                    &[
                        Arg::String("foo".to_owned()),
                        Arg::String("bar".to_owned()),
                    ],
                );
            }
            other => panic!("expected nested call arg, got {other:?}"),
        }
    }

    #[test]
    fn rejects_trailing_input_after_literal() {
        assert!(parse_line(r#"x = true false"#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"x = "a" "b""#, DUMMY_CTX).is_err());
        assert!(parse_line(r#"x = 1 2"#, DUMMY_CTX).is_err());
    }

    #[test]
    fn rejects_malformed_identifiers() {
        assert!(parse_line("1abc = true", DUMMY_CTX).is_err());
        assert!(parse_line("@1abc true", DUMMY_CTX).is_err());
        assert!(parse_line(r#"define 1abc "x""#, DUMMY_CTX).is_err());
    }
}
