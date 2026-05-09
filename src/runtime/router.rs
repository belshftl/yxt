// SPDX-License-Identifier: MIT

use crate::model::*;

#[derive(Debug, Clone, Copy)]
pub enum RouteInput<'a> {
    Event(&'a Event),
    Token(&'a Token),
    Group(GroupId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteEffect {
    Token(Token),
    Action(Action),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteResult {
    pub matched: bool,
    pub effects: Vec<RouteEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouteError {
    #[error("group cycle detected (TODO: format message)")]
    GroupCycle {
        stack: Vec<GroupId>,
        repeated: GroupId,
    },

    #[error("target requires payload of type {required:?} that the mapped source did not provide")]
    MissingPayload {
        required: PayloadKind,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteEntry {
    from: Source,
    to: Target,
    attrs: MappingAttrs,
}

#[derive(Debug)]
pub struct Router {
    entries: Vec<RouteEntry>,
    group_count: usize,
}

impl Router {
    pub fn new(config: &Config) -> Self {
        Self {
            entries: config.mappings.iter().map(|m| RouteEntry {
                from: m.from.clone(),
                to: m.to.clone(),
                attrs: m.attrs,
            }).collect(),
            group_count: config.groups.len(),
        }
    }

    pub fn fire(&self, input: RouteInput<'_>) -> Result<RouteResult, RouteError> {
        let mut ctx = FireCtx::new(self.group_count);
        self.fire_input(input, &mut ctx)?;
        Ok(RouteResult {
            matched: ctx.matched,
            effects: ctx.effects,
        })
    }

    fn fire_input(&self, input: RouteInput<'_>, ctx: &mut FireCtx) -> Result<(), RouteError> {
        for entry in &self.entries {
            let Some(payload) = match_source(&entry.from, input) else {
                continue;
            };
            ctx.matched = true;
            if entry.attrs.passthrough {
                let RouteInput::Token(t) = input else {
                    panic!("passthrough is only valid for token sources, this should've gotten caught by lower validation");
                };
                ctx.effects.push(RouteEffect::Token(t.clone()));
            }
            self.fire_target(&entry.to, payload, ctx)?;
        }
        Ok(())
    }

    fn fire_target(&self, target: &Target, payload: Option<Payload>, ctx: &mut FireCtx) -> Result<(), RouteError> {
        match target {
            Target::Token(token) => {
                let token = match payload.and_then(Payload::token) {
                    Some(payload) => token.clone().with_kind(payload.kind),
                    None => token.clone(),
                };
                ctx.effects.push(RouteEffect::Token(token));
                Ok(())
            }
            Target::InheritToken(target) => {
                let payload = payload.and_then(Payload::token).ok_or(RouteError::MissingPayload {
                    required: PayloadKind::Token,
                })?;
                ctx.effects.push(RouteEffect::Token(target.to_token(payload)));
                Ok(())
            }
            Target::Group(group) => {
                if fires_non_token_target(payload) {
                    self.fire_group(*group, ctx)?;
                }
                Ok(())
            }
            Target::Action(action) => {
                if fires_non_token_target(payload) {
                    ctx.effects.push(RouteEffect::Action(action.clone()));
                }
                Ok(())
            }
        }
    }

    fn fire_group(&self, group: GroupId, ctx: &mut FireCtx) -> Result<(), RouteError> {
        ctx.push_group(group)?;
        let result = self.fire_input(RouteInput::Group(group), ctx);
        ctx.pop_group(group);
        result
    }
}

fn match_source(pattern: &Source, input: RouteInput<'_>) -> Option<Option<Payload>> {
    match (pattern, input) {
        (Source::Event(want), RouteInput::Event(got)) if want == got => Some(None),
        (Source::Group(want), RouteInput::Group(got)) if *want == got => Some(None),
        (Source::Token(pattern), RouteInput::Token(token)) =>
            match_token_pattern(pattern, token).map(|p| Some(Payload::Token(p))),
        _ => None,
    }
}

fn match_token_pattern(pattern: &TokenPattern, token: &Token) -> Option<TokenPayload> {
    match pattern {
        TokenPattern::Key { key, mods } => {
            match_key_pattern(key, mods, token)
        }
    }
}

fn match_key_pattern(pattern: &KeyPattern, mods: &ModsPattern, token: &Token) -> Option<TokenPayload> {
    match (pattern, token) {
        (KeyPattern::Named(want), Token::Key { key: got, mods: actual_mods, kind }) if want == got => {
            let logical_mods = *actual_mods;
            mods.matches(logical_mods).then_some(TokenPayload {
                actual_mods: *actual_mods,
                logical_mods,
                kind: *kind,
            })
        }
        (KeyPattern::CharPair(pair), Token::Utf8 { ch, mods: actual_mods, kind }) => {
            let logical_mods = char_pair_logical_mods(*pair, *ch, *actual_mods)?;
            mods.matches(logical_mods).then_some(TokenPayload {
                actual_mods: *actual_mods,
                logical_mods,
                kind: *kind,
            })
        }
        _ => None,
    }
}

fn char_pair_logical_mods(pair: CharPair, ch: char, actual_mods: Mods) -> Option<Mods> {
    if pair.unshifted == pair.shifted {
        (ch == pair.unshifted).then_some(actual_mods)
    } else if ch == pair.unshifted {
        Some(actual_mods & !Mods::SHIFT)
    } else if ch == pair.shifted {
        Some(actual_mods | Mods::SHIFT)
    } else {
        None
    }
}

fn fires_non_token_target(payload: Option<Payload>) -> bool {
    match payload.and_then(Payload::token).map(|p| p.kind) {
        None => true,
        Some(KeyEventKind::Press | KeyEventKind::Repeat) => true,
        Some(KeyEventKind::Release) => false,
    }
}

struct FireCtx {
    active_groups: Vec<bool>,
    group_stack: Vec<GroupId>,
    matched: bool,
    effects: Vec<RouteEffect>,
}

impl FireCtx {
    fn new(group_count: usize) -> Self {
        Self {
            active_groups: vec![false; group_count],
            group_stack: Vec::new(),
            matched: false,
            effects: Vec::new(),
        }
    }

    fn push_group(&mut self, group: GroupId) -> Result<(), RouteError> {
        let idx = group.0;
        debug_assert!(idx < self.active_groups.len());
        if self.active_groups[idx] {
            return Err(RouteError::GroupCycle {
                stack: self.group_stack.clone(),
                repeated: group,
            });
        }
        self.active_groups[idx] = true;
        self.group_stack.push(group);
        Ok(())
    }

    fn pop_group(&mut self, group: GroupId) {
        let popped = self.group_stack.pop();
        debug_assert_eq!(popped, Some(group));
        self.active_groups[group.0] = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use crate::config::loader::ConfigLoader;
    use crate::model::{Action, CommandSpec, Event, Key, Mods, Token};

    fn cfg(src: &str) -> crate::model::Config {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.conf");
        fs::write(&path, src).unwrap();
        ConfigLoader::new().parse_file(&path).unwrap()
    }

    fn router(src: &str) -> Router {
        Router::new(&cfg(src))
    }

    fn fire_key(router: &Router, key: Key, mods: Mods) -> RouteResult {
        let token = Token::press_key(key, mods);
        router.fire(RouteInput::Token(&token)).unwrap()
    }

    fn fire_utf8(router: &Router, ch: char, mods: Mods) -> RouteResult {
        let token = Token::press_utf8(ch, mods);
        router.fire(RouteInput::Token(&token)).unwrap()
    }

    fn fire_token(router: &Router, token: &Token) -> RouteResult {
        router.fire(RouteInput::Token(token)).unwrap()
    }

    fn fire_sockdata(router: &Router, data: &[u8]) -> RouteResult {
        let event = Event::Sockdata(data.to_vec());
        router.fire(RouteInput::Event(&event)).unwrap()
    }

    #[test]
    fn unmatched_input_produces_no_effects() {
        let router = router(r#"
@version 1
key(f1) => send_key('x')
"#);

        let result = fire_key(&router, Key::Function(2), Mods::EMPTY);

        assert!(!result.matched);
        assert!(result.effects.is_empty());
    }

    #[test]
    fn direct_named_key_to_concrete_utf8_mapping() {
        let router = router(r#"
@version 1
key(f1) => send_key('x')
"#);

        let result = fire_key(&router, Key::Function(1), Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('x', Mods::EMPTY))],
        );
    }

    #[test]
    fn direct_named_key_to_named_key_mapping() {
        let router = router(r#"
@version 1
key(f1) => send_key(enter)
"#);

        let result = fire_key(&router, Key::Function(1), Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_key(Key::Enter, Mods::EMPTY))],
        );
    }

    #[test]
    fn direct_action_mapping() {
        let router = router(r#"
@version 1
key(f1) => sh("echo hi")
"#);

        let result = fire_key(&router, Key::Function(1), Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Action(Action::Command(CommandSpec::Shell {
                command: "echo hi".to_owned(),
            }))],
        );
    }

    #[test]
    fn event_source_mapping() {
        let router = router(r#"
@version 1
sockdata_utf8("reload") => sh("reload")
"#);

        let result = fire_sockdata(&router, b"reload");

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Action(Action::Command(CommandSpec::Shell {
                command: "reload".to_owned(),
            }))],
        );

        assert!(!fire_sockdata(&router, b"reload\n").matched);
    }

    #[test]
    fn source_default_mod_pattern_is_none() {
        let router = router(r#"
@version 1
key(f1) => send_key('x')
"#);

        assert!(fire_key(&router, Key::Function(1), Mods::EMPTY).matched);
        assert!(!fire_key(&router, Key::Function(1), Mods::CTRL).matched);
        assert!(!fire_key(&router, Key::Function(1), Mods::SHIFT).matched);
    }

    #[test]
    fn source_any_mod_pattern_matches_everything() {
        let router = router(r#"
@version 1
key(f1, any) => send_key('x')
"#);

        assert!(fire_key(&router, Key::Function(1), Mods::EMPTY).matched);
        assert!(fire_key(&router, Key::Function(1), Mods::CTRL).matched);
        assert!(fire_key(&router, Key::Function(1), Mods::ALT | Mods::CTRL).matched);
    }

    #[test]
    fn source_explicit_mod_pattern_is_exact() {
        let router = router(r#"
@version 1
key(f1, ctrl) => send_key('x')
"#);

        assert!(!fire_key(&router, Key::Function(1), Mods::EMPTY).matched);
        assert!(!fire_key(&router, Key::Function(1), Mods::ALT).matched);
        assert!(!fire_key(&router, Key::Function(1), Mods::ALT | Mods::CTRL).matched);

        let result = fire_key(&router, Key::Function(1), Mods::CTRL);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('x', Mods::EMPTY))],
        );
    }

    #[test]
    fn target_token_modifiers_are_preserved() {
        let router = router(r#"
@version 1
key(f1) => send_key('x', ctrl & alt)
"#);

        let result = fire_key(&router, Key::Function(1), Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('x', Mods::ALT | Mods::CTRL))],
        );
    }

    #[test]
    fn multiple_mappings_for_same_source_preserve_order() {
        let router = router(r#"
@version 1
key(f1) => send_key('a')
key(f1) => send_key('b')
key(f1) => sh("c")
key(f1) => send_key('d')
"#);

        let result = fire_key(&router, Key::Function(1), Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![
                RouteEffect::Token(Token::press_utf8('a', Mods::EMPTY)),
                RouteEffect::Token(Token::press_utf8('b', Mods::EMPTY)),
                RouteEffect::Action(Action::Command(CommandSpec::Shell {
                    command: "c".to_owned(),
                })),
                RouteEffect::Token(Token::press_utf8('d', Mods::EMPTY)),
            ],
        );
    }

    #[test]
    fn group_expands_to_effects() {
        let router = router(r#"
@version 1
define group "reload"

key(f5) => group("reload")
group("reload") => send_key('r')
group("reload") => sh("reload")
"#);

        let result = fire_key(&router, Key::Function(5), Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![
                RouteEffect::Token(Token::press_utf8('r', Mods::EMPTY)),
                RouteEffect::Action(Action::Command(CommandSpec::Shell {
                    command: "reload".to_owned(),
                })),
            ],
        );
    }

    #[test]
    fn nested_groups_expand_depth_first() {
        let router = router(r#"
@version 1
define group "a"
define group "b"
define group "c"

key(f1) => group("a")
group("a") => send_key('a')
group("a") => group("b")
group("a") => send_key('d')

group("b") => send_key('b')
group("b") => group("c")

group("c") => send_key('c')
"#);

        let result = fire_key(&router, Key::Function(1), Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![
                RouteEffect::Token(Token::press_utf8('a', Mods::EMPTY)),
                RouteEffect::Token(Token::press_utf8('b', Mods::EMPTY)),
                RouteEffect::Token(Token::press_utf8('c', Mods::EMPTY)),
                RouteEffect::Token(Token::press_utf8('d', Mods::EMPTY)),
            ],
        );
    }

    #[test]
    fn produced_tokens_do_not_map() {
        let router = router(r#"
@version 1
key(f1) => send_key('x')
key('x'~'X') => sh("should run only for external x")
"#);

        let result = fire_key(&router, Key::Function(1), Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('x', Mods::EMPTY))],
        );

        let result = fire_utf8(&router, 'x', Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Action(Action::Command(CommandSpec::Shell {
                command: "should run only for external x".to_owned(),
            }))],
        );
    }

    #[test]
    fn char_pair_matches_unshifted_and_shifted_sides_using_logical_mods() {
        let router = router(r#"
@version 1
key('x'~'X') => send_key('u')
key('x'~'X', shift) => send_key('s')
"#);

        let result = fire_utf8(&router, 'x', Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('u', Mods::EMPTY))],
        );

        let result = fire_utf8(&router, 'X', Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('s', Mods::EMPTY))],
        );
    }

    #[test]
    fn char_pair_shift_side_matches_even_when_shift_mod_is_reported() {
        let router = router(r#"
@version 1
key('x'~'X', shift) => send_key('s')
"#);

        let result = fire_utf8(&router, 'X', Mods::SHIFT);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('s', Mods::EMPTY))],
        );
    }

    #[test]
    fn same_char_pair_uses_actual_mods_for_logical_mods() {
        let router = router(r#"
@version 1
key(' '~' ') => send_key('u')
key(' '~' ', shift) => send_key('s')
"#);

        let result = fire_utf8(&router, ' ', Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('u', Mods::EMPTY))],
        );

        let result = fire_utf8(&router, ' ', Mods::SHIFT);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('s', Mods::EMPTY))],
        );
    }

    #[test]
    fn named_key_missing_mod_pattern_defaults_to_none() {
        let router = router(r#"
@version 1
key(f1) => send_key('x')
"#);

        assert!(fire_key(&router, Key::Function(1), Mods::EMPTY).matched);
        assert!(!fire_key(&router, Key::Function(1), Mods::SHIFT).matched);
        assert!(!fire_key(&router, Key::Function(1), Mods::CTRL).matched);
    }

    #[test]
    fn named_key_mod_pattern_any_matches_all_mod_masks() {
        let router = router(r#"
@version 1
key(f1, any) => send_key('x')
"#);
        for mods in [
            Mods::EMPTY,
            Mods::SHIFT,
            Mods::CTRL,
            Mods::ALT,
            Mods::ALT | Mods::CTRL,
            Mods::SHIFT | Mods::ALT | Mods::CTRL,
            Mods::SUPER | Mods::META,
        ] {
            assert!(fire_key(&router, Key::Function(1), mods).matched, "mods {mods:?} did not match");
        }
    }

    #[test]
    fn named_key_mod_pattern_exact_mask_rejects_subset_and_superset() {
        let router = router(r#"
@version 1
key(f1, alt & ctrl) => send_key('x')
"#);

        assert!(!fire_key(&router, Key::Function(1), Mods::EMPTY).matched);
        assert!(!fire_key(&router, Key::Function(1), Mods::CTRL).matched);
        assert!(!fire_key(&router, Key::Function(1), Mods::ALT).matched);
        assert!(!fire_key(&router, Key::Function(1), Mods::SHIFT | Mods::ALT | Mods::CTRL).matched);

        assert!(fire_key(&router, Key::Function(1), Mods::ALT | Mods::CTRL).matched);
    }

    #[test]
    fn char_pair_default_none_matches_unshifted_logical_side_only() {
        let router = router(r#"
@version 1
key('x'~'X') => send_key('u')
"#);

        assert!(fire_utf8(&router, 'x', Mods::EMPTY).matched);

        // shift on actual mods is cleared when the matched char is the unshifted one
        assert!(fire_utf8(&router, 'x', Mods::SHIFT).matched);

        assert!(!fire_utf8(&router, 'X', Mods::EMPTY).matched);
        assert!(!fire_utf8(&router, 'X', Mods::SHIFT).matched);
        assert!(!fire_utf8(&router, 'x', Mods::CTRL).matched);
    }

    #[test]
    fn char_pair_shift_pattern_matches_shifted_logical_side() {
        let router = router(r#"
@version 1
key('x'~'X', shift) => send_key('s')
"#);

        assert!(!fire_utf8(&router, 'x', Mods::EMPTY).matched);
        assert!(!fire_utf8(&router, 'x', Mods::SHIFT).matched);

        // shifted side implies logical shift mod even when there's no actual shift mod
        assert!(fire_utf8(&router, 'X', Mods::EMPTY).matched);
        assert!(fire_utf8(&router, 'X', Mods::SHIFT).matched);
    }

    #[test]
    fn char_pair_ctrl_pattern_uses_logical_mods() {
        let router = router(r#"
@version 1
key('x'~'X', ctrl) => send_key('c')
"#);

        assert!(fire_utf8(&router, 'x', Mods::CTRL).matched);

        // actual shift mod is cleared for the unshifted side before matching
        assert!(fire_utf8(&router, 'x', Mods::SHIFT | Mods::CTRL).matched);

        assert!(!fire_utf8(&router, 'X', Mods::CTRL).matched);
        assert!(!fire_utf8(&router, 'X', Mods::SHIFT | Mods::CTRL).matched);
        assert!(!fire_utf8(&router, 'x', Mods::ALT).matched);
    }

    #[test]
    fn char_pair_ctrl_shift_pattern_matches_shifted_side_with_ctrl() {
        let router = router(r#"
@version 1
key('x'~'X', ctrl & shift) => send_key('s')
"#);

        assert!(!fire_utf8(&router, 'x', Mods::CTRL).matched);
        assert!(!fire_utf8(&router, 'x', Mods::SHIFT | Mods::CTRL).matched);

        assert!(fire_utf8(&router, 'X', Mods::CTRL).matched);
        assert!(fire_utf8(&router, 'X', Mods::SHIFT | Mods::CTRL).matched);

        assert!(!fire_utf8(&router, 'X', Mods::ALT).matched);
        assert!(!fire_utf8(&router, 'X', Mods::ALT | Mods::CTRL).matched);
    }

    #[test]
    fn same_char_char_pair_does_not_infer_shift_from_character() {
        let router = router(r#"
@version 1
key(' '~' ') => send_key('u')
key(' '~' ', shift) => send_key('s')
"#);

        let result = fire_utf8(&router, ' ', Mods::EMPTY);
        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('u', Mods::EMPTY))],
        );

        let result = fire_utf8(&router, ' ', Mods::SHIFT);
        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('s', Mods::EMPTY))],
        );
    }

    #[test]
    fn same_char_pair_any_matches_with_or_without_reported_shift() {
        let router = router(r#"
@version 1
key(' '~' ', any) => send_key('x')
"#);

        assert!(fire_utf8(&router, ' ', Mods::EMPTY).matched);
        assert!(fire_utf8(&router, ' ', Mods::SHIFT).matched);
        assert!(fire_utf8(&router, ' ', Mods::CTRL).matched);
        assert!(fire_utf8(&router, ' ', Mods::SHIFT | Mods::CTRL).matched);
    }

    #[test]
    fn token_to_token_preserves_press_repeat_release() {
        let router = router(r#"
@version 1
key('x'~'X') => send_key('y')
"#);

        for kind in [KeyEventKind::Press, KeyEventKind::Repeat, KeyEventKind::Release] {
            let token = Token::Utf8 {
                ch: 'x',
                mods: Mods::EMPTY,
                kind,
            };

            let result = fire_token(&router, &token);

            assert!(result.matched);
            assert_eq!(
                result.effects,
                vec![RouteEffect::Token(Token::Utf8 {
                    ch: 'y',
                    mods: Mods::EMPTY,
                    kind,
                })],
            );
        }
    }

    #[test]
    fn token_to_group_fires_on_press_and_repeat_not_release() {
        let router = router(r#"
@version 1
define group "g"

key('x'~'X') => group("g")
group("g") => send_key('y')
"#);

        for kind in [KeyEventKind::Press, KeyEventKind::Repeat] {
            let token = Token::Utf8 {
                ch: 'x',
                mods: Mods::EMPTY,
                kind,
            };

            let result = fire_token(&router, &token);

            assert!(result.matched);
            assert_eq!(
                result.effects,
                vec![RouteEffect::Token(Token::press_utf8('y', Mods::EMPTY))],
            );
        }

        let token = Token::Utf8 {
            ch: 'x',
            mods: Mods::EMPTY,
            kind: KeyEventKind::Release,
        };

        let result = fire_token(&router, &token);

        assert!(result.matched);
        assert!(result.effects.is_empty());
    }

    #[test]
    fn token_to_action_fires_on_press_and_repeat_not_release() {
        let router = router(r#"
@version 1
key('x'~'X') => sh("x")
"#);

        for kind in [KeyEventKind::Press, KeyEventKind::Repeat] {
            let token = Token::Utf8 {
                ch: 'x',
                mods: Mods::EMPTY,
                kind,
            };

            let result = fire_token(&router, &token);

            assert!(result.matched);
            assert_eq!(
                result.effects,
                vec![RouteEffect::Action(Action::Command(CommandSpec::Shell {
                    command: "x".to_owned(),
                }))],
            );
        }

        let token = Token::Utf8 {
            ch: 'x',
            mods: Mods::EMPTY,
            kind: KeyEventKind::Release,
        };

        let result = fire_token(&router, &token);

        assert!(result.matched);
        assert!(result.effects.is_empty());
    }

    #[test]
    fn unmatched_release_is_not_matched() {
        let router = router(r#"
@version 1
key('x'~'X') => send_key('y')
"#);

        let token = Token::Utf8 {
            ch: 'z',
            mods: Mods::EMPTY,
            kind: KeyEventKind::Release,
        };

        let result = fire_token(&router, &token);

        assert!(!result.matched);
        assert!(result.effects.is_empty());
    }

    #[test]
    fn normal_token_target_does_not_copy_modifiers() {
        let router = router(r#"
@version 1
key('x'~'X', ctrl) => send_key('y')
"#);

        let token = Token::Utf8 {
            ch: 'x',
            mods: Mods::CTRL,
            kind: KeyEventKind::Repeat,
        };

        let result = fire_token(&router, &token);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::Utf8 {
                ch: 'y',
                mods: Mods::EMPTY,
                kind: KeyEventKind::Repeat,
            })],
        );
    }

    #[test]
    fn inherit_named_key_copies_actual_modifiers_and_kind() {
        let router = router(r#"
@version 1
key('x'~'X', ctrl & alt) => inherit_key(enter)
"#);

        let token = Token::Utf8 {
            ch: 'x',
            mods: Mods::ALT | Mods::CTRL,
            kind: KeyEventKind::Release,
        };

        let result = fire_token(&router, &token);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::Key {
                key: Key::Enter,
                mods: Mods::ALT | Mods::CTRL,
                kind: KeyEventKind::Release,
            })],
        );
    }

    #[test]
    fn inherit_char_pair_chooses_side_from_logical_shift_and_preserves_actual_mods() {
        let router = router(r#"
@version 1
key('x'~'X', any) => inherit_key('w'~'W')
"#);

        let result = fire_utf8(&router, 'x', Mods::CTRL);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::Utf8 {
                ch: 'w',
                mods: Mods::CTRL,
                kind: KeyEventKind::Press,
            })],
        );

        let result = fire_utf8(&router, 'X', Mods::CTRL);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::Utf8 {
                ch: 'W',
                mods: Mods::CTRL,
                kind: KeyEventKind::Press,
            })],
        );
    }

    #[test]
    fn inherit_same_char_pair_preserves_shift_when_terminal_reported_it() {
        let router = router(r#"
@version 1
key(' '~' ', any) => inherit_key(' '~' ')
"#);

        let result = fire_utf8(&router, ' ', Mods::SHIFT);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::Utf8 {
                ch: ' ',
                mods: Mods::SHIFT,
                kind: KeyEventKind::Press,
            })],
        );
    }

    #[test]
    fn passthrough_preserves_input_then_fires_target() {
        let router = router(r#"
@version 1
passthrough! key(f1, ctrl) => sh("xyz")
"#);

        let input = Token::Key {
            key: Key::Function(1),
            mods: Mods::CTRL,
            kind: KeyEventKind::Press,
        };

        let result = fire_token(&router, &input);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![
                RouteEffect::Token(input.clone()),
                RouteEffect::Action(Action::Command(CommandSpec::Shell {
                    command: "xyz".to_owned(),
                })),
            ],
        );
    }

    #[test]
    fn passthrough_token_mapping_emits_original_token_before_target_effect() {
        let router = router(r#"
@version 1
passthrough! key('x'~'X', any) => send_key('y')
"#);

        let input = Token::Utf8 {
            ch: 'X',
            mods: Mods::CTRL,
            kind: KeyEventKind::Repeat,
        };

        let result = fire_token(&router, &input);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![
                RouteEffect::Token(input.clone()),
                RouteEffect::Token(Token::Utf8 {
                    ch: 'y',
                    mods: Mods::EMPTY,
                    kind: KeyEventKind::Repeat,
                }),
            ],
        );
    }

    #[test]
    fn passthrough_preserves_release_even_when_non_token_target_is_suppressed() {
        let router = router(r#"
@version 1
passthrough! key(f1, ctrl) => sh("should not run on release")
"#);

        let input = Token::Key {
            key: Key::Function(1),
            mods: Mods::CTRL,
            kind: KeyEventKind::Release,
        };

        let result = fire_token(&router, &input);

        assert!(result.matched);
        assert_eq!(result.effects, vec![RouteEffect::Token(input.clone())]);
    }

    #[test]
    fn passthrough_does_not_remap_emitted_token() {
        let router = router(r#"
@version 1
passthrough! key(f1) => send_key('x')
key(f1) => send_key('y')
key('x'~'X') => sh("passthrough/result tokens must not re-enter router")
"#);

        let input = Token::Key {
            key: Key::Function(1),
            mods: Mods::EMPTY,
            kind: KeyEventKind::Press,
        };

        let result = fire_token(&router, &input);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![
                RouteEffect::Token(input),
                RouteEffect::Token(Token::press_utf8('x', Mods::EMPTY)),
                RouteEffect::Token(Token::press_utf8('y', Mods::EMPTY)),
            ],
        );
    }

    #[test]
    fn same_group_can_be_used_twice_if_not_recursive() {
        let router = router(r#"
@version 1
define group "common"

key(f1) => group("common")
key(f1) => group("common")

group("common") => send_key('x')
"#);

        let result = fire_key(&router, Key::Function(1), Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![
                RouteEffect::Token(Token::press_utf8('x', Mods::EMPTY)),
                RouteEffect::Token(Token::press_utf8('x', Mods::EMPTY)),
            ],
        );
    }

    #[test]
    fn nested_group_expansion_is_not_a_cycle() {
        let router = router(r#"
@version 1
define group "a"
define group "b"
define group "c"
define group "d"

key(f1) => group("a")
group("a") => group("b")
group("a") => group("c")
group("b") => group("d")
group("c") => group("d")
group("d") => send_key('x')
"#);

        let result = fire_key(&router, Key::Function(1), Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![
                RouteEffect::Token(Token::press_utf8('x', Mods::EMPTY)),
                RouteEffect::Token(Token::press_utf8('x', Mods::EMPTY)),
            ],
        );
    }

    #[test]
    fn indirect_group_cycle_is_runtime_error() {
        let router = router(r#"
@version 1
define group "a"
define group "b"

key(f1) => group("a")
group("a") => group("b")
group("b") => group("a")
"#);

        let err = router.fire(RouteInput::Token(&Token::press_key(Key::Function(1), Mods::EMPTY))).unwrap_err();
        assert!(matches!(err, RouteError::GroupCycle { .. }));
    }

    #[test]
    fn longer_group_cycle_is_runtime_error() {
        let router = router(r#"
@version 1
define group "a"
define group "b"
define group "c"

key(f1) => group("a")
group("a") => group("b")
group("b") => group("c")
group("c") => group("b")
"#);

        let err = router.fire(RouteInput::Token(&Token::press_key(Key::Function(1), Mods::EMPTY))).unwrap_err();
        match err {
            RouteError::GroupCycle { stack, repeated } => {
                assert!(!stack.is_empty());
                assert!(stack.contains(&repeated));
            }
            _ => panic!("expected group cycle"),
        }
    }

    #[test]
    fn group_cycle_discards_partial_effects() {
        let router = router(r#"
@version 1
define group "a"
define group "b"

key(f1) => group("a")
group("a") => send_key('x')
group("a") => group("b")
group("b") => group("a")
"#);

        let err = router.fire(RouteInput::Token(&Token::press_key(Key::Function(1), Mods::EMPTY))).unwrap_err();
        assert!(matches!(err, RouteError::GroupCycle { .. }));
    }

    #[test]
    fn cycle_error_does_not_poison_later_fire() {
        let router = router(r#"
@version 1
define group "a"
define group "b"

key(f1) => group("a")
group("a") => group("b")
group("b") => group("a")

key(f2) => send_key('x')
"#);

        assert!(router.fire(RouteInput::Token(&Token::press_key(Key::Function(1), Mods::EMPTY))).is_err());

        let result = fire_key(&router, Key::Function(2), Mods::EMPTY);

        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('x', Mods::EMPTY))],
        );
    }
}
