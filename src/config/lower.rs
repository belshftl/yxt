// SPDX-License-Identifier: MIT

use std::collections::HashSet;

use crate::model::*;

use super::line::{Arg, Expr, Literal, MappingOp, Span, Stmt};
use super::options::Options;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralKind {
    Bool,
    Int,
    String,
}

impl LiteralKind {
    pub fn of(value: &Literal) -> Self {
        match value {
            Literal::Bool(_) => Self::Bool,
            Literal::Int(_) => Self::Int,
            Literal::String(_) => Self::String,
        }
    }
}

use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum ConfigError {
    #[error("unknown directive '@{name}'")]
    UnknownDirective { name: String, span: Span },

    #[error("bad arguments for directive '@{kind}'")]
    BadDirectiveArgs { kind: &'static str, span: Span },

    #[error("duplicate of directive '@{kind}' is not allowed: {reason}")]
    DuplicateDirective { kind: &'static str, reason: &'static str, span: Span },

    #[error("unknown command kind '{kind}'")]
    UnknownCommandKind { kind: String, span: Span },

    #[error("bad arguments for command of kind '{kind}'")]
    BadCommandArgs { kind: &'static str, span: Span },

    #[error("command cannot be empty")]
    EmptyCommand { span: Span },

    #[error("unknown definition kind '{kind}'")]
    UnknownDefinition { kind: String, span: Span },

    #[error("bad arguments for definition '{kind}'")]
    BadDefinitionArgs { kind: &'static str, span: Span },

    #[error("duplicate group '{name}'")]
    DuplicateGroup { name: String, span: Span },

    #[error("unknown group '{name}'")]
    UnknownGroup { name: String, span: Span },

    #[error("unknown option '{name}'")]
    UnknownOption { name: String, span: Span },

    #[error("wrong literal type: expected '{expected:?}', got '{got:?}'")]
    WrongLiteralType { expected: LiteralKind, got: LiteralKind, span: Span },

    #[error("unknown entity '{name}'")]
    UnknownEntity { name: String, span: Span },

    #[error("bad arguments for entity '{kind}'")]
    BadEntityArgs { kind: &'static str, span: Span },

    #[error("unknown signal '{name}'")]
    UnknownSignal { name: String, span: Span },

    #[error("signal '{name}' is {reason}")]
    UnsupportedSignal { name: String, reason: &'static str, span: Span },

    #[error("tok_utf8() string must be a single unicode character")]
    TokUtf8NeedsOneChar { span: Span },

    #[error("unknown key '{name}'")]
    UnknownKey { name: String, span: Span },

    #[error("unknown modifier '{name}'")]
    UnknownModifier { name: String, span: Span },

    #[error("duplicate modifier '{name}'")]
    DuplicateModifier { name: String, span: Span },

    #[error("bad modifier argument")]
    BadModifier { span: Span },

    #[error("action cannot be used as mapping source")]
    ActionAsSource { span: Span },

    #[error("event cannot be used as mapping target")]
    EventAsTarget { span: Span },

    #[error("cannot map a group to itself")]
    GroupSelfMap { span: Span },
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
            Stmt::Mapping { lhs, op, rhs, span } => self.apply_mapping(lhs, op, rhs, span),
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

    fn apply_directive(&mut self, name: String, args: Vec<Arg>, span: Span) -> Result<(), ConfigError> {
        match name.as_str() {
            "protocol" => todo!(),
            "service" => self.apply_service(args, span),
            _ => Err(ConfigError::UnknownDirective { name, span }),
        }
    }

    fn apply_service(&mut self, args: Vec<Arg>, span: Span) -> Result<(), ConfigError> {
        let mut args = args.into_iter();
        let Some(Arg::String(name)) = args.next() else {
            return Err(ConfigError::BadDirectiveArgs { kind: "service", span });
        };
        if name.is_empty() {
            return Err(ConfigError::BadDirectiveArgs { kind: "service", span });
        }
        let Some(Arg::Call(command)) = args.next() else {
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
        let command = lower_command_spec(command)?;
        self.services.push(Service { name, command });
        Ok(())
    }

    fn apply_definition(&mut self, kind: String, args: Vec<Arg>, span: Span) -> Result<(), ConfigError> {
        match kind.as_str() {
            "group" => {
                let name = expect_one_string(args).map_err(|_| ConfigError::BadDefinitionArgs {
                    kind: "group", span
                })?;
                self.groups.define(name).map_err(|e| match e {
                    DefineGroupError::Duplicate(name) => ConfigError::DuplicateGroup { name, span }
                })?;
                Ok(())
            }
            _ => Err(ConfigError::UnknownDefinition { kind, span }),
        }
    }

    fn apply_mapping(&mut self, lhs: Expr, op: MappingOp, rhs: Expr, span: Span) -> Result<(), ConfigError> {
        let (from_expr, to_expr) = match op {
            MappingOp::Right => (lhs, rhs),
            MappingOp::Left => (rhs, lhs),
        };
        let from = self.lower_source(from_expr)?;
        let to = self.lower_target(to_expr)?;

        if let (Source::Group(a), Target::Group(b)) = (&from, &to) {
            if a == b {
                return Err(ConfigError::GroupSelfMap { span });
            }
        }
        self.mappings.push(Mapping { from, to, span });
        Ok(())
    }

    fn lower_source(&self, expr: Expr) -> Result<Source, ConfigError> {
        let span = expr.span();
        match self.lower_entity(expr)? {
            Entity::Event(x) => Ok(Source::Event(x)),
            Entity::Token(x) => Ok(Source::Token(x)),
            Entity::Group(x) => Ok(Source::Group(x)),
            Entity::Action(_) => Err(ConfigError::ActionAsSource { span }),
        }
    }

    fn lower_target(&self, expr: Expr) -> Result<Target, ConfigError> {
        let span = expr.span();
        match self.lower_entity(expr)? {
            Entity::Token(x) => Ok(Target::Token(x)),
            Entity::Group(x) => Ok(Target::Group(x)),
            Entity::Action(x) => Ok(Target::Action(x)),
            Entity::Event(_) => Err(ConfigError::EventAsTarget { span }),
        }
    }

    fn lower_entity(&self, expr: Expr) -> Result<Entity, ConfigError> {
        match expr {
            Expr::Call { name, args, span } => match name.as_str() {
                "evt_signal" => lower_evt_signal(args, span),
                "evt_sockdata_utf8" => lower_evt_sockdata_utf8(args, span),
                "tok_utf8" => lower_tok_utf8(args, span),
                "tok_key" => lower_tok_key(args, span),
                "group" => {
                    let group_name = expect_one_string(args).map_err(|_| ConfigError::BadEntityArgs {
                        kind: "group", span
                    })?;
                    let id = self.groups.lookup(&group_name).ok_or_else(|| {
                        ConfigError::UnknownGroup { name: group_name, span }
                    })?;
                    Ok(Entity::Group(id))
                }
                "act_exec" => Ok(Entity::Action(Action::Command(lower_exec_command(args, span)?))),
                "act_shell" => Ok(Entity::Action(Action::Command(lower_shell_command(args, span)?))),
                _ => Err(ConfigError::UnknownEntity { name, span }),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Entity {
    Event(Event),
    Token(Token),
    Group(GroupId),
    Action(Action),
}

fn lower_evt_signal(args: Vec<Arg>, span: Span) -> Result<Entity, ConfigError> {
    let name = expect_one_string(args).map_err(|_| ConfigError::BadEntityArgs {
        kind: "evt_signal", span,
    })?;
    let signal = lower_signal_name(&name, span)?;
    Ok(Entity::Event(Event::Signal(signal)))
}

fn lower_evt_sockdata_utf8(args: Vec<Arg>, span: Span) -> Result<Entity, ConfigError> {
    let s = expect_one_string(args).map_err(|_| ConfigError::BadEntityArgs {
        kind: "evt_sockdata_utf8", span,
    })?;
    Ok(Entity::Event(Event::Sockdata(s.as_bytes().to_vec())))
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
            span
        }),
        "SIGILL" | "SIGABRT" | "SIGFPE" | "SIGSEGV" | "SIGBUS" | "SIGTRAP" | "SIGSYS" => Err(ConfigError::UnsupportedSignal {
            name: name.to_owned(),
            reason: "unsupported; error signals are not supported as events",
            span
        }),
        _ => Err(ConfigError::UnknownSignal { name: name.to_owned(), span })
    }
}

fn lower_tok_utf8(args: Vec<Arg>, span: Span) -> Result<Entity, ConfigError> {
    let mut args = args.into_iter();
    let Some(Arg::String(v)) = args.next() else {
        return Err(ConfigError::BadEntityArgs { kind: "tok_utf8", span });
    };
    let mut chars = v.chars();
    let Some(ch) = chars.next() else {
        return Err(ConfigError::BadEntityArgs { kind: "tok_utf8", span });
    };
    if chars.next().is_some() {
        return Err(ConfigError::BadEntityArgs { kind: "tok_utf8", span });
    }
    let mods = lower_mods(args, span)?;
    Ok(Entity::Token(Token::press_utf8(ch, mods)))
}

fn lower_tok_key(args: Vec<Arg>, span: Span) -> Result<Entity, ConfigError> {
    let mut args = args.into_iter();
    let Some(Arg::Ident(v)) = args.next() else {
        return Err(ConfigError::BadEntityArgs { kind: "tok_key", span });
    };
    let key = lower_key_name(&v, span)?;
    let mods = lower_mods(args, span)?;
    Ok(Entity::Token(Token::press_key(key, mods)))
}

fn lower_mods(args: impl IntoIterator<Item = Arg>, span: Span) -> Result<Mods, ConfigError> {
    let mut mods = Mods::EMPTY;
    for arg in args {
        let Arg::Ident(name) = arg else {
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
            if let Some(n) = parse_numbered_name(name, "f") && (n >= 1 && n <= 35) {
                return Ok(Key::Function(n));
            }
            if let Some(n) = parse_numbered_name(name, "kp_") && n <= 9 {
                return Ok(Key::Keypad(KeypadKey::Digit(n)));
            }
            Err(ConfigError::UnknownKey { name: name.to_owned(), span })
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

fn lower_command_spec(expr: Expr) -> Result<CommandSpec, ConfigError> {
    match expr {
        Expr::Call { name, args, span } => match name.as_str() {
            "exec" => lower_exec_command(args, span),
            "sh" => lower_shell_command(args, span),
            _ => Err(ConfigError::UnknownCommandKind { kind: name, span }),
        },
    }
}

fn lower_exec_command(args: Vec<Arg>, span: Span) -> Result<CommandSpec, ConfigError> {
    let mut argv = Vec::new();
    for arg in args {
        let Arg::String(s) = arg else {
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

fn lower_shell_command(args: Vec<Arg>, span: Span) -> Result<CommandSpec, ConfigError> {
    let command = expect_one_string(args).map_err(|_| ConfigError::BadCommandArgs {
        kind: "sh", span,
    })?;
    if command.is_empty() {
        Err(ConfigError::EmptyCommand { span })
    } else {
        Ok(CommandSpec::Shell { command })
    }
}

fn expect_one_string(args: Vec<Arg>) -> Result<String, ()> {
    let mut args = args.into_iter();
    let Some(Arg::String(value)) = args.next() else {
        return Err(());
    };
    if args.next().is_some() {
        return Err(());
    }
    Ok(value)
}

trait ExprExt {
    fn span(&self) -> Span;
}

impl ExprExt for Expr {
    fn span(&self) -> Span {
        match self {
            Expr::Call { span, .. } => *span,
        }
    }
}
