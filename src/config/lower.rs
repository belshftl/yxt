// SPDX-FileCopyrightText: 2026 belshftl
// SPDX-License-Identifier: MIT

use std::collections::HashSet;

use super::ast::{Expr, InfixOp, Literal, MappingAttr, MappingOp, PairSide, Span, Stmt};
use super::options::Options;
use crate::model::{
    Action, CharPair, CommandSpec, Config, DefineGroupError, Direction, Event, GroupId, GroupTable,
    InheritToken, Key, KeyPattern, KeypadKey, Mapping, MappingAttrs, MediaKey, ModifierKey, Mods,
    ModsPattern, PayloadKind, Protocol, ProtocolRequest, ProtocolVerb, Service, Signal, Source,
    Target, ToggleOp, Token, TokenPattern,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LoweredCharPair {
    pair: CharPair,
    default_mods: Mods,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProtocolNeed {
    protocol: Protocol,
    span: Span,
}

impl ProtocolNeed {
    fn of(protocol: Protocol, span: Span) -> Option<Self> {
        (protocol > Protocol::Legacy).then_some(Self { protocol, span })
    }

    fn max(first: Option<Self>, second: Option<Self>) -> Option<Self> {
        match (first, second) {
            (Some(first), Some(second)) => Some(if second.protocol > first.protocol {
                second
            } else {
                first
            }),
            (first, second) => first.or(second),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ModsAlt {
    mods: Mods,
    need: Option<ProtocolNeed>,
}

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
pub enum ErrorKind {
    #[error("unknown directive '@{name}'")]
    UnknownDirective { name: String },

    #[error("invalid arguments for directive '@{kind}'")]
    BadDirectiveArgs { kind: &'static str },

    #[error("duplicate of directive '@{kind}' is not allowed: {reason}")]
    DuplicateDirective {
        kind: &'static str,
        reason: &'static str,
    },

    #[error("unknown command kind '{kind}'")]
    UnknownCommandKind { kind: String },

    #[error("invalid arguments for command of kind '{kind}'")]
    BadCommandArgs { kind: &'static str },

    #[error("command cannot be empty")]
    EmptyCommand,

    #[error("unknown protocol verb '{verb}'")]
    UnknownProtocolVerb { verb: String },

    #[error("protocol verb '{verb}' is not yet supported")]
    UnsupportedProtocolVerb { verb: &'static str },

    #[error("unknown protocol '{name}'")]
    UnknownProtocol { name: String },

    #[error("'@protocol' must come before any mappings")]
    ProtocolAfterMappings,

    #[error(
        "\
this needs the '{needs}' protocol, whereas only '{have}' is specified; \
add '@protocol want {needs}' before any mappings"
    )]
    SourceNeedsProtocol {
        needs: &'static str,
        have: &'static str,
    },

    #[error("unknown definition kind '{kind}'")]
    UnknownDefinition { kind: String },

    #[error("invalid arguments for definition '{kind}'")]
    BadDefinitionArgs { kind: &'static str },

    #[error("duplicate group '{name}'")]
    DuplicateGroup { name: String },

    #[error("unknown group '{name}'")]
    UnknownGroup { name: String },

    #[error("unknown option '{name}'")]
    UnknownOption { name: String },

    #[error("wrong literal type: expected '{expected:?}', got '{got:?}'")]
    WrongLiteralType {
        expected: LiteralKind,
        got: LiteralKind,
    },

    #[error("bad value for option '{name}': {desc}")]
    BadOptionValue { name: String, desc: &'static str },

    #[error("unknown entity '{name}'")]
    UnknownEntity { name: String },

    #[error("invalid arguments for entity '{kind}'")]
    BadEntityArgs { kind: &'static str },

    #[error("unknown mapping attribute '{name}'")]
    UnsupportedMappingAttr { name: String },

    #[error("invalid arguments for mapping attribute '{kind}'")]
    BadMappingAttrArgs { kind: &'static str },

    #[error("duplicate mapping attribute '{kind}'")]
    DuplicateMappingAttr { kind: &'static str },

    #[error("'passthrough!' mapping attribute is only valid for token sources")]
    InvalidPassthroughSource,

    #[error(
        "'always!' mapping attribute has no effect on a group mapping, which only ever resolves through expansion"
    )]
    InvalidAlwaysSource,

    #[error("unknown toggle operation '{name}' (expected 'on', 'off' or 'toggle')")]
    UnknownToggleOp { name: String },

    #[error("pair expressions are not supported here")]
    PairUnsupported,

    #[error("char-pair key must contain two character literals")]
    CharPairKeyNeedsChars,

    #[error("can't infer char pair for key from character '{ch}'")]
    CantInferTextPair { ch: char },

    #[error("unknown signal '{name}'")]
    UnknownSignal { name: String },

    #[error("signal '{name}' is {reason}")]
    UnsupportedSignal { name: String, reason: &'static str },

    #[error("unknown key '{name}'")]
    UnknownKey { name: String },

    #[error("unknown modifier '{name}'")]
    UnknownModifier { name: String },

    #[error("duplicate modifier '{name}'")]
    DuplicateModifier { name: String },

    #[error("expected concrete modifier set, not pattern")]
    NeedNonPatModSet,

    #[error("invalid modifier pattern/set")]
    BadMods,

    #[error("too many args for modifier pattern (expected 0 or 1)")]
    TooManyModPatternArgs,

    #[error("'any' must be the entire modifier pattern, not part of a pattern")]
    AnyModsPatMustBeAlone,

    #[error("target-only token constructor cannot be used as mapping source")]
    SendTokenAsSource,

    #[error("source-only token constructor cannot be used as mapping target")]
    SourceTokenAsTarget,

    #[error("action cannot be used as mapping source")]
    ActionAsSource,

    #[error("inherit token cannot be used as mapping source")]
    InheritTokenAsSource,

    #[error("event cannot be used as mapping target")]
    EventAsTarget,

    #[error("target requires payload of type {required:?} that the mapped source did not provide")]
    TargetRequiresPayload { required: PayloadKind },

    #[error("cannot map a group to itself")]
    GroupSelfMap,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("byte {}..{}: {kind}", .span.start, .span.end)]
pub struct ConfigError {
    pub kind: ErrorKind,
    pub span: Span,
}

#[derive(Debug, Default)]
pub struct ConfigBuilder {
    options: Options,
    protocol: ProtocolRequest,
    groups: GroupTable,
    mappings: Vec<Mapping>,
    services: Vec<Service>,
    sv_names: HashSet<String>,
}

impl ConfigBuilder {
    pub fn apply_stmt(&mut self, stmt: Stmt) -> Result<(), ConfigError> {
        match stmt {
            Stmt::Directive { name, args, span } => self.apply_directive(name, args, span),
            Stmt::Definition { kind, args, span } => self.apply_definition(kind, args, span),
            Stmt::Mapping {
                attrs,
                lhs,
                op,
                rhs,
                span,
            } => self.apply_mapping(attrs, lhs, op, rhs, span),
            Stmt::OptionAssignment { name, val, span } => self.options.set(name, val, span),
        }
    }

    pub fn finish(self) -> Config {
        Config {
            options: self.options,
            protocol: self.protocol,
            groups: self.groups,
            mappings: self.mappings,
            services: self.services,
        }
    }

    pub fn take_finish(&mut self) -> Config {
        std::mem::take(self).finish()
    }

    fn apply_directive(
        &mut self,
        name: String,
        args: Vec<Expr>,
        span: Span,
    ) -> Result<(), ConfigError> {
        match name.as_str() {
            "protocol" => self.apply_protocol(&args, span),
            "service" => self.apply_service(args, span),
            _ => Err(ConfigError {
                kind: ErrorKind::UnknownDirective { name },
                span,
            }),
        }
    }

    fn apply_protocol(&mut self, args: &[Expr], span: Span) -> Result<(), ConfigError> {
        if !self.mappings.is_empty() {
            return Err(ConfigError {
                kind: ErrorKind::ProtocolAfterMappings,
                span,
            });
        }

        let [verb, protocol] = args else {
            return Err(ConfigError {
                kind: ErrorKind::BadDirectiveArgs { kind: "protocol" },
                span,
            });
        };
        let (verb_name, verb_span) = expect_ident(verb).map_err(|()| ConfigError {
            kind: ErrorKind::BadDirectiveArgs { kind: "protocol" },
            span,
        })?;
        let (protocol_name, protocol_span) = expect_ident(protocol).map_err(|()| ConfigError {
            kind: ErrorKind::BadDirectiveArgs { kind: "protocol" },
            span,
        })?;

        let verb = match verb_name {
            "want" => ProtocolVerb::Want,
            "require" => {
                return Err(ConfigError {
                    kind: ErrorKind::UnsupportedProtocolVerb { verb: "require" },
                    span: verb_span,
                });
            }
            _ => {
                return Err(ConfigError {
                    kind: ErrorKind::UnknownProtocolVerb {
                        verb: verb_name.to_owned(),
                    },
                    span: verb_span,
                });
            }
        };

        let protocol = Protocol::from_name(protocol_name).ok_or_else(|| ConfigError {
            kind: ErrorKind::UnknownProtocol {
                name: protocol_name.to_owned(),
            },
            span: protocol_span,
        })?;

        if protocol >= self.protocol.protocol {
            self.protocol = ProtocolRequest { verb, protocol };
        }
        Ok(())
    }

    fn apply_service(&mut self, args: Vec<Expr>, span: Span) -> Result<(), ConfigError> {
        let mut args = args.into_iter();

        let Some(Expr::Literal {
            value: Literal::String(name),
            ..
        }) = args.next()
        else {
            return Err(ConfigError {
                kind: ErrorKind::BadDirectiveArgs { kind: "service" },
                span,
            });
        };

        if name.is_empty() {
            return Err(ConfigError {
                kind: ErrorKind::BadDirectiveArgs { kind: "service" },
                span,
            });
        }

        let Some(expr) = args.next() else {
            return Err(ConfigError {
                kind: ErrorKind::BadDirectiveArgs { kind: "service" },
                span,
            });
        };

        if args.next().is_some() {
            return Err(ConfigError {
                kind: ErrorKind::BadDirectiveArgs { kind: "service" },
                span,
            });
        }

        if !self.sv_names.insert(name.clone()) {
            return Err(ConfigError {
                kind: ErrorKind::DuplicateDirective {
                    kind: "service",
                    reason: "duplicate service name; services must have unique names",
                },
                span,
            });
        }

        let (call_name, call_args, call_span) = expect_call(expr).map_err(|()| ConfigError {
            kind: ErrorKind::BadDirectiveArgs { kind: "service" },
            span,
        })?;
        let command = match call_name.as_str() {
            "exec" => lower_exec_command(call_args, call_span)?,
            "sh" => lower_shell_command(call_args, call_span)?,
            _ => {
                return Err(ConfigError {
                    kind: ErrorKind::UnknownCommandKind { kind: call_name },
                    span: call_span,
                });
            }
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
                let name = expect_one_string(args).map_err(|()| ConfigError {
                    kind: ErrorKind::BadDefinitionArgs { kind: "group" },
                    span,
                })?;
                self.groups.define(name).map_err(|e| match e {
                    DefineGroupError::Duplicate(name) => ConfigError {
                        kind: ErrorKind::DuplicateGroup { name },
                        span,
                    },
                })?;
                Ok(())
            }
            _ => Err(ConfigError {
                kind: ErrorKind::UnknownDefinition { kind },
                span,
            }),
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
        let (from_expr, to_expr) = match op {
            MappingOp::Right => (lhs, rhs),
            MappingOp::Left => (rhs, lhs),
        };
        let (from, need) = self.lower_source(from_expr)?;

        if let Some(need) = need
            && need.protocol > self.protocol.protocol
        {
            return Err(ConfigError {
                kind: ErrorKind::SourceNeedsProtocol {
                    needs: need.protocol.name(),
                    have: self.protocol.protocol.name(),
                },
                span: need.span,
            });
        }

        let to = self.lower_target(to_expr)?;
        let attrs = lower_mapping_attrs(attrs, &from)?;
        let required = to.requires_payload();
        if let Some(required) = required
            && from.provides_payload() != Some(required)
        {
            Err(ConfigError {
                kind: ErrorKind::TargetRequiresPayload { required },
                span,
            })
        } else if let (Source::Group(a), Target::Group(b)) = (&from, &to)
            && a == b
        {
            Err(ConfigError {
                kind: ErrorKind::GroupSelfMap,
                span,
            })
        } else {
            self.mappings.push(Mapping {
                from,
                to,
                attrs,
                span,
            });
            Ok(())
        }
    }

    fn lower_source(&self, expr: Expr) -> Result<(Source, Option<ProtocolNeed>), ConfigError> {
        let span = expr.span();
        let (name, args, call_span) = expect_call(expr).map_err(|()| ConfigError {
            kind: ErrorKind::UnknownEntity {
                name: "<non-call>".to_owned(),
            },
            span,
        })?;
        match name.as_str() {
            "signal" => Ok((lower_signal_source(args, call_span)?, None)),
            "sockdata_utf8" => Ok((lower_sockdata_utf8_source(args, call_span)?, None)),
            "key" => lower_key_source(args, call_span),
            "group" => Ok((Source::Group(self.lower_group_id(args, call_span)?), None)),
            "send_key" => Err(ConfigError {
                kind: ErrorKind::SendTokenAsSource,
                span,
            }),
            "inherit_key" => Err(ConfigError {
                kind: ErrorKind::InheritTokenAsSource,
                span,
            }),
            "exec" | "sh" | "toggle_mappings" => Err(ConfigError {
                kind: ErrorKind::ActionAsSource,
                span,
            }),
            _ => Err(ConfigError {
                kind: ErrorKind::UnknownEntity { name },
                span: call_span,
            }),
        }
    }

    fn lower_target(&self, expr: Expr) -> Result<Target, ConfigError> {
        let span = expr.span();
        let (name, args, call_span) = expect_call(expr).map_err(|()| ConfigError {
            kind: ErrorKind::UnknownEntity {
                name: "<non-call>".to_owned(),
            },
            span,
        })?;
        match name.as_str() {
            "send_key" => lower_send_key(args, call_span),
            "inherit_key" => lower_inherit_key(args, call_span),
            "group" => Ok(Target::Group(self.lower_group_id(args, call_span)?)),
            "exec" => Ok(Target::Action(Action::Command(lower_exec_command(
                args, call_span,
            )?))),
            "sh" => Ok(Target::Action(Action::Command(lower_shell_command(
                args, call_span,
            )?))),
            "toggle_mappings" => Ok(Target::Action(Action::ToggleMappings(lower_toggle_op(
                args, call_span,
            )?))),
            "signal" | "sockdata_utf8" => Err(ConfigError {
                kind: ErrorKind::EventAsTarget,
                span,
            }),
            "key" => Err(ConfigError {
                kind: ErrorKind::SourceTokenAsTarget,
                span,
            }),
            _ => Err(ConfigError {
                kind: ErrorKind::UnknownEntity { name },
                span: call_span,
            }),
        }
    }

    fn lower_group_id(&self, args: Vec<Expr>, span: Span) -> Result<GroupId, ConfigError> {
        let name = expect_one_string(args).map_err(|()| ConfigError {
            kind: ErrorKind::BadEntityArgs { kind: "group" },
            span,
        })?;
        self.groups.lookup(&name).ok_or(ConfigError {
            kind: ErrorKind::UnknownGroup { name },
            span,
        })
    }
}

fn lower_mapping_attrs(
    attrs: Vec<MappingAttr>,
    from: &Source,
) -> Result<MappingAttrs, ConfigError> {
    let mut out = MappingAttrs::default();
    for attr in attrs {
        match attr.name.as_str() {
            "passthrough" => {
                if !attr.args.is_empty() {
                    return Err(ConfigError {
                        kind: ErrorKind::BadMappingAttrArgs {
                            kind: "passthrough",
                        },
                        span: attr.span,
                    });
                }
                if out.passthrough {
                    return Err(ConfigError {
                        kind: ErrorKind::DuplicateMappingAttr {
                            kind: "passthrough",
                        },
                        span: attr.span,
                    });
                }
                if !matches!(from, Source::Token(_)) {
                    return Err(ConfigError {
                        kind: ErrorKind::InvalidPassthroughSource,
                        span: attr.span,
                    });
                }
                out.passthrough = true;
            }
            "always" => {
                if !attr.args.is_empty() {
                    return Err(ConfigError {
                        kind: ErrorKind::BadMappingAttrArgs { kind: "always" },
                        span: attr.span,
                    });
                }
                if out.always {
                    return Err(ConfigError {
                        kind: ErrorKind::DuplicateMappingAttr { kind: "always" },
                        span: attr.span,
                    });
                }
                // a group only ever fires from a mapping that already resolved, so it's never
                // gated in the first place
                if matches!(from, Source::Group(_)) {
                    return Err(ConfigError {
                        kind: ErrorKind::InvalidAlwaysSource,
                        span: attr.span,
                    });
                }
                out.always = true;
            }
            _ => {
                return Err(ConfigError {
                    kind: ErrorKind::UnsupportedMappingAttr { name: attr.name },
                    span: attr.span,
                });
            }
        }
    }
    Ok(out)
}

fn lower_toggle_op(args: Vec<Expr>, span: Span) -> Result<ToggleOp, ConfigError> {
    let mut args = args.into_iter();

    let Some(Expr::Ident {
        name,
        span: op_span,
    }) = args.next()
    else {
        return Err(ConfigError {
            kind: ErrorKind::BadEntityArgs {
                kind: "toggle_mappings",
            },
            span,
        });
    };

    if args.next().is_some() {
        return Err(ConfigError {
            kind: ErrorKind::BadEntityArgs {
                kind: "toggle_mappings",
            },
            span,
        });
    }

    match name.as_str() {
        "on" => Ok(ToggleOp::On),
        "off" => Ok(ToggleOp::Off),
        "toggle" => Ok(ToggleOp::Toggle),
        _ => Err(ConfigError {
            kind: ErrorKind::UnknownToggleOp { name },
            span: op_span,
        }),
    }
}

fn lower_signal_source(args: Vec<Expr>, span: Span) -> Result<Source, ConfigError> {
    let name = expect_one_string(args).map_err(|()| ConfigError {
        kind: ErrorKind::BadEntityArgs { kind: "signal" },
        span,
    })?;
    let signal = lower_signal_name(&name, span)?;
    Ok(Source::Event(Event::Signal(signal)))
}

fn lower_sockdata_utf8_source(args: Vec<Expr>, span: Span) -> Result<Source, ConfigError> {
    let s = expect_one_string(args).map_err(|()| ConfigError {
        kind: ErrorKind::BadEntityArgs {
            kind: "sockdata_utf8",
        },
        span,
    })?;
    Ok(Source::Event(Event::Sockdata(s.as_bytes().to_vec())))
}

fn lower_key_source(
    args: Vec<Expr>,
    span: Span,
) -> Result<(Source, Option<ProtocolNeed>), ConfigError> {
    let mut args = args.into_iter();
    let Some(key_expr) = args.next() else {
        return Err(ConfigError {
            kind: ErrorKind::BadEntityArgs { kind: "key" },
            span,
        });
    };
    let key_span = key_expr.span();
    let (key, dfl) = lower_key_pattern_arg(key_expr, "key", span)?;
    let key_need = match key {
        KeyPattern::Named(key) => ProtocolNeed::of(key.required_protocol(), key_span),
        KeyPattern::CharPair(_) => None, // a char on its own is always just text
    };

    let mod_args: Vec<Expr> = args.collect();
    // a key and a modifier set that are each fine alone can still be unreportable together, so
    // there is no one argument to blame for it; point at the whole list instead
    let args_span = Span {
        ctx: key_span.ctx,
        start: key_span.start,
        end: mod_args.last().map_or(key_span.end, |e| e.span().end),
    };

    let (mods, mods_need) = lower_mods_pattern(mod_args, span, dfl)?;
    let combined_need = match &mods {
        // `any` intentionally means "anything effectively catchable under the current protocol"
        ModsPattern::Any => None,
        ModsPattern::AnyOf(alts) => alts
            .iter()
            .map(|alt| ProtocolNeed::of(key.required_protocol(*alt), args_span))
            .fold(None, ProtocolNeed::max),
    };

    Ok((
        Source::Token(TokenPattern::Key { key, mods }),
        // the more precisely blamed needs come first so that they win an equal-rank tie
        ProtocolNeed::max(ProtocolNeed::max(key_need, mods_need), combined_need),
    ))
}

fn lower_send_key(args: Vec<Expr>, span: Span) -> Result<Target, ConfigError> {
    let mut args = args.into_iter();
    let Some(key_expr) = args.next() else {
        return Err(ConfigError {
            kind: ErrorKind::BadEntityArgs { kind: "send_key" },
            span,
        });
    };
    let mods = lower_mods(args.collect(), span)?;
    match key_expr {
        Expr::Ident { name, .. } => Ok(Target::Token(Token::press_key(
            lower_key_name(&name, span)?,
            mods,
        ))),
        Expr::Literal {
            value: Literal::Char(ch),
            ..
        } => Ok(Target::Token(Token::press_utf8(ch, mods))),
        Expr::Pair { span, .. } | Expr::InferPair { span, .. } => Err(ConfigError {
            kind: ErrorKind::PairUnsupported,
            span,
        }),
        _ => Err(ConfigError {
            kind: ErrorKind::BadEntityArgs { kind: "send_key" },
            span,
        }),
    }
}

fn lower_inherit_key(args: Vec<Expr>, span: Span) -> Result<Target, ConfigError> {
    let mut args = args.into_iter();
    let Some(expr) = args.next() else {
        return Err(ConfigError {
            kind: ErrorKind::BadEntityArgs {
                kind: "inherit_key",
            },
            span,
        });
    };
    if args.next().is_some() {
        return Err(ConfigError {
            kind: ErrorKind::BadEntityArgs {
                kind: "inherit_key",
            },
            span,
        });
    }
    let (key, _) = lower_key_pattern_arg(expr, "inherit_key", span)?;
    Ok(Target::InheritToken(InheritToken::Key { key }))
}

fn lower_key_pattern_arg(
    expr: Expr,
    kind: &'static str,
    span: Span,
) -> Result<(KeyPattern, Mods), ConfigError> {
    match expr {
        Expr::Ident { name, .. } => {
            Ok((KeyPattern::Named(lower_key_name(&name, span)?), Mods::EMPTY))
        }
        Expr::Pair { .. } | Expr::InferPair { .. } => {
            let l = lower_char_pair_expr(expr)?;
            Ok((KeyPattern::CharPair(l.pair), l.default_mods))
        }
        _ => Err(ConfigError {
            kind: ErrorKind::BadEntityArgs { kind },
            span,
        }),
    }
}

fn lower_char_pair_expr(expr: Expr) -> Result<LoweredCharPair, ConfigError> {
    match expr {
        Expr::Pair {
            unshifted,
            shifted,
            span,
        } => {
            let Some(unshifted) = literal_char(&unshifted) else {
                return Err(ConfigError {
                    kind: ErrorKind::CharPairKeyNeedsChars,
                    span,
                });
            };
            let Some(shifted) = literal_char(&shifted) else {
                return Err(ConfigError {
                    kind: ErrorKind::CharPairKeyNeedsChars,
                    span,
                });
            };
            Ok(LoweredCharPair {
                pair: CharPair { unshifted, shifted },
                default_mods: Mods::EMPTY,
            })
        }
        Expr::InferPair { known, side, span } => {
            let Some(ch) = literal_char(&known) else {
                return Err(ConfigError {
                    kind: ErrorKind::CharPairKeyNeedsChars,
                    span,
                });
            };
            let pair = match side {
                PairSide::Unshifted => infer_us_pair_from_unshifted(ch),
                PairSide::Shifted => infer_us_pair_from_shifted(ch),
            }
            .ok_or(ConfigError {
                kind: ErrorKind::CantInferTextPair { ch },
                span,
            })?;
            let default_mods = match side {
                PairSide::Unshifted => Mods::EMPTY,
                PairSide::Shifted => Mods::SHIFT,
            };
            Ok(LoweredCharPair { pair, default_mods })
        }
        _ => Err(ConfigError {
            kind: ErrorKind::CharPairKeyNeedsChars,
            span: expr.span(),
        }),
    }
}

fn literal_char(expr: &Expr) -> Option<char> {
    if let Expr::Literal {
        value: Literal::Char(ch),
        ..
    } = *expr
    {
        Some(ch)
    } else {
        None
    }
}

fn infer_us_pair_from_unshifted(ch: char) -> Option<CharPair> {
    let shifted = match ch {
        'a'..='z' => ch.to_ascii_uppercase(),

        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',

        '`' => '~',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',

        ' ' => ' ',

        _ => return None,
    };

    Some(CharPair {
        unshifted: ch,
        shifted,
    })
}

fn infer_us_pair_from_shifted(ch: char) -> Option<CharPair> {
    let unshifted = match ch {
        'A'..='Z' => ch.to_ascii_lowercase(),

        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',

        '~' => '`',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',

        ' ' => ' ',

        _ => return None,
    };

    Some(CharPair {
        unshifted,
        shifted: ch,
    })
}

fn lower_mods(args: Vec<Expr>, span: Span) -> Result<Mods, ConfigError> {
    match args.len() {
        0 => Ok(Mods::EMPTY),
        1 => lower_mod_alts(args.into_iter().next().unwrap()),
        _ => Err(ConfigError {
            kind: ErrorKind::TooManyModPatternArgs,
            span,
        }),
    }
}

fn lower_mod_alts(expr: Expr) -> Result<Mods, ConfigError> {
    fn lower_mod_mask(expr: Expr) -> Result<Mods, ConfigError> {
        match expr {
            Expr::Infix {
                op: InfixOp::BitAnd,
                lhs,
                rhs,
                ..
            } => {
                let lhs = lower_mod_mask(*lhs)?;
                let rhs = lower_mod_mask(*rhs)?;
                Ok(lhs | rhs)
            }
            Expr::Infix {
                op: InfixOp::Or,
                span,
                ..
            } => Err(ConfigError {
                kind: ErrorKind::NeedNonPatModSet,
                span,
            }),
            Expr::Ident { name, span } => lower_mod_name(&name, span, false),
            _ => Err(ConfigError {
                kind: ErrorKind::BadMods,
                span: expr.span(),
            }),
        }
    }
    let expr = unparen(expr);
    match expr {
        Expr::Infix {
            op: InfixOp::Or,
            span,
            ..
        } => Err(ConfigError {
            kind: ErrorKind::NeedNonPatModSet,
            span,
        }),
        other => Ok(lower_mod_mask(other)?),
    }
}

fn lower_mods_pattern(
    args: Vec<Expr>,
    span: Span,
    dfl: Mods,
) -> Result<(ModsPattern, Option<ProtocolNeed>), ConfigError> {
    match args.len() {
        0 => Ok((
            ModsPattern::AnyOf(vec![dfl]),
            ProtocolNeed::of(dfl.required_protocol(), span),
        )),
        1 => {
            let expr = unparen(args.into_iter().next().unwrap());
            if let Expr::Ident { name, .. } = &expr
                && name == "any"
            {
                // `any` needs no particular protocol, and intentionally means "any effectively
                // catchable modifier under the current protocol"
                return Ok((ModsPattern::Any, None));
            }
            let mut alts = lower_mod_pat_alts(expr)?;
            dedup_mod_alts(&mut alts);
            let need = alts
                .iter()
                .fold(None, |need, alt| ProtocolNeed::max(need, alt.need));
            Ok((
                ModsPattern::AnyOf(alts.iter().map(|alt| alt.mods).collect()),
                need,
            ))
        }
        _ => Err(ConfigError {
            kind: ErrorKind::TooManyModPatternArgs,
            span,
        }),
    }
}

fn lower_mod_pat_alts(expr: Expr) -> Result<Vec<ModsAlt>, ConfigError> {
    let expr = unparen(expr);
    match expr {
        Expr::Infix {
            op: InfixOp::BitAnd,
            lhs,
            rhs,
            ..
        } => {
            let lhs = lower_mod_pat_alts(*lhs)?;
            let rhs = lower_mod_pat_alts(*rhs)?;
            let mut out = Vec::new();
            for l in &lhs {
                for r in &rhs {
                    out.push(ModsAlt {
                        mods: l.mods | r.mods,
                        need: ProtocolNeed::max(l.need, r.need),
                    });
                }
            }
            Ok(out)
        }
        Expr::Infix {
            op: InfixOp::Or,
            lhs,
            rhs,
            ..
        } => {
            let mut out = lower_mod_pat_alts(*lhs)?;
            out.extend(lower_mod_pat_alts(*rhs)?);
            Ok(out)
        }
        Expr::Ident { name, span } => {
            let mods = lower_mod_name(&name, span, true)?;
            Ok(vec![ModsAlt {
                mods,
                need: ProtocolNeed::of(mods.required_protocol(), span),
            }])
        }
        _ => Err(ConfigError {
            kind: ErrorKind::BadMods,
            span: expr.span(),
        }),
    }
}

fn lower_mod_name(name: &str, span: Span, pattern: bool) -> Result<Mods, ConfigError> {
    match name {
        "none" => Ok(Mods::EMPTY),
        "shift" => Ok(Mods::SHIFT),
        "alt" => Ok(Mods::ALT),
        "ctrl" => Ok(Mods::CTRL),
        "super" => Ok(Mods::SUPER),
        "hyper" => Ok(Mods::HYPER),
        "meta" => Ok(Mods::META),
        "any" => {
            if pattern {
                Err(ConfigError {
                    kind: ErrorKind::AnyModsPatMustBeAlone,
                    span,
                })
            } else {
                Err(ConfigError {
                    kind: ErrorKind::NeedNonPatModSet,
                    span,
                })
            }
        }
        _ => Err(ConfigError {
            kind: ErrorKind::UnknownModifier {
                name: name.to_owned(),
            },
            span,
        }),
    }
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
        "SIGKILL" | "SIGSTOP" => Err(ConfigError {
            kind: ErrorKind::UnsupportedSignal {
                name: name.to_owned(),
                reason: "uncatchable",
            },
            span,
        }),
        "SIGILL" | "SIGABRT" | "SIGFPE" | "SIGSEGV" | "SIGBUS" | "SIGTRAP" | "SIGSYS" => {
            Err(ConfigError {
                kind: ErrorKind::UnsupportedSignal {
                    name: name.to_owned(),
                    reason: "unsupported; error signals are not supported as events",
                },
                span,
            })
        }
        _ => Err(ConfigError {
            kind: ErrorKind::UnknownSignal {
                name: name.to_owned(),
            },
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
            if let Some(n) = parse_numbered_name(name, "f")
                && (1..=35).contains(&n)
            {
                Ok(Key::Function(n))
            } else if let Some(n) = parse_numbered_name(name, "kp_")
                && n <= 9
            {
                Ok(Key::Keypad(KeypadKey::Digit(n)))
            } else {
                Err(ConfigError {
                    kind: ErrorKind::UnknownKey {
                        name: name.to_owned(),
                    },
                    span,
                })
            }
        }
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
        let Expr::Literal {
            value: Literal::String(s),
            ..
        } = arg
        else {
            return Err(ConfigError {
                kind: ErrorKind::BadCommandArgs { kind: "exec" },
                span,
            });
        };
        argv.push(s);
    }

    if argv.is_empty() || argv[0].is_empty() {
        Err(ConfigError {
            kind: ErrorKind::EmptyCommand,
            span,
        })
    } else {
        Ok(CommandSpec::Exec { argv })
    }
}

fn lower_shell_command(args: Vec<Expr>, span: Span) -> Result<CommandSpec, ConfigError> {
    let command = expect_one_string(args).map_err(|()| ConfigError {
        kind: ErrorKind::BadCommandArgs { kind: "sh" },
        span,
    })?;

    if command.is_empty() {
        Err(ConfigError {
            kind: ErrorKind::EmptyCommand,
            span,
        })
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

fn expect_ident(expr: &Expr) -> Result<(&str, Span), ()> {
    match expr {
        Expr::Ident { name, span } => Ok((name.as_str(), *span)),
        _ => Err(()),
    }
}

fn expect_one_string(args: Vec<Expr>) -> Result<String, ()> {
    let mut args = args.into_iter();

    let Some(Expr::Literal {
        value: Literal::String(value),
        ..
    }) = args.next()
    else {
        return Err(());
    };

    if args.next().is_some() {
        return Err(());
    }

    Ok(value)
}

fn unparen(mut expr: Expr) -> Expr {
    while let Expr::Paren { inner, .. } = expr {
        expr = *inner;
    }
    expr
}

fn dedup_mod_alts(values: &mut Vec<ModsAlt>) {
    let mut out: Vec<ModsAlt> = Vec::new();
    for value in values.drain(..) {
        if !out.iter().any(|kept| kept.mods == value.mods) {
            out.push(value);
        }
    }
    *values = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::ast::{FileId, LineCtx};

    fn no_attrs() -> MappingAttrs {
        MappingAttrs {
            passthrough: false,
            always: false,
        }
    }

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

    fn infer_unshifted(c: char) -> Expr {
        Expr::InferPair {
            known: Box::new(ch(c)),
            side: PairSide::Unshifted,
            span: sp(),
        }
    }

    fn infer_shifted(c: char) -> Expr {
        Expr::InferPair {
            known: Box::new(ch(c)),
            side: PairSide::Shifted,
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

    fn protocol_want(name: &str) -> Stmt {
        directive("protocol", vec![ident("want"), ident(name)])
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

    fn bitand(lhs: Expr, rhs: Expr) -> Expr {
        Expr::Infix {
            op: InfixOp::BitAnd,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            span: sp(),
        }
    }

    fn or(lhs: Expr, rhs: Expr) -> Expr {
        Expr::Infix {
            op: InfixOp::Or,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            span: sp(),
        }
    }

    fn source_mods(source: &Source) -> &ModsPattern {
        match source {
            Source::Token(TokenPattern::Key { mods, .. }) => mods,
            other => panic!("expected key token source, got {other:?}"),
        }
    }

    fn finish(stmts: Vec<Stmt>) -> Result<Config, ConfigError> {
        let mut b = ConfigBuilder::default();
        for stmt in stmts {
            b.apply_stmt(stmt)?;
        }
        Ok(b.finish())
    }

    fn err(stmts: Vec<Stmt>) -> ErrorKind {
        finish(stmts).unwrap_err().kind
    }

    fn group_id(config: &Config, name: &str) -> GroupId {
        config.groups.lookup(name).unwrap()
    }

    #[test]
    fn unknown_directive_is_rejected() {
        let e = err(vec![directive("bogus", vec![])]);
        assert!(matches!(e, ErrorKind::UnknownDirective { name } if name == "bogus"));
    }

    #[test]
    fn defines_groups_and_rejects_duplicates() {
        let config = finish(vec![
            define("group", vec![string("reload")]),
            define("group", vec![string("other")]),
        ])
        .unwrap();
        assert!(config.groups.lookup("reload").is_some());
        assert!(config.groups.lookup("other").is_some());

        let e = err(vec![
            define("group", vec![string("reload")]),
            define("group", vec![string("reload")]),
        ]);
        assert!(matches!(e, ErrorKind::DuplicateGroup { name } if name == "reload"));
    }

    #[test]
    fn rejects_bad_group_definition_args() {
        for args in [
            vec![],
            vec![ident("reload")],
            vec![string("a"), string("b")],
        ] {
            let e = err(vec![define("group", args)]);
            assert!(matches!(e, ErrorKind::BadDefinitionArgs { kind: "group" }));
        }
    }

    #[test]
    fn rejects_unknown_definition_kind() {
        let e = err(vec![define("abab", vec![string("x")])]);
        assert!(matches!(e, ErrorKind::UnknownDefinition { kind } if kind == "abab"));
    }

    #[test]
    fn service_exec_is_stored() {
        let config = finish(vec![directive(
            "service",
            vec![
                string("helper"),
                call("exec", vec![string("somehelper"), string("--flag")]),
            ],
        )])
        .unwrap();
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
    fn service_sh_is_stored() {
        let config = finish(vec![directive(
            "service",
            vec![string("helper"), call("sh", vec![string("echo hi")])],
        )])
        .unwrap();
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
            vec![
                string("helper"),
                call("exec", vec![string("x")]),
                string("extra"),
            ],
            vec![string(""), call("exec", vec![string("x")])],
        ] {
            let e = err(vec![directive("service", args)]);
            eprintln!("{e}");
            assert!(matches!(e, ErrorKind::BadDirectiveArgs { kind: "service" }));
        }
    }

    #[test]
    fn service_names_must_be_unique() {
        let e = err(vec![
            directive(
                "service",
                vec![string("helper"), call("exec", vec![string("a")])],
            ),
            directive(
                "service",
                vec![string("helper"), call("exec", vec![string("b")])],
            ),
        ]);
        assert!(matches!(
            e,
            ErrorKind::DuplicateDirective {
                kind: "service",
                ..
            }
        ));
    }

    #[test]
    fn command_exec_requires_string_args_and_nonempty_argv0() {
        for command in [call("exec", vec![]), call("exec", vec![string("")])] {
            let e = err(vec![directive("service", vec![string("helper"), command])]);
            assert!(matches!(e, ErrorKind::EmptyCommand));
        }

        let e = err(vec![directive(
            "service",
            vec![string("helper"), call("exec", vec![ident("prog")])],
        )]);
        assert!(matches!(e, ErrorKind::BadCommandArgs { kind: "exec" }));
    }

    #[test]
    fn command_sh_requires_one_nonempty_string() {
        for command in [
            call("sh", vec![]),
            call("sh", vec![string("a"), string("b")]),
            call("sh", vec![ident("echo")]),
        ] {
            let e = err(vec![directive("service", vec![string("helper"), command])]);
            assert!(matches!(e, ErrorKind::BadCommandArgs { kind: "sh" }));
        }

        let e = err(vec![directive(
            "service",
            vec![string("helper"), call("sh", vec![string("")])],
        )]);
        assert!(matches!(e, ErrorKind::EmptyCommand));
    }

    #[test]
    fn unknown_command_kind_is_rejected() {
        let e = err(vec![directive(
            "service",
            vec![string("name"), call("foo", vec![string("x")])],
        )]);
        assert!(matches!(e, ErrorKind::UnknownCommandKind { kind } if kind == "foo"));
    }

    #[test]
    fn protocol_directive_sets_the_request() {
        let config = finish(vec![protocol_want("kitty")]).unwrap();

        assert_eq!(
            config.protocol,
            ProtocolRequest {
                verb: ProtocolVerb::Want,
                protocol: Protocol::Kitty,
            },
        );
    }

    #[test]
    fn default_protocol_request_is_want_legacy() {
        let config = finish(vec![]).unwrap();

        assert_eq!(
            config.protocol,
            ProtocolRequest {
                verb: ProtocolVerb::Want,
                protocol: Protocol::Legacy,
            },
        );
    }

    #[test]
    fn highest_rank_protocol_request_wins() {
        for order in [["kitty", "legacy"], ["legacy", "kitty"]] {
            let config = finish(order.map(protocol_want).to_vec()).unwrap();

            assert_eq!(
                config.protocol.protocol,
                Protocol::Kitty,
                "order: {order:?}"
            );
        }
    }

    #[test]
    fn protocol_after_any_mappings_is_rejected() {
        let e = err(vec![
            map(
                call("key", vec![ident("f1")]),
                call("send_key", vec![ch('x')]),
            ),
            protocol_want("kitty"),
        ]);

        assert!(matches!(e, ErrorKind::ProtocolAfterMappings));
    }

    #[test]
    fn require_is_reserved_but_unsupported() {
        let e = err(vec![directive(
            "protocol",
            vec![ident("require"), ident("kitty")],
        )]);

        assert!(matches!(
            e,
            ErrorKind::UnsupportedProtocolVerb { verb } if verb == "require"
        ));
    }

    #[test]
    fn unknown_protocol_verbs_and_names_are_rejected() {
        let e = err(vec![directive(
            "protocol",
            vec![ident("demand"), ident("kitty")],
        )]);
        assert!(matches!(e, ErrorKind::UnknownProtocolVerb { verb } if verb == "demand"));

        let e = err(vec![protocol_want("modifyOtherKeys")]);
        assert!(matches!(e, ErrorKind::UnknownProtocol { name } if name == "modifyOtherKeys"));
    }

    #[test]
    fn bad_protocol_directive_args_are_rejected() {
        let e = err(vec![directive("protocol", vec![ident("want")])]);
        assert!(matches!(
            e,
            ErrorKind::BadDirectiveArgs { kind } if kind == "protocol"
        ));

        let e = err(vec![directive(
            "protocol",
            vec![ident("want"), string("kitty")],
        )]);
        assert!(matches!(
            e,
            ErrorKind::BadDirectiveArgs { kind } if kind == "protocol"
        ));
    }

    #[test]
    fn sources_needing_a_higher_rank_protocol_are_rejected() {
        let e = err(vec![map(
            call("key", vec![infer_unshifted('r'), ident("super")]),
            call("exec", vec![string("true")]),
        )]);
        assert!(matches!(
            e,
            ErrorKind::SourceNeedsProtocol {
                needs: "kitty",
                have: "legacy"
            }
        ));

        let e = err(vec![map(
            call("key", vec![ident("left_super")]),
            call("exec", vec![string("true")]),
        )]);
        assert!(matches!(
            e,
            ErrorKind::SourceNeedsProtocol {
                needs: "kitty",
                have: "legacy"
            }
        ));
    }

    #[test]
    fn sources_needing_a_higher_rank_protocol_are_allowed_if_protocol_requested() {
        finish(vec![
            protocol_want("kitty"),
            map(
                call("key", vec![infer_unshifted('r'), ident("super")]),
                call("exec", vec![string("true")]),
            ),
            map(
                call("key", vec![ident("left_super")]),
                call("exec", vec![string("true")]),
            ),
        ])
        .unwrap();
    }

    #[test]
    fn every_alternative_must_be_reachable_under_the_protocol() {
        let e = err(vec![map(
            call(
                "key",
                vec![infer_unshifted('r'), or(ident("shift"), ident("super"))],
            ),
            call("exec", vec![string("true")]),
        )]);

        assert!(matches!(
            e,
            ErrorKind::SourceNeedsProtocol {
                needs: "kitty",
                have: "legacy"
            }
        ));
    }

    #[test]
    fn the_any_modifier_pattern_has_no_protocol_requirement() {
        finish(vec![map(
            call("key", vec![infer_unshifted('r'), ident("any")]),
            call("exec", vec![string("true")]),
        )])
        .unwrap();
    }

    #[test]
    fn targets_are_not_gated_by_protocol() {
        // sending a key the child's protocol cannot express is lossy at encode time, not an error
        finish(vec![map(
            call("key", vec![ident("f1")]),
            call("send_key", vec![ident("f13")]),
        )])
        .unwrap();
    }

    #[test]
    fn key_accepts_known_key_names_and_function_keys() {
        let config = finish(vec![
            // f35 is only reportable under kitty
            protocol_want("kitty"),
            map(
                call("key", vec![ident("esc")]),
                call("send_key", vec![ch('a')]),
            ),
            map(
                call("key", vec![ident("enter")]),
                call("send_key", vec![ch('b')]),
            ),
            map(
                call("key", vec![ident("left")]),
                call("send_key", vec![ch('c')]),
            ),
            map(
                call("key", vec![ident("f35")]),
                call("send_key", vec![ch('d')]),
            ),
            map(
                call("key", vec![ident("kp_9")]),
                call("send_key", vec![ch('e')]),
            ),
        ])
        .unwrap();
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
            assert!(matches!(e, ErrorKind::UnknownKey { name: got } if got == name));
        }
    }

    #[test]
    fn key_accepts_char_pair() {
        let config = finish(vec![map(
            call("key", vec![pair('d', 'D'), ident("shift")]),
            call("inherit_key", vec![pair('w', 'W')]),
        )])
        .unwrap();
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
        ])
        .unwrap();
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
        assert!(matches!(e, ErrorKind::PairUnsupported));
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
        ])
        .unwrap();
        assert_eq!(
            config.mappings[0].to,
            Target::InheritToken(InheritToken::Key {
                key: KeyPattern::Named(Key::Enter),
            }),
        );
        assert_eq!(
            config.mappings[1].to,
            Target::InheritToken(InheritToken::Key {
                key: KeyPattern::CharPair(CharPair {
                    unshifted: 'x',
                    shifted: 'X',
                }),
            }),
        );
    }

    #[test]
    fn inherit_key_rejects_unpaired_char() {
        let e = err(vec![map(
            call("key", vec![ident("f1")]),
            call("inherit_key", vec![ch('🙂')]),
        )]);
        assert!(matches!(
            e,
            ErrorKind::BadEntityArgs {
                kind: "inherit_key"
            }
        ));
    }

    #[test]
    fn char_pair_key_must_contain_chars() {
        let e = err(vec![map(
            call("key", vec![bad_pair()]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(e, ErrorKind::CharPairKeyNeedsChars));
    }

    #[test]
    fn infers_shifted_from_unshifted() {
        let config = finish(vec![map(
            call("key", vec![infer_unshifted('a')]),
            call("send_key", vec![ch('x')]),
        )])
        .unwrap();
        assert_eq!(
            config.mappings[0].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::CharPair(CharPair {
                    unshifted: 'a',
                    shifted: 'A',
                }),
                mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
            }),
        );
    }

    #[test]
    fn infers_unshifted_from_shifted_and_defaults_to_shift() {
        let config = finish(vec![map(
            call("key", vec![infer_shifted('A')]),
            call("send_key", vec![ch('x')]),
        )])
        .unwrap();
        assert_eq!(
            config.mappings[0].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::CharPair(CharPair {
                    unshifted: 'a',
                    shifted: 'A',
                }),
                mods: ModsPattern::AnyOf(vec![Mods::SHIFT]),
            }),
        );
    }

    #[test]
    fn infers_us_punctuation_pairs() {
        let config = finish(vec![
            map(
                call("key", vec![infer_unshifted('1')]),
                call("send_key", vec![ch('x')]),
            ),
            map(
                call("key", vec![infer_shifted('!')]),
                call("send_key", vec![ch('y')]),
            ),
        ])
        .unwrap();
        assert_eq!(
            config.mappings[0].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::CharPair(CharPair {
                    unshifted: '1',
                    shifted: '!',
                }),
                mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
            }),
        );

        assert_eq!(
            config.mappings[1].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::CharPair(CharPair {
                    unshifted: '1',
                    shifted: '!',
                }),
                mods: ModsPattern::AnyOf(vec![Mods::SHIFT]),
            }),
        );
    }

    #[test]
    fn infers_space_as_same_char_pair() {
        let config = finish(vec![map(
            call("key", vec![infer_unshifted(' ')]),
            call("send_key", vec![ch('x')]),
        )])
        .unwrap();

        assert_eq!(
            config.mappings[0].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::CharPair(CharPair {
                    unshifted: ' ',
                    shifted: ' ',
                }),
                mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
            }),
        );
    }

    #[test]
    fn inherit_key_accepts_infer_pair() {
        let config = finish(vec![map(
            call("key", vec![infer_unshifted('a')]),
            call("inherit_key", vec![infer_unshifted('w')]),
        )])
        .unwrap();
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
    fn send_key_rejects_infer_pair() {
        let e = err(vec![map(
            call("key", vec![infer_unshifted('a')]),
            call("send_key", vec![infer_unshifted('x')]),
        )]);
        assert!(matches!(e, ErrorKind::PairUnsupported));
    }

    #[test]
    fn cant_infer_non_us_pair() {
        let e = err(vec![map(
            call("key", vec![infer_unshifted('é')]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(e, ErrorKind::CantInferTextPair { ch: 'é' }));
    }

    #[test]
    fn key_mod_pat_bitand_lowers_to_one_mask() {
        let config = finish(vec![map(
            call(
                "key",
                vec![ident("f1"), bitand(ident("shift"), ident("ctrl"))],
            ),
            call("send_key", vec![ch('x')]),
        )])
        .unwrap();
        assert_eq!(
            source_mods(&config.mappings[0].from),
            &ModsPattern::AnyOf(vec![Mods::SHIFT | Mods::CTRL]),
        );
    }

    #[test]
    fn key_mod_pat_or_lowers_to_alts() {
        let config = finish(vec![map(
            call("key", vec![ident("f1"), or(ident("none"), ident("shift"))]),
            call("send_key", vec![ch('x')]),
        )])
        .unwrap();
        assert_eq!(
            source_mods(&config.mappings[0].from),
            &ModsPattern::AnyOf(vec![Mods::EMPTY, Mods::SHIFT,]),
        );
    }

    #[test]
    fn key_mod_pat_bitand_binds_inside_or() {
        let config = finish(vec![
            protocol_want("kitty"),
            map(
                call(
                    "key",
                    vec![
                        ident("f1"),
                        or(bitand(ident("shift"), ident("ctrl")), ident("super")),
                    ],
                ),
                call("send_key", vec![ch('x')]),
            ),
        ])
        .unwrap();
        assert_eq!(
            source_mods(&config.mappings[0].from),
            &ModsPattern::AnyOf(vec![Mods::SHIFT | Mods::CTRL, Mods::SUPER,]),
        );
    }

    #[test]
    fn key_mod_pat_can_distribute_bitand_over_or() {
        let config = finish(vec![
            protocol_want("kitty"),
            map(
                call(
                    "key",
                    vec![
                        ident("f1"),
                        bitand(ident("ctrl"), or(ident("shift"), ident("super"))),
                    ],
                ),
                call("send_key", vec![ch('x')]),
            ),
        ])
        .unwrap();
        assert_eq!(
            source_mods(&config.mappings[0].from),
            &ModsPattern::AnyOf(vec![Mods::CTRL | Mods::SHIFT, Mods::CTRL | Mods::SUPER,]),
        );
    }

    #[test]
    fn key_mod_pat_rejects_old_varargs_form() {
        let e = finish(vec![map(
            call("key", vec![ident("f1"), ident("shift"), ident("ctrl")]),
            call("send_key", vec![ch('x')]),
        )])
        .unwrap_err();
        assert!(matches!(
            e,
            ConfigError {
                kind: ErrorKind::TooManyModPatternArgs,
                ..
            }
        ));
    }

    #[test]
    fn key_mod_pat_rejects_any_inside_or() {
        let e = finish(vec![map(
            call("key", vec![ident("f1"), or(ident("any"), ident("shift"))]),
            call("send_key", vec![ch('x')]),
        )])
        .unwrap_err();
        assert!(matches!(
            e,
            ConfigError {
                kind: ErrorKind::AnyModsPatMustBeAlone,
                ..
            }
        ));
    }

    #[test]
    fn key_mod_pat_rejects_any_inside_bitand() {
        let e = finish(vec![map(
            call(
                "key",
                vec![ident("f1"), bitand(ident("any"), ident("shift"))],
            ),
            call("send_key", vec![ch('x')]),
        )])
        .unwrap_err();
        assert!(matches!(
            e,
            ConfigError {
                kind: ErrorKind::AnyModsPatMustBeAlone,
                ..
            }
        ));
    }

    #[test]
    fn unknown_or_non_ident_mod_is_rejected() {
        let e = err(vec![map(
            call("key", vec![ident("f1"), ident("bogus")]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(e, ErrorKind::UnknownModifier { name } if name == "bogus"));

        let e = err(vec![map(
            call("key", vec![ident("f1"), string("ctrl")]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(e, ErrorKind::BadMods));
    }

    #[test]
    fn send_key_rejects_mod_pat() {
        let e = finish(vec![map(
            call("key", vec![ident("f1"), ident("any")]),
            call(
                "send_key",
                vec![ch('x'), or(ident("shift"), ident("super"))],
            ),
        )])
        .unwrap_err();
        assert!(matches!(
            e,
            ConfigError {
                kind: ErrorKind::NeedNonPatModSet,
                ..
            }
        ));
    }

    #[test]
    fn left_mapping_reverses_sides() {
        let config = finish(vec![map_left(
            call("send_key", vec![ch('x')]),
            call("key", vec![ident("f1")]),
        )])
        .unwrap();
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
        ])
        .unwrap();
        let reload = group_id(&config, "reload");
        assert_eq!(
            config.mappings[0],
            Mapping {
                from: Source::Token(TokenPattern::Key {
                    key: KeyPattern::Named(Key::Function(5)),
                    mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
                }),
                to: Target::Group(reload),
                attrs: no_attrs(),
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
        assert!(matches!(e, ErrorKind::UnknownGroup { name } if name == "xyz"));
    }

    #[test]
    fn group_self_map_is_rejected() {
        let e = err(vec![
            define("group", vec![string("g")]),
            map(
                call("group", vec![string("g")]),
                call("group", vec![string("g")]),
            ),
        ]);
        assert!(matches!(e, ErrorKind::GroupSelfMap));
    }

    #[test]
    fn target_only_token_cannot_be_source() {
        let e = err(vec![map(
            call("send_key", vec![ch('x')]),
            call("send_key", vec![ch('y')]),
        )]);
        assert!(matches!(e, ErrorKind::SendTokenAsSource));
    }

    #[test]
    fn source_only_token_cannot_be_target() {
        let e = err(vec![map(
            call("key", vec![ident("f1")]),
            call("key", vec![ident("f2")]),
        )]);
        assert!(matches!(e, ErrorKind::SourceTokenAsTarget));
    }

    #[test]
    fn action_cannot_be_source() {
        let e = err(vec![map(
            call("sh", vec![string("echo hi")]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(e, ErrorKind::ActionAsSource));
    }

    #[test]
    fn event_cannot_be_target() {
        let e = err(vec![map(
            call("key", vec![ident("f1")]),
            call("sockdata_utf8", vec![string("reload")]),
        )]);
        assert!(matches!(e, ErrorKind::EventAsTarget));
    }

    #[test]
    fn inherit_token_cannot_be_source() {
        let e = err(vec![map(
            call("inherit_key", vec![pair('x', 'X')]),
            call("send_key", vec![ch('y')]),
        )]);
        assert!(matches!(e, ErrorKind::InheritTokenAsSource));
    }

    #[test]
    fn inherit_token_requires_token_payload() {
        let e = err(vec![
            define("group", vec![string("g")]),
            map(
                call("group", vec![string("g")]),
                call("inherit_key", vec![pair('x', 'X')]),
            ),
        ]);
        assert!(matches!(
            e,
            ErrorKind::TargetRequiresPayload {
                required: PayloadKind::Token
            }
        ));
    }

    #[test]
    fn normal_group_does_not_propagate_token_payload() {
        let e = err(vec![
            define("group", vec![string("g")]),
            map(
                call("key", vec![pair('d', 'D')]),
                call("group", vec![string("g")]),
            ),
            map(
                call("group", vec![string("g")]),
                call("inherit_key", vec![pair('w', 'W')]),
            ),
        ]);
        assert!(matches!(
            e,
            ErrorKind::TargetRequiresPayload {
                required: PayloadKind::Token
            }
        ));
    }

    #[test]
    fn sockdata_utf8_source_stores_utf8_bytes() {
        let config = finish(vec![map(
            call("sockdata_utf8", vec![string("å")]),
            call("sh", vec![string("reload")]),
        )])
        .unwrap();
        assert_eq!(
            config.mappings[0].from,
            Source::Event(Event::Sockdata("å".as_bytes().to_vec()))
        );
    }

    #[test]
    fn signal_source_lowers_supported_signals() {
        let config = finish(vec![map(
            call("signal", vec![string("SIGWINCH")]),
            call("sh", vec![string("resize")]),
        )])
        .unwrap();
        assert_eq!(
            config.mappings[0].from,
            Source::Event(Event::Signal(Signal(libc::SIGWINCH)))
        );
    }

    #[test]
    fn signal_source_rejects_unknown_and_unsupported_signals() {
        let e = err(vec![map(
            call("signal", vec![string("SIGXYZ")]),
            call("sh", vec![string("x")]),
        )]);
        assert!(matches!(e, ErrorKind::UnknownSignal { name } if name == "SIGXYZ"));

        let e = err(vec![map(
            call("signal", vec![string("SIGKILL")]),
            call("sh", vec![string("x")]),
        )]);
        assert!(
            matches!(e, ErrorKind::UnsupportedSignal { name, reason: "uncatchable"} if name == "SIGKILL")
        );

        let e = err(vec![map(
            call("signal", vec![string("SIGSEGV")]),
            call("sh", vec![string("x")]),
        )]);
        assert!(matches!(e, ErrorKind::UnsupportedSignal { name, .. } if name == "SIGSEGV"));
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
        ])
        .unwrap();
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
        assert!(matches!(e,ErrorKind::UnknownEntity { name } if name == "bogus"));
    }

    #[test]
    fn unknown_mapping_attr_is_rejected() {
        let e = err(vec![map_with_attrs(
            vec![attr("xyz", vec![])],
            call("key", vec![ident("f1")]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(e, ErrorKind::UnsupportedMappingAttr { name } if name == "xyz"));
    }

    #[test]
    fn passthrough_attr_is_stored_on_token_source_mapping() {
        let config = finish(vec![map_with_attrs(
            vec![attr("passthrough", vec![])],
            call("key", vec![ident("f1")]),
            call("send_key", vec![ch('x')]),
        )])
        .unwrap();
        assert_eq!(config.mappings.len(), 1);
        assert!(config.mappings[0].attrs.passthrough);
    }

    #[test]
    fn passthrough_requires_token_source() {
        let e = err(vec![
            define("group", vec![string("g")]),
            map_with_attrs(
                vec![attr("passthrough", vec![])],
                call("group", vec![string("g")]),
                call("send_key", vec![ch('x')]),
            ),
        ]);
        assert!(matches!(e, ErrorKind::InvalidPassthroughSource));

        let e = err(vec![map_with_attrs(
            vec![attr("passthrough", vec![])],
            call("sockdata_utf8", vec![string("reload")]),
            call("sh", vec![string("reload")]),
        )]);
        assert!(matches!(e, ErrorKind::InvalidPassthroughSource));
    }

    #[test]
    fn passthrough_rejects_args_and_duplicates() {
        let e = err(vec![map_with_attrs(
            vec![attr("passthrough", vec![int(1)])],
            call("key", vec![ident("f1")]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(
            e,
            ErrorKind::BadMappingAttrArgs {
                kind: "passthrough"
            }
        ));

        let e = err(vec![map_with_attrs(
            vec![attr("passthrough", vec![]), attr("passthrough", vec![])],
            call("key", vec![ident("f1")]),
            call("send_key", vec![ch('x')]),
        )]);
        assert!(matches!(
            e,
            ErrorKind::DuplicateMappingAttr {
                kind: "passthrough"
            }
        ));
    }
}
