// SPDX-License-Identifier: MIT

use std::collections::HashSet;

use crate::model::*;
use super::line::{Expr, Literal, MappingAttr, MappingOp, Span, Stmt};
use super::options::Options;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralKind {
    Bool,
    Int,
    String,
    Char,
}

impl LiteralKind {
    pub fn of(value: &Literal) -> Self {
        match value {
            Literal::Bool(_) => Self::Bool,
            Literal::Int(_) => Self::Int,
            Literal::String(_) => Self::String,
            Literal::Char(_) => Self::Char,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ConfigError {
    #[error("unknown directive '@{name}'")]
    UnknownDirective {
        name: String,
        span: Span,
    },

    #[error("invalid arguments for directive '@{kind}'")]
    BadDirectiveArgs {
        kind: &'static str,
        span: Span,
    },

    #[error("duplicate of directive '@{kind}' is not allowed: {reason}")]
    DuplicateDirective {
        kind: &'static str,
        reason: &'static str,
        span: Span,
    },

    #[error("unknown command kind '{kind}'")]
    UnknownCommandKind {
        kind: String,
        span: Span,
    },

    #[error("invalid arguments for command of kind '{kind}'")]
    BadCommandArgs {
        kind: &'static str,
        span: Span,
    },

    #[error("command cannot be empty")]
    EmptyCommand {
        span: Span,
    },

    #[error("unknown definition kind '{kind}'")]
    UnknownDefinition {
        kind: String,
        span: Span,
    },

    #[error("invalid arguments for definition '{kind}'")]
    BadDefinitionArgs {
        kind: &'static str,
        span: Span,
    },

    #[error("duplicate group '{name}'")]
    DuplicateGroup {
        name: String,
        span: Span,
    },

    #[error("unknown group '{name}'")]
    UnknownGroup {
        name: String,
        span: Span,
    },

    #[error("unknown option '{name}'")]
    UnknownOption {
        name: String,
        span: Span,
    },

    #[error("wrong literal type: expected '{expected:?}', got '{got:?}'")]
    WrongLiteralType {
        expected: LiteralKind,
        got: LiteralKind,
        span: Span,
    },

    #[error("unknown entity '{name}'")]
    UnknownEntity {
        name: String,
        span: Span,
    },

    #[error("invalid arguments for entity '{kind}'")]
    BadEntityArgs {
        kind: &'static str,
        span: Span,
    },

    #[error("unknown mapping attribute '{name}'")]
    UnsupportedMappingAttr {
        name: String,
        span: Span,
    },

    #[error("invalid arguments for mapping attribute '{kind}'")]
    BadMappingAttrArgs {
        kind: &'static str,
        span: Span,
    },

    #[error("duplicate mapping attribute '{kind}'")]
    DuplicateMappingAttr {
        kind: &'static str,
        span: Span,
    },

    #[error("pair expressions are not supported here")]
    PairUnsupported {
        span: Span,
    },

    #[error("char-pair key must contain two character literals")]
    CharPairKeyNeedsChars {
        span: Span,
    },

    #[error("unknown signal '{name}'")]
    UnknownSignal {
        name: String,
        span: Span,
    },

    #[error("signal '{name}' is {reason}")]
    UnsupportedSignal {
        name: String,
        reason: &'static str,
        span: Span,
    },

    #[error("unknown key '{name}'")]
    UnknownKey {
        name: String,
        span: Span,
    },

    #[error("unknown modifier '{name}'")]
    UnknownModifier {
        name: String,
        span: Span,
    },

    #[error("duplicate modifier '{name}'")]
    DuplicateModifier {
        name: String,
        span: Span,
    },

    #[error("invalid modifier")]
    BadModifier {
        span: Span,
    },

    #[error("target-only token constructor cannot be used as mapping source")]
    SendTokenAsSource {
        span: Span,
    },

    #[error("source-only token constructor cannot be used as mapping target")]
    SourceTokenAsTarget {
        span: Span,
    },

    #[error("action cannot be used as mapping source")]
    ActionAsSource {
        span: Span,
    },

    #[error("inherit token cannot be used as mapping source")]
    InheritTokenAsSource {
        span: Span,
    },

    #[error("event cannot be used as mapping target")]
    EventAsTarget {
        span: Span,
    },

    #[error("target requires payload of type {required:?} that the mapped source did not provide")]
    TargetRequiresPayload {
        required: PayloadKind,
        span: Span,
    },

    #[error("cannot map a group to itself")]
    GroupSelfMap {
        span: Span,
    },
}

#[derive(Debug)]
pub struct ConfigBuilder {
    protocol: Option<ProtocolPolicy>,
    options: Options,
    groups: GroupTable,
    mappings: Vec<Mapping>,
    services: Vec<Service>,
    sv_names: HashSet<String>,
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        Self {
            protocol: None,
            options: Options::default(),
            groups: GroupTable::default(),
            mappings: Vec::new(),
            services: Vec::new(),
            sv_names: HashSet::new(),
        }
    }
}

impl ConfigBuilder {
    pub fn apply_stmt(&mut self, stmt: Stmt) -> Result<(), ConfigError> {
        match stmt {
            Stmt::Directive { name, args, span } => self.apply_directive(name, args, span),
            Stmt::Definition { kind, args, span } => self.apply_definition(kind, args, span),
            Stmt::Mapping { attrs, lhs, op, rhs, span } => self.apply_mapping(attrs, lhs, op, rhs, span),
            Stmt::OptionAssignment { name, val, span } => self.options.set(name, val, span),
        }
    }

    pub fn finish(self) -> Result<Config, ConfigError> {
        Ok(Config {
            protocol: self.protocol.unwrap_or_default(),
            options: self.options,
            groups: self.groups,
            mappings: self.mappings,
            services: self.services,
        })
    }

    fn apply_directive(
        &mut self,
        name: String,
        args: Vec<Expr>,
        span: Span,
    ) -> Result<(), ConfigError> {
        match name.as_str() {
            "protocol" => todo!(),
            "service" => self.apply_service(args, span),
            _ => Err(ConfigError::UnknownDirective {
                name,
                span,
            }),
        }
    }

    fn apply_service(&mut self, args: Vec<Expr>, span: Span) -> Result<(), ConfigError> {
        let mut args = args.into_iter();

        let Some(Expr::Literal { value: Literal::String(name), ..}) = args.next() else {
            return Err(ConfigError::BadDirectiveArgs { kind: "service", span });
        };

        if name.is_empty() {
            return Err(ConfigError::BadDirectiveArgs { kind: "service", span });
        }

        let Some(expr) = args.next() else {
            return Err(ConfigError::BadDirectiveArgs { kind: "service", span });
        };

        if args.next().is_some() {
            return Err(ConfigError::BadDirectiveArgs { kind: "service", span });
        }

        if !self.sv_names.insert(name.clone()) {
            return Err(ConfigError::DuplicateDirective {
                kind: "service",
                reason: "duplicate service name; services must have unique names",
                span,
            });
        }

        let (call_name, call_args, call_span) = expect_call(expr).map_err(|_| ConfigError::BadDirectiveArgs {
            kind: "service", span
        })?;
        let command = match call_name.as_str() {
            "exec" => lower_exec_command(call_args, call_span)?,
            "sh" => lower_shell_command(call_args, call_span)?,
            _ => return Err(ConfigError::UnknownCommandKind { kind: call_name, span: call_span }),
        };
        self.services.push(Service { name, command });
        Ok(())
    }

    fn apply_definition(
        &mut self,
        kind: String,
        args: Vec<Expr>,
        span: Span,
    ) -> Result<(), ConfigError> {
        match kind.as_str() {
            "group" => {
                let name = expect_one_string(args).map_err(|_| ConfigError::BadDefinitionArgs {
                    kind: "group", span,
                })?;
                self.groups.define(name).map_err(|e| match e {
                    DefineGroupError::Duplicate(name) => ConfigError::DuplicateGroup { name, span }
                })?;
                Ok(())
            }
            _ => Err(ConfigError::UnknownDefinition { kind, span }),
        }
    }

    fn apply_mapping(
        &mut self,
        attrs: Vec<MappingAttr>,
        lhs: Expr,
        op: MappingOp,
        rhs: Expr,
        span: Span,
    ) -> Result<(), ConfigError> {
        let _attrs = lower_mapping_attrs(attrs)?;

        let (from_expr, to_expr) = match op {
            MappingOp::Right => (lhs, rhs),
            MappingOp::Left => (rhs, lhs),
        };
        let from = self.lower_source(from_expr)?;
        let to = self.lower_target(to_expr)?;
        let required = to.requires_payload();
        if let Some(required) = required && from.provides_payload() != Some(required) {
            Err(ConfigError::TargetRequiresPayload { required, span })
        } else if let (Source::Group(a), Target::Group(b)) = (&from, &to) && a == b {
            Err(ConfigError::GroupSelfMap { span })
        } else {
            self.mappings.push(Mapping { from, to, span });
            Ok(())
        }
    }

    fn lower_source(&self, expr: Expr) -> Result<Source, ConfigError> {
        let span = expr.span();
        let (name, args, call_span) = expect_call(expr).map_err(|_| ConfigError::UnknownEntity {
            name: "<non-call>".to_owned(), span
        })?;
        match name.as_str() {
            "signal" => lower_signal_source(args, call_span),
            "sockdata_utf8" => lower_sockdata_utf8_source(args, call_span),
            "key" => lower_key_source(args, call_span),
            "group" => Ok(Source::Group(self.lower_group_id(args, call_span)?)),
            "send_key" => Err(ConfigError::SendTokenAsSource { span }),
            "inherit_key" => Err(ConfigError::InheritTokenAsSource { span }),
            "exec" | "sh" => Err(ConfigError::ActionAsSource { span }),
            _ => Err(ConfigError::UnknownEntity { name, span: call_span }),
        }
    }

    fn lower_target(&self, expr: Expr) -> Result<Target, ConfigError> {
        let span = expr.span();
        let (name, args, call_span) = expect_call(expr).map_err(|_| ConfigError::UnknownEntity {
            name: "<non-call>".to_owned(), span
        })?;
        match name.as_str() {
            "send_key" => lower_send_key(args, call_span),
            "inherit_key" => lower_inherit_key(args, call_span),
            "group" => Ok(Target::Group(self.lower_group_id(args, call_span)?)),
            "exec" => Ok(Target::Action(Action::Command(lower_exec_command(args, call_span)?))),
            "sh" => Ok(Target::Action(Action::Command(lower_shell_command(args, call_span)?))),
            "signal" | "sockdata_utf8" => Err(ConfigError::EventAsTarget { span }),
            "key" => Err(ConfigError::SourceTokenAsTarget { span }),
            _ => Err(ConfigError::UnknownEntity { name, span: call_span }),
        }
    }

    fn lower_group_id(&self, args: Vec<Expr>, span: Span) -> Result<GroupId, ConfigError> {
        let group_name = expect_one_string(args).map_err(|_| ConfigError::BadEntityArgs {
            kind: "group", span,
        })?;
        self.groups.lookup(&group_name).ok_or_else(|| ConfigError::UnknownGroup { name: group_name, span })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct MappingAttrs {
    // TODO
}

fn lower_mapping_attrs(attrs: Vec<MappingAttr>) -> Result<MappingAttrs, ConfigError> {
    let out = MappingAttrs::default();
    for attr in attrs {
        match attr.name.as_str() {
            // TODO
            _ => {
                return Err(ConfigError::UnsupportedMappingAttr {
                    name: attr.name,
                    span: attr.span,
                });
            }
        }
    }
    Ok(out)
}

fn lower_signal_source(args: Vec<Expr>, span: Span) -> Result<Source, ConfigError> {
    let name = expect_one_string(args).map_err(|_| ConfigError::BadEntityArgs {
        kind: "signal", span,
    })?;
    let signal = lower_signal_name(&name, span)?;
    Ok(Source::Event(Event::Signal(signal)))
}

fn lower_sockdata_utf8_source(args: Vec<Expr>, span: Span) -> Result<Source, ConfigError> {
    let s = expect_one_string(args).map_err(|_| ConfigError::BadEntityArgs {
        kind: "sockdata_utf8", span,
    })?;
    Ok(Source::Event(Event::Sockdata(s.as_bytes().to_vec())))
}

fn lower_key_source(args: Vec<Expr>, span: Span) -> Result<Source, ConfigError> {
    let mut args = args.into_iter();
    let Some(key_expr) = args.next() else {
        return Err(ConfigError::BadEntityArgs { kind: "key", span });
    };
    let key = lower_key_pattern_arg(key_expr, "key", span)?;
    let mods = lower_mod_pattern(args.collect(), span)?;
    Ok(Source::Token(TokenPattern::Key { key, mods }))
}

fn lower_send_key(args: Vec<Expr>, span: Span) -> Result<Target, ConfigError> {
    let mut args = args.into_iter();
    let Some(key_expr) = args.next() else {
        return Err(ConfigError::BadEntityArgs { kind: "send_key", span });
    };
    let mods = lower_mods(args, span)?;
    match key_expr {
        Expr::Ident { name, .. } => Ok(Target::Token(Token::press_key(lower_key_name(&name, span)?, mods))),
        Expr::Literal { value: Literal::Char(ch), .. } => Ok(Target::Token(Token::press_utf8(ch, mods))),
        Expr::Pair { span, .. } => Err(ConfigError::PairUnsupported { span }),
        _ => Err(ConfigError::BadEntityArgs { kind: "send_key", span }),
    }
}

fn lower_inherit_key(args: Vec<Expr>, span: Span) -> Result<Target, ConfigError> {
    let mut args = args.into_iter();
    let Some(expr) = args.next() else {
        return Err(ConfigError::BadEntityArgs { kind: "inherit_key", span });
    };
    if args.next().is_some() {
        return Err(ConfigError::BadEntityArgs { kind: "inherit_key", span });
    }
    Ok(Target::InheritToken(InheritToken::Key {
        key: lower_key_pattern_arg(expr, "inherit_key", span)?,
    }))
}

fn lower_key_pattern_arg(
    expr: Expr,
    kind: &'static str,
    span: Span,
) -> Result<KeyPattern, ConfigError> {
    match expr {
        Expr::Ident { name, .. } => Ok(KeyPattern::Named(lower_key_name(&name, span)?)),
        Expr::Pair { .. } => Ok(KeyPattern::CharPair(lower_char_pair_expr(expr)?)),
        _ => Err(ConfigError::BadEntityArgs { kind, span }),
    }
}

fn lower_char_pair_expr(expr: Expr) -> Result<CharPair, ConfigError> {
    let span = expr.span();
    let Expr::Pair { unshifted, shifted, .. } = expr else {
        return Err(ConfigError::CharPairKeyNeedsChars { span });
    };
    let Some(unshifted) = literal_char(*unshifted) else {
        return Err(ConfigError::CharPairKeyNeedsChars { span });
    };
    let Some(shifted) = literal_char(*shifted) else {
        return Err(ConfigError::CharPairKeyNeedsChars { span });
    };
    Ok(CharPair { unshifted, shifted })
}

fn literal_char(expr: Expr) -> Option<char> {
    if let Expr::Literal { value: Literal::Char(ch), .. } = expr {
        Some(ch)
    } else {
        None
    }
}

fn lower_mod_pattern(args: Vec<Expr>, span: Span) -> Result<ModsPattern, ConfigError> {
    if args.is_empty() {
        return Ok(ModsPattern::AnyOf(vec![Mods::EMPTY]));
    }

    if args.len() == 1 {
        if let Expr::Ident { name, .. } = &args[0] {
            match name.as_str() {
                "any" => return Ok(ModsPattern::Any),
                "none" => return Ok(ModsPattern::AnyOf(vec![Mods::EMPTY])),
                _ => {}
            }
        }
    }

    let mods = lower_mods(args, span)?;
    Ok(ModsPattern::AnyOf(vec![mods]))
}

fn lower_mods(
    args: impl IntoIterator<Item = Expr>,
    span: Span,
) -> Result<Mods, ConfigError> {
    let mut mods = Mods::EMPTY;

    for arg in args {
        let Expr::Ident { name, .. } = arg else {
            return Err(ConfigError::BadModifier { span });
        };
        let bit = lower_mod_name(&name, span)?;
        if (mods & bit) != Mods::EMPTY {
            return Err(ConfigError::DuplicateModifier { name, span });
        }
        mods |= bit;
    }

    Ok(mods)
}

fn lower_signal_name(name: &str, span: Span) -> Result<Signal, ConfigError> {
    match name {
        "SIGHUP" => Ok(Signal(libc::SIGHUP)),
        "SIGINT" => Ok(Signal(libc::SIGINT)),
        "SIGQUIT" => Ok(Signal(libc::SIGQUIT)),
        "SIGTERM" => Ok(Signal(libc::SIGTERM)),
        "SIGUSR1" => Ok(Signal(libc::SIGUSR1)),
        "SIGUSR2" => Ok(Signal(libc::SIGUSR2)),
        "SIGCHLD" => Ok(Signal(libc::SIGCHLD)),
        "SIGCONT" => Ok(Signal(libc::SIGCONT)),
        "SIGTSTP" => Ok(Signal(libc::SIGTSTP)),
        "SIGTTIN" => Ok(Signal(libc::SIGTTIN)),
        "SIGTTOU" => Ok(Signal(libc::SIGTTOU)),
        "SIGWINCH" => Ok(Signal(libc::SIGWINCH)),
        "SIGKILL" | "SIGSTOP" => Err(ConfigError::UnsupportedSignal {
            name: name.to_owned(),
            reason: "uncatchable",
            span,
        }),
        "SIGILL" | "SIGABRT" | "SIGFPE" | "SIGSEGV" | "SIGBUS" | "SIGTRAP" | "SIGSYS" => {
            Err(ConfigError::UnsupportedSignal {
                name: name.to_owned(),
                reason: "unsupported; error signals are not supported as events",
                span,
            })
        }
        _ => Err(ConfigError::UnknownSignal {
            name: name.to_owned(),
            span,
        }),
    }
}

fn lower_key_name(name: &str, span: Span) -> Result<Key, ConfigError> {
    match name {
        "esc" => Ok(Key::Esc),
        "enter" => Ok(Key::Enter),
        "tab" => Ok(Key::Tab),
        "backspace" => Ok(Key::Backspace),

        "insert" => Ok(Key::Insert),
        "delete" => Ok(Key::Delete),
        "home" => Ok(Key::Home),
        "end" => Ok(Key::End),
        "page_up" => Ok(Key::PageUp),
        "page_down" => Ok(Key::PageDown),

        "left" => Ok(Key::Arrow(Direction::Left)),
        "right" => Ok(Key::Arrow(Direction::Right)),
        "up" => Ok(Key::Arrow(Direction::Up)),
        "down" => Ok(Key::Arrow(Direction::Down)),

        "kp_decimal" => Ok(Key::Keypad(KeypadKey::Decimal)),
        "kp_divide" => Ok(Key::Keypad(KeypadKey::Divide)),
        "kp_multiply" => Ok(Key::Keypad(KeypadKey::Multiply)),
        "kp_subtract" => Ok(Key::Keypad(KeypadKey::Subtract)),
        "kp_add" => Ok(Key::Keypad(KeypadKey::Add)),
        "kp_enter" => Ok(Key::Keypad(KeypadKey::Enter)),
        "kp_equal" => Ok(Key::Keypad(KeypadKey::Equal)),
        "kp_separator" => Ok(Key::Keypad(KeypadKey::Separator)),
        "kp_begin" => Ok(Key::Keypad(KeypadKey::Begin)),

        "kp_left" => Ok(Key::Keypad(KeypadKey::Left)),
        "kp_right" => Ok(Key::Keypad(KeypadKey::Right)),
        "kp_up" => Ok(Key::Keypad(KeypadKey::Up)),
        "kp_down" => Ok(Key::Keypad(KeypadKey::Down)),
        "kp_page_up" => Ok(Key::Keypad(KeypadKey::PageUp)),
        "kp_page_down" => Ok(Key::Keypad(KeypadKey::PageDown)),
        "kp_home" => Ok(Key::Keypad(KeypadKey::Home)),
        "kp_end" => Ok(Key::Keypad(KeypadKey::End)),
        "kp_insert" => Ok(Key::Keypad(KeypadKey::Insert)),
        "kp_delete" => Ok(Key::Keypad(KeypadKey::Delete)),

        "caps_lock" => Ok(Key::CapsLock),
        "scroll_lock" => Ok(Key::ScrollLock),
        "num_lock" => Ok(Key::NumLock),
        "print_screen" => Ok(Key::PrintScreen),
        "pause" => Ok(Key::Pause),
        "menu" => Ok(Key::Menu),

        "media_play" => Ok(Key::Media(MediaKey::Play)),
        "media_pause" => Ok(Key::Media(MediaKey::Pause)),
        "media_play_pause" => Ok(Key::Media(MediaKey::PlayPause)),
        "media_reverse" => Ok(Key::Media(MediaKey::Reverse)),
        "media_stop" => Ok(Key::Media(MediaKey::Stop)),
        "media_fast_forward" => Ok(Key::Media(MediaKey::FastForward)),
        "media_rewind" => Ok(Key::Media(MediaKey::Rewind)),
        "media_track_next" => Ok(Key::Media(MediaKey::TrackNext)),
        "media_track_previous" => Ok(Key::Media(MediaKey::TrackPrevious)),
        "media_record" => Ok(Key::Media(MediaKey::Record)),
        "volume_down" => Ok(Key::Media(MediaKey::LowerVolume)),
        "volume_up" => Ok(Key::Media(MediaKey::RaiseVolume)),
        "volume_mute" => Ok(Key::Media(MediaKey::MuteVolume)),

        "left_shift" => Ok(Key::ModifierKey(ModifierKey::LeftShift)),
        "left_ctrl" => Ok(Key::ModifierKey(ModifierKey::LeftCtrl)),
        "left_alt" => Ok(Key::ModifierKey(ModifierKey::LeftAlt)),
        "left_super" => Ok(Key::ModifierKey(ModifierKey::LeftSuper)),
        "left_hyper" => Ok(Key::ModifierKey(ModifierKey::LeftHyper)),
        "left_meta" => Ok(Key::ModifierKey(ModifierKey::LeftMeta)),

        "right_shift" => Ok(Key::ModifierKey(ModifierKey::RightShift)),
        "right_ctrl" => Ok(Key::ModifierKey(ModifierKey::RightCtrl)),
        "right_alt" => Ok(Key::ModifierKey(ModifierKey::RightAlt)),
        "right_super" => Ok(Key::ModifierKey(ModifierKey::RightSuper)),
        "right_hyper" => Ok(Key::ModifierKey(ModifierKey::RightHyper)),
        "right_meta" => Ok(Key::ModifierKey(ModifierKey::RightMeta)),

        "iso_level3_shift" => Ok(Key::IsoLevel3Shift),
        "iso_level5_shift" => Ok(Key::IsoLevel5Shift),

        _ => {
            if let Some(n) = parse_numbered_name(name, "f") && (1..=35).contains(&n) {
                Ok(Key::Function(n))
            } else if let Some(n) = parse_numbered_name(name, "kp_") && n <= 9 {
                Ok(Key::Keypad(KeypadKey::Digit(n)))
            } else {
                Err(ConfigError::UnknownKey { name: name.to_owned(), span })
            }
        }
    }
}

fn lower_mod_name(name: &str, span: Span) -> Result<Mods, ConfigError> {
    match name {
        "shift" => Ok(Mods::SHIFT),
        "alt" => Ok(Mods::ALT),
        "ctrl" => Ok(Mods::CTRL),
        "super" => Ok(Mods::SUPER),
        "hyper" => Ok(Mods::HYPER),
        "meta" => Ok(Mods::META),
        _ => Err(ConfigError::UnknownModifier { name: name.to_owned(), span }),
    }
}

fn parse_numbered_name(name: &str, prefix: &str) -> Option<u8> {
    let rest = name.strip_prefix(prefix)?;
    if rest.is_empty() {
        return None;
    }
    if !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse::<u8>().ok()
}

fn lower_exec_command(args: Vec<Expr>, span: Span) -> Result<CommandSpec, ConfigError> {
    let mut argv = Vec::new();

    for arg in args {
        let Expr::Literal { value: Literal::String(s), .. } = arg else {
            return Err(ConfigError::BadCommandArgs { kind: "exec", span });
        };
        argv.push(s);
    }

    if argv.is_empty() || argv[0].is_empty() {
        Err(ConfigError::EmptyCommand { span })
    } else {
        Ok(CommandSpec::Exec { argv })
    }
}

fn lower_shell_command(args: Vec<Expr>, span: Span) -> Result<CommandSpec, ConfigError> {
    let command = expect_one_string(args).map_err(|_| ConfigError::BadCommandArgs {
        kind: "sh", span,
    })?;

    if command.is_empty() {
        Err(ConfigError::EmptyCommand { span })
    } else {
        Ok(CommandSpec::Shell { command })
    }
}

fn expect_call(expr: Expr) -> Result<(String, Vec<Expr>, Span), ()> {
    match expr {
        Expr::Call { name, args, span } => Ok((name, args, span)),
        _ => Err(()),
    }
}

fn expect_one_string(args: Vec<Expr>) -> Result<String, ()> {
    let mut args = args.into_iter();

    let Some(Expr::Literal { value: Literal::String(value), ..}) = args.next() else {
        return Err(());
    };

    if args.next().is_some() {
        return Err(());
    }

    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::line::{FileId, LineCtx};

    fn sp() -> Span {
        Span {
            ctx: LineCtx {
                file: FileId(0),
                line: 0,
            },
            start: 0,
            end: 0,
        }
    }

    fn ident(name: &str) -> Expr {
        Expr::Ident {
            name: name.to_owned(),
            span: sp(),
        }
    }

    fn string(s: &str) -> Expr {
        Expr::Literal {
            value: Literal::String(s.to_owned()),
            span: sp(),
        }
    }

    fn ch(ch: char) -> Expr {
        Expr::Literal {
            value: Literal::Char(ch),
            span: sp(),
        }
    }

    fn int(v: i32) -> Expr {
        Expr::Literal {
            value: Literal::Int(v),
            span: sp(),
        }
    }

    fn pair(unshifted: char, shifted: char) -> Expr {
        Expr::Pair {
            unshifted: Box::new(ch(unshifted)),
            shifted: Box::new(ch(shifted)),
            span: sp(),
        }
    }

    fn bad_pair() -> Expr {
        Expr::Pair {
            unshifted: Box::new(ident("a")),
            shifted: Box::new(ch('A')),
            span: sp(),
        }
    }

    fn call(name: &str, args: Vec<Expr>) -> Expr {
        Expr::Call {
            name: name.to_owned(),
            args,
            span: sp(),
        }
    }

    fn directive(name: &str, args: Vec<Expr>) -> Stmt {
        Stmt::Directive {
            name: name.to_owned(),
            args,
            span: sp(),
        }
    }

    fn define(kind: &str, args: Vec<Expr>) -> Stmt {
        Stmt::Definition {
            kind: kind.to_owned(),
            args,
            span: sp(),
        }
    }

    fn attr(name: &str, args: Vec<Expr>) -> MappingAttr {
        MappingAttr {
            name: name.to_owned(),
            args,
            span: sp(),
        }
    }

    fn map(lhs: Expr, rhs: Expr) -> Stmt {
        Stmt::Mapping {
            attrs: Vec::new(),
            lhs,
            op: MappingOp::Right,
            rhs,
            span: sp(),
        }
    }

    fn map_with_attrs(attrs: Vec<MappingAttr>, lhs: Expr, rhs: Expr) -> Stmt {
        Stmt::Mapping {
            attrs,
            lhs,
            op: MappingOp::Right,
            rhs,
            span: sp(),
        }
    }

    fn map_left(lhs: Expr, rhs: Expr) -> Stmt {
        Stmt::Mapping {
            attrs: Vec::new(),
            lhs,
            op: MappingOp::Left,
            rhs,
            span: sp(),
        }
    }

    fn finish(stmts: Vec<Stmt>) -> Result<Config, ConfigError> {
        let mut b = ConfigBuilder::default();
        for stmt in stmts {
            b.apply_stmt(stmt)?;
        }
        b.finish()
    }

    fn err(stmts: Vec<Stmt>) -> ConfigError {
        finish(stmts).unwrap_err()
    }

    fn group_id(config: &Config, name: &str) -> GroupId {
        config.groups.lookup(name).unwrap()
    }

    #[test]
    fn unknown_directive_is_rejected() {
        let e = err(vec![directive("bogus", vec![])]);
        assert!(matches!(e, ConfigError::UnknownDirective { name, .. } if name == "bogus"));
    }

    #[test]
    fn defines_groups_and_rejects_duplicates() {
        let config = finish(vec![
            define("group", vec![string("reload")]),
            define("group", vec![string("other")]),
        ]).unwrap();
        assert!(config.groups.lookup("reload").is_some());
        assert!(config.groups.lookup("other").is_some());

        let e = err(vec![
            define("group", vec![string("reload")]),
            define("group", vec![string("reload")]),
        ]);
        assert!(matches!(e, ConfigError::DuplicateGroup { name, .. } if name == "reload"));
    }

    #[test]
    fn rejects_bad_group_definition_args() {
        for args in [
            vec![],
            vec![ident("reload")],
            vec![string("a"), string("b")],
        ] {
            let e = err(vec![define("group", args)]);
            assert!(matches!(e, ConfigError::BadDefinitionArgs { kind: "group", .. }));
        }
    }

    #[test]
    fn rejects_unknown_definition_kind() {
        let e = err(vec![define("abab", vec![string("x")])]);
        assert!(matches!(e, ConfigError::UnknownDefinition { kind, .. } if kind == "abab"));
    }

    #[test]
    fn service_exec_is_stored() {
        let config = finish(vec![directive(
            "service",
            vec![
                string("helper"),
                call("exec", vec![string("somehelper"), string("--flag")]),
            ],
        )]).unwrap();
        assert_eq!(
            config.services,
            vec![Service {
                name: "helper".to_owned(),
                command: CommandSpec::Exec {
                    argv: vec!["somehelper".to_owned(), "--flag".to_owned()],
                },
            }],
        );
    }

    #[test]
    fn service_shell_is_stored() {
        let config = finish(vec![directive(
            "service",
            vec![string("helper"), call("sh", vec![string("echo hi")])],
        )]).unwrap();
        assert_eq!(
            config.services,
            vec![Service {
                name: "helper".to_owned(),
                command: CommandSpec::Shell {
                    command: "echo hi".to_owned(),
                },
            }],
        );
    }

    #[test]
    fn service_requires_name_and_command() {
        for args in [
            vec![],
            vec![ident("helper"), call("exec", vec![string("x")])],
            vec![string("helper")],
            vec![string("helper"), string("not-call")],
            vec![string("helper"), call("exec", vec![string("x")]), string("extra")],
            vec![string(""), call("exec", vec![string("x")])],
        ] {
            let e = err(vec![directive("service", args)]);
            eprintln!("{e}");
            assert!(matches!(e, ConfigError::BadDirectiveArgs { kind: "service", .. }));
        }
    }

    #[test]
    fn service_names_must_be_unique() {
        let e = err(vec![
            directive("service", vec![string("helper"), call("exec", vec![string("a")])]),
            directive("service", vec![string("helper"), call("exec", vec![string("b")])]),
        ]);
        assert!(matches!(e, ConfigError::DuplicateDirective { kind: "service", .. }));
    }

    #[test]
    fn command_exec_requires_string_args_and_nonempty_program() {
        for command in [
            call("exec", vec![]),
            call("exec", vec![string("")]),
        ] {
            let e = err(vec![directive("service", vec![string("helper"), command])]);
            assert!(matches!(e, ConfigError::EmptyCommand { .. }));
        }

        let e = err(vec![directive("service",
            vec![string("helper"), call("exec", vec![ident("prog")])],
        )]);
        assert!(matches!(e, ConfigError::BadCommandArgs { kind: "exec", .. }));
    }

    #[test]
    fn command_shell_requires_one_nonempty_string() {
        for command in [
            call("sh", vec![]),
            call("sh", vec![string("a"), string("b")]),
            call("sh", vec![ident("echo")]),
        ] {
            let e = err(vec![directive("service", vec![string("helper"), command])]);
            assert!(matches!(e, ConfigError::BadCommandArgs { kind: "sh", .. }));
        }

        let e = err(vec![directive("service",
            vec![string("helper"), call("sh", vec![string("")])],
        )]);
        assert!(matches!(e, ConfigError::EmptyCommand { .. }));
    }

    #[test]
    fn unknown_command_kind_is_rejected() {
        let e = err(vec![directive("service",
            vec![string("name"), call("foo", vec![string("x")])],
        )]);
        assert!(matches!(e, ConfigError::UnknownCommandKind { kind, .. } if kind == "foo"));
    }

    #[test]
    fn key_source_defaults_to_no_mods() {
        let config = finish(vec![map(
            call("key", vec![ident("f1")]),
            call("send_key", vec![ch('x')]),
        )]).unwrap();
        assert_eq!(
            config.mappings,
            vec![Mapping {
                from: Source::Token(TokenPattern::Key {
                    key: KeyPattern::Named(Key::Function(1)),
                    mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
                }),
                to: Target::Token(Token::press_utf8('x', Mods::EMPTY)),
                span: sp(),
            }],
        );
    }

    #[test]
    fn key_source_can_match_any_mods() {
        let config = finish(vec![map(
            call("key", vec![ident("tab"), ident("any")]),
            call("inherit_key", vec![pair('q', 'Q')]),
        )]).unwrap();
        assert_eq!(
            config.mappings[0].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::Named(Key::Tab),
                mods: ModsPattern::Any,
            }),
        );
    }

    #[test]
    fn key_source_can_match_explicit_no_mods() {
        let config = finish(vec![map(
            call("key", vec![ident("tab"), ident("none")]),
            call("send_key", vec![ident("enter")]),
        )]).unwrap();
        assert_eq!(
            config.mappings[0].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::Named(Key::Tab),
                mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
            }),
        );
    }

    #[test]
    fn key_source_can_match_exact_mod_mask() {
        let config = finish(vec![map(
            call("key", vec![ident("f1"), ident("ctrl"), ident("alt")]),
            call("send_key", vec![ch('x')]),
        )]).unwrap();
        assert_eq!(
            config.mappings[0].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::Named(Key::Function(1)),
                mods: ModsPattern::AnyOf(vec![Mods::CTRL | Mods::ALT]),
            }),
        );
    }

    #[test]
    fn key_source_accepts_char_pair() {
        let config = finish(vec![map(
            call("key", vec![pair('d', 'D'), ident("shift")]),
            call("inherit_key", vec![pair('w', 'W')]),
        )]).unwrap();
        assert_eq!(
            config.mappings[0].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::CharPair(CharPair {
                    unshifted: 'd',
                    shifted: 'D',
                }),
                mods: ModsPattern::AnyOf(vec![Mods::SHIFT]),
            }),
        );
        assert_eq!(
            config.mappings[0].to,
            Target::InheritToken(InheritToken::Key {
                key: KeyPattern::CharPair(CharPair {
                    unshifted: 'w',
                    shifted: 'W',
                }),
            }),
        );
    }

    #[test]
    fn send_key_accepts_named_key_and_concrete_char() {
        let config = finish(vec![
            map(
                call("key", vec![ident("f1")]),
                call("send_key", vec![ident("enter")]),
            ),
            map(
                call("key", vec![ident("f2")]),
                call("send_key", vec![ch('x'), ident("ctrl")]),
            ),
        ]).unwrap();
        assert_eq!(
            config.mappings[0].to,
            Target::Token(Token::press_key(Key::Enter, Mods::EMPTY)),
        );
        assert_eq!(
            config.mappings[1].to,
            Target::Token(Token::press_utf8('x', Mods::CTRL)),
        );
    }

    #[test]
    fn send_key_rejects_pair() {
        let e = err(vec![map(
            call("key", vec![ident("f1")]),
            call("send_key", vec![pair('x', 'X')]),
        )]);
        assert!(matches!(e, ConfigError::PairUnsupported { .. }));
    }

    #[test]
    fn inherit_key_accepts_named_key_and_char_pair() {
        let config = finish(vec![
            map(
                call("key", vec![ident("f1")]),
                call("inherit_key", vec![ident("enter")]),
            ),
            map(
                call("key", vec![ident("f2")]),
                call("inherit_key", vec![pair('x', 'X')]),
            ),
        ]).unwrap();
        assert_eq!(
            config.mappings[0].to,
            Target::InheritToken(InheritToken::Key {
                key: KeyPattern::Named(Key::Enter),
            }),
        );
        assert_eq!(
            config.mappings[1].to,
            Target::InheritToken(InheritToken::Key {
                key: KeyPattern::CharPair( CharPair {
                    unshifted: 'x',
                    shifted: 'X',
                }),
            }),
        );
    }

    #[test]
    fn inherit_key_rejects_unpaired_char_for_now() {
        let e = err(vec![map(
            call("key", vec![ident("f1")]),
            call("inherit_key", vec![ch('🙂')]),
        )]);
        assert!(matches!(e, ConfigError::BadEntityArgs { kind: "inherit_key", .. }));
    }

    #[test]
    fn char_pair_key_must_contain_chars() {
        let e = err(vec![map(
            call("key", vec![bad_pair()]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(e, ConfigError::CharPairKeyNeedsChars { .. }));
    }

    #[test]
    fn left_mapping_reverses_sides() {
        let config = finish(vec![map_left(
            call("send_key", vec![ch('x')]),
            call("key", vec![ident("f1")]),
        )]).unwrap();
        assert_eq!(
            config.mappings[0].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::Named(Key::Function(1)),
                mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
            }),
        );
        assert_eq!(
            config.mappings[0].to,
            Target::Token(Token::press_utf8('x', Mods::EMPTY)),
        );
    }

    #[test]
    fn group_mapping_uses_defined_group_id() {
        let config = finish(vec![
            define("group", vec![string("reload")]),
            map(
                call("key", vec![ident("f5")]),
                call("group", vec![string("reload")]),
            ),
        ]).unwrap();
        let reload = group_id(&config, "reload");
        assert_eq!(
            config.mappings[0],
            Mapping {
                from: Source::Token(TokenPattern::Key {
                    key: KeyPattern::Named(Key::Function(5)),
                    mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
                }),
                to: Target::Group(reload),
                span: sp(),
            },
        );
    }

    #[test]
    fn unknown_group_is_rejected() {
        let e = err(vec![map(
            call("key", vec![ident("f1")]),
            call("group", vec![string("xyz")]),
        )]);
        assert!(matches!(e, ConfigError::UnknownGroup { name, .. } if name == "xyz"));
    }

    #[test]
    fn group_self_map_is_rejected() {
        let e = err(vec![
            define("group", vec![string("g")]),
            map(call("group", vec![string("g")]), call("group", vec![string("g")])),
        ]);
        assert!(matches!(e, ConfigError::GroupSelfMap { .. }));
    }

    #[test]
    fn target_only_token_cannot_be_source() {
        let e = err(vec![map(
            call("send_key", vec![ch('x')]),
            call("send_key", vec![ch('y')]),
        )]);
        assert!(matches!(e, ConfigError::SendTokenAsSource { .. }));
    }

    #[test]
    fn source_only_token_cannot_be_target() {
        let e = err(vec![map(
            call("key", vec![ident("f1")]),
            call("key", vec![ident("f2")]),
        )]);
        assert!(matches!(e, ConfigError::SourceTokenAsTarget { .. }));
    }

    #[test]
    fn action_cannot_be_source() {
        let e = err(vec![map(
            call("sh", vec![string("echo hi")]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(e, ConfigError::ActionAsSource { .. }));
    }

    #[test]
    fn event_cannot_be_target() {
        let e = err(vec![map(
            call("key", vec![ident("f1")]),
            call("sockdata_utf8", vec![string("reload")]),
        )]);
        assert!(matches!(e, ConfigError::EventAsTarget { .. }));
    }

    #[test]
    fn inherit_token_cannot_be_source() {
        let e = err(vec![map(
            call("inherit_key", vec![pair('x', 'X')]),
            call("send_key", vec![ch('y')]),
        )]);
        assert!(matches!(e, ConfigError::InheritTokenAsSource { .. }));
    }

    #[test]
    fn inherit_token_requires_token_payload() {
        let e = err(vec![
            define("group", vec![string("g")]),
            map(call("group", vec![string("g")]), call("inherit_key", vec![pair('x', 'X')])),
        ]);
        assert!(matches!(e, ConfigError::TargetRequiresPayload { required: PayloadKind::Token, .. }));
    }

    #[test]
    fn normal_group_does_not_propagate_token_payload() {
        let e = err(vec![
            define("group", vec![string("g")]),
            map(call("key", vec![pair('d', 'D')]), call("group", vec![string("g")])),
            map(call("group", vec![string("g")]), call("inherit_key", vec![pair('w', 'W')])),
        ]);
        assert!(matches!(e, ConfigError::TargetRequiresPayload { required: PayloadKind::Token, .. }));
    }

    #[test]
    fn sockdata_utf8_source_stores_utf8_bytes() {
        let config = finish(vec![map(
            call("sockdata_utf8", vec![string("å")]),
            call("sh", vec![string("reload")]),
        )]).unwrap();
        assert_eq!(config.mappings[0].from, Source::Event(Event::Sockdata("å".as_bytes().to_vec())));
    }

    #[test]
    fn signal_source_lowers_supported_signals() {
        let config = finish(vec![map(
            call("signal", vec![string("SIGWINCH")]),
            call("sh", vec![string("resize")]),
        )]).unwrap();
        assert_eq!(config.mappings[0].from, Source::Event(Event::Signal(Signal(libc::SIGWINCH))));
    }

    #[test]
    fn signal_source_rejects_unknown_and_unsupported_signals() {
        let e = err(vec![map(
            call("signal", vec![string("SIGXYZ")]),
            call("sh", vec![string("x")]),
        )]);
        assert!(matches!(e, ConfigError::UnknownSignal { name, .. } if name == "SIGXYZ"));

        let e = err(vec![map(
            call("signal", vec![string("SIGKILL")]),
            call("sh", vec![string("x")]),
        )]);
        assert!(matches!(e, ConfigError::UnsupportedSignal { name, reason: "uncatchable", ..} if name == "SIGKILL"));

        let e = err(vec![map(
            call("signal", vec![string("SIGSEGV")]),
            call("sh", vec![string("x")]),
        )]);
        assert!(matches!(e, ConfigError::UnsupportedSignal { name, .. } if name == "SIGSEGV"));
    }

    #[test]
    fn key_accepts_known_key_names_and_function_keys() {
        let config = finish(vec![
            map(call("key", vec![ident("esc")]), call("send_key", vec![ch('a')])),
            map(call("key", vec![ident("enter")]), call("send_key", vec![ch('b')])),
            map(call("key", vec![ident("left")]), call("send_key", vec![ch('c')])),
            map(call("key", vec![ident("f35")]), call("send_key", vec![ch('d')])),
            map(call("key", vec![ident("kp_9")]), call("send_key", vec![ch('e')])),
        ]).unwrap();
        assert_eq!(
            config.mappings[0].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::Named(Key::Esc),
                mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
            }),
        );
        assert_eq!(
            config.mappings[1].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::Named(Key::Enter),
                mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
            }),
        );
        assert_eq!(
            config.mappings[2].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::Named(Key::Arrow(Direction::Left)),
                mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
            }),
        );
        assert_eq!(
            config.mappings[3].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::Named(Key::Function(35)),
                mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
            }),
        );
        assert_eq!(
            config.mappings[4].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::Named(Key::Keypad(KeypadKey::Digit(9))),
                mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
            }),
        );
    }

    #[test]
    fn key_rejects_unknown_or_out_of_range_key_names() {
        for name in ["bogus", "f0", "f36", "kp_10", "kp_"] {
            let e = err(vec![map(
                call("key", vec![ident(name)]),
                call("send_key", vec![ch('x')]),
            )]);
            assert!(matches!(e, ConfigError::UnknownKey { name: got, .. } if got == name));
        }
    }

    #[test]
    fn modifiers_are_lowered_and_duplicates_rejected() {
        let config = finish(vec![map(
            call("key", vec![ident("f1"), ident("shift"), ident("alt"), ident("ctrl")]),
            call("send_key", vec![ch('x')]),
        )]).unwrap();
        assert_eq!(
            config.mappings[0].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::Named(Key::Function(1)),
                mods: ModsPattern::AnyOf(vec![Mods::SHIFT | Mods::ALT | Mods::CTRL]),
            }),
        );

        let e = err(vec![map(
            call("key", vec![ident("f1"), ident("ctrl"), ident("ctrl")]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(e, ConfigError::DuplicateModifier { name, .. } if name == "ctrl"));
    }

    #[test]
    fn unknown_or_non_ident_modifier_is_rejected() {
        let e = err(vec![map(
            call("key", vec![ident("f1"), ident("bogus")]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(e, ConfigError::UnknownModifier { name, .. } if name == "bogus"));

        let e = err(vec![map(
            call("key", vec![ident("f1"), string("ctrl")]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(e, ConfigError::BadModifier { .. }));
    }

    #[test]
    fn exec_and_sh_lower_to_actions() {
        let config = finish(vec![
            map(
                call("key", vec![ident("f1")]),
                call("exec", vec![string("prog"), string("arg")]),
            ),
            map(
                call("key", vec![ident("f2")]),
                call("sh", vec![string("echo hi")]),
            ),
        ]).unwrap();
        assert_eq!(
            config.mappings[0].to,
            Target::Action(Action::Command(CommandSpec::Exec {
                argv: vec!["prog".to_owned(), "arg".to_owned()],
            })),
        );
        assert_eq!(
            config.mappings[1].to,
            Target::Action(Action::Command(CommandSpec::Shell {
                command: "echo hi".to_owned(),
            })),
        );
    }

    #[test]
    fn unknown_entity_is_rejected() {
        let e = err(vec![map(
            call("key", vec![ident("f1")]),
            call("bogus", vec![]),
        )]);
        assert!(matches!(e,ConfigError::UnknownEntity { name, .. } if name == "bogus"));
    }

    #[test]
    fn unknown_mapping_attr_is_rejected() {
        let e = err(vec![map_with_attrs(
            vec![attr("xyz", vec![])],
            call("key", vec![ident("f1")]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(e, ConfigError::UnsupportedMappingAttr { name, .. } if name == "xyz"));
    }
}
