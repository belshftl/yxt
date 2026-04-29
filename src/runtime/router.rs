// SPDX-License-Identifier: MIT

use std::collections::HashMap;

use crate::model::{Action, Config, Event, GroupId, Key, KeyEventKind, Mods, Source, Target, Token};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    GroupCycle {
        stack: Vec<GroupId>,
        repeated: GroupId,
    },
}

#[derive(Debug)]
pub struct Router {
    by_source: HashMap<SourceKey, Vec<Target>>,
    group_count: usize,
}

impl Router {
    pub fn new(config: &Config) -> Self {
        let mut by_source: HashMap<SourceKey, Vec<Target>> = HashMap::new();
        for mapping in &config.mappings {
            by_source.entry(SourceKey::from(&mapping.from)).or_default().push(mapping.to.clone());
        }
        Self {
            by_source,
            group_count: config.groups.len(),
        }
    }

    pub fn fire(&self, source: &Source) -> Result<RouteResult, RouteError> {
        let mut ctx = FireCtx::new(self.group_count);
        let source_kind = source_kind(source);
        self.fire_source(source, source_kind, &mut ctx)?;
        Ok(RouteResult {
            matched: ctx.matched,
            effects: ctx.effects,
        })
    }

    fn fire_source(&self, source: &Source, source_kind: Option<KeyEventKind>, ctx: &mut FireCtx) -> Result<(), RouteError> {
        let key = SourceKey::from(source);
        let Some(targets) = self.by_source.get(&key) else {
            return Ok(());
        };
        ctx.matched = true;
        for target in targets {
            self.fire_target(target, source_kind, ctx)?;
        }
        Ok(())
    }

    fn fire_target(&self, target: &Target, source_kind: Option<KeyEventKind>, ctx: &mut FireCtx) -> Result<(), RouteError> {
        match target {
            Target::Token(token) => {
                let token = match source_kind {
                    Some(kind) => token.clone().with_kind(kind),
                    None => token.clone(),
                };
                ctx.effects.push(RouteEffect::Token(token));
                Ok(())
            }
            Target::Action(action) => {
                if fires_non_token_target(source_kind) {
                    ctx.effects.push(RouteEffect::Action(action.clone()));
                }
                Ok(())
            }
            Target::Group(group) => {
                if fires_non_token_target(source_kind) {
                    self.fire_group(*group, ctx)?;
                }
                Ok(())
            }
        }
    }

    fn fire_group(&self, group: GroupId, ctx: &mut FireCtx) -> Result<(), RouteError> {
        ctx.push_group(group)?;
        let result = self.fire_source(&Source::Group(group), None, ctx);
        ctx.pop_group(group);
        result
    }
}

fn source_kind(source: &Source) -> Option<KeyEventKind> {
    match source {
        Source::Token(token) => Some(token.kind()),
        Source::Event(_) | Source::Group(_) => None,
    }
}

fn fires_non_token_target(kind: Option<KeyEventKind>) -> bool {
    match kind {
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SourceKey {
    Event(Event),
    Token(TokenPattern),
    Group(GroupId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TokenPattern {
    Utf8 {
        ch: char,
        mods: Mods,
    },
    Key {
        key: Key,
        mods: Mods,
    },
}

impl From<&Token> for TokenPattern {
    fn from(token: &Token) -> Self {
        match *token {
            Token::Utf8 { ch, mods, .. } => Self::Utf8 {
                ch,
                mods,
            },
            Token::Key { key, mods, .. } => Self::Key {
                key,
                mods,
            },
        }
    }
}

impl From<&Source> for SourceKey {
    fn from(source: &Source) -> Self {
        match source {
            Source::Event(event) => Self::Event(event.clone()),
            Source::Token(token) => Self::Token(TokenPattern::from(token)),
            Source::Group(group) => Self::Group(*group),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use crate::config::loader::ConfigLoader;
    use crate::model::{
        Action, Event, Key, Mods, Source, Token,
    };

    fn cfg(src: &str) -> crate::model::Config {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.conf");
        fs::write(&path, src).unwrap();
        ConfigLoader::new().parse_file(&path).unwrap()
    }

    fn router(src: &str) -> Router {
        Router::new(&cfg(src))
    }

    fn key_source(key: Key, mods: Mods) -> Source {
        Source::Token(Token::press_key(key, mods))
    }

    fn utf8_source(ch: char, mods: Mods) -> Source {
        Source::Token(Token::press_utf8(ch, mods))
    }

    fn sockdata_source(data: &[u8]) -> Source {
        Source::Event(Event::Sockdata(data.to_vec()))
    }

    #[test]
    fn unmatched_source_produces_no_effects() {
        let router = router(r#"
@version 1
tok_key(f1) => tok_utf8("x")
"#);
        let result = router.fire(&key_source(Key::Function(2), Mods::EMPTY)).unwrap();
        assert!(!result.matched);
        assert!(result.effects.is_empty());
    }

    #[test]
    fn direct_token_mapping() {
        let router = router(r#"
@version 1
tok_key(f1) => tok_utf8("x")
"#);
        let result = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('x', Mods::EMPTY))],
        );
    }

    #[test]
    fn direct_action_mapping() {
        let router = router(r#"
@version 1
tok_key(f1) => act_shell("echo hi")
"#);
        let result = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Action(Action::Shell("echo hi".to_owned()))],
        );
    }

    #[test]
    fn event_source_mapping() {
        let router = router(r#"
@version 1
evt_sockdata("reload") => act_shell("reload")
"#);
        let result = router.fire(&sockdata_source(b"reload")).unwrap();
        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Action(Action::Shell("reload".to_owned()))],
        );
        assert!(!router.fire(&sockdata_source(b"reload\n")).unwrap().matched);
    }

    #[test]
    fn source_matching_is_exact_for_modifiers() {
        let router = router(r#"
@version 1
tok_key(f1, ctrl) => tok_utf8("x")
"#);
        assert!(!router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap().matched);
        assert!(!router.fire(&key_source(Key::Function(1), Mods::ALT)).unwrap().matched);
        let result = router.fire(&key_source(Key::Function(1), Mods::CTRL)).unwrap();
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
tok_key(f1) => tok_utf8("x", ctrl, alt)
"#);
        let result = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('x', Mods::CTRL | Mods::ALT))],
        );
    }

    #[test]
    fn multiple_mappings_for_same_source_preserve_order() {
        let router = router(r#"
@version 1
tok_key(f1) => tok_utf8("a")
tok_key(f1) => tok_utf8("b")
tok_key(f1) => act_shell("c")
tok_key(f1) => tok_utf8("d")
"#);
        let result = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![
                RouteEffect::Token(Token::press_utf8('a', Mods::EMPTY)),
                RouteEffect::Token(Token::press_utf8('b', Mods::EMPTY)),
                RouteEffect::Action(Action::Shell("c".to_owned())),
                RouteEffect::Token(Token::press_utf8('d', Mods::EMPTY)),
            ],
        );
    }

    #[test]
    fn group_expands_to_effects() {
        let router = router(r#"
@version 1
define group "reload"

tok_key(f5) => group("reload")
group("reload") => tok_utf8("r")
group("reload") => act_shell("reload")
"#);
        let result = router.fire(&key_source(Key::Function(5), Mods::EMPTY)).unwrap();
        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![
                RouteEffect::Token(Token::press_utf8('r', Mods::EMPTY)),
                RouteEffect::Action(Action::Shell("reload".to_owned())),
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

tok_key(f1) => group("a")
group("a") => tok_utf8("a")
group("a") => group("b")
group("a") => tok_utf8("d")

group("b") => tok_utf8("b")
group("b") => group("c")

group("c") => tok_utf8("c")
"#);
        let result = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
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
tok_key(f1) => tok_utf8("x")
tok_utf8("x") => act_shell("should run only for external x")
"#);
        let result = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('x', Mods::EMPTY))],
        );

        let result = router.fire(&utf8_source('x', Mods::EMPTY)).unwrap();
        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Action(Action::Shell("should run only for external x".to_owned()))],
        );
    }

    #[test]
    fn token_to_token_preserves_press_repeat_release() {
        let router = router(r#"
@version 1
tok_utf8("x") => tok_utf8("y")
"#);

        for kind in [KeyEventKind::Press, KeyEventKind::Repeat, KeyEventKind::Release] {
            let result = router.fire(&Source::Token(Token::Utf8 {
                ch: 'x',
                mods: Mods::EMPTY,
                kind,
            })).unwrap();
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

tok_utf8("x") => group("g")
group("g") => tok_utf8("y")
"#);

        for kind in [KeyEventKind::Press, KeyEventKind::Repeat] {
            let result = router.fire(&Source::Token(Token::Utf8 {
                ch: 'x',
                mods: Mods::EMPTY,
                kind,
            })).unwrap();
            assert!(result.matched);
            assert_eq!(
                result.effects,
                vec![RouteEffect::Token(Token::press_utf8('y', Mods::EMPTY))],
            );
        }

        let result = router.fire(&Source::Token(Token::Utf8 {
            ch: 'x',
            mods: Mods::EMPTY,
            kind: KeyEventKind::Release,
        })).unwrap();
        assert!(result.matched);
        assert!(result.effects.is_empty());
    }

    #[test]
    fn token_to_action_fires_on_press_and_repeat_not_release() {
        let router = router(r#"
@version 1
tok_utf8("x") => act_shell("x")
"#);

        for kind in [KeyEventKind::Press, KeyEventKind::Repeat] {
            let result = router.fire(&Source::Token(Token::Utf8 {
                ch: 'x',
                mods: Mods::EMPTY,
                kind,
            })).unwrap();
            assert!(result.matched);
            assert_eq!(
                result.effects,
                vec![RouteEffect::Action(Action::Shell("x".to_owned()))],
            );
        }

        let result = router.fire(&Source::Token(Token::Utf8 {
            ch: 'x',
            mods: Mods::EMPTY,
            kind: KeyEventKind::Release,
        })).unwrap();
        assert!(result.matched);
        assert!(result.effects.is_empty());
    }

    #[test]
    fn unmatched_release_is_not_matched() {
        let router = router(r#"
@version 1
tok_utf8("x") => tok_utf8("y")
"#);
        let result = router.fire(&Source::Token(Token::Utf8 {
            ch: 'z',
            mods: Mods::EMPTY,
            kind: KeyEventKind::Release,
        })).unwrap();
        assert!(!result.matched);
        assert!(result.effects.is_empty());
    }

    #[test]
    fn same_group_can_be_used_twice_if_not_recursive() {
        let router = router(r#"
@version 1
define group "common"

tok_key(f1) => group("common")
tok_key(f1) => group("common")

group("common") => tok_utf8("x")
"#);
        let result = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
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

tok_key(f1) => group("a")
group("a") => group("b")
group("a") => group("c")
group("b") => group("d")
group("c") => group("d")
group("d") => tok_utf8("x")
"#);
        let result = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
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

tok_key(f1) => group("a")
group("a") => group("b")
group("b") => group("a")
"#);
        let err = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap_err();
        assert!(matches!(err, RouteError::GroupCycle { .. }));
    }

    #[test]
    fn longer_group_cycle_is_runtime_error() {
        let router = router(r#"
@version 1
define group "a"
define group "b"
define group "c"

tok_key(f1) => group("a")
group("a") => group("b")
group("b") => group("c")
group("c") => group("b")
"#);
        let err = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap_err();
        match err {
            RouteError::GroupCycle { stack, repeated } => {
                assert!(!stack.is_empty());
                assert!(stack.contains(&repeated));
            }
        }
    }

    #[test]
    fn group_cycle_discards_partial_effects() {
        let router = router(r#"
@version 1
define group "a"
define group "b"

tok_key(f1) => group("a")
group("a") => tok_utf8("x")
group("a") => group("b")
group("b") => group("a")
"#);
        let err = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap_err();
        assert!(matches!(err, RouteError::GroupCycle { .. }));
    }

    #[test]
    fn cycle_error_does_not_poison_later_fire() {
        let router = router(r#"
@version 1
define group "a"
define group "b"

tok_key(f1) => group("a")
group("a") => group("b")
group("b") => group("a")

tok_key(f2) => tok_utf8("x")
"#);
        assert!(router.fire(&key_source(Key::Function(1), Mods::EMPTY)).is_err());
        let result = router.fire(&key_source(Key::Function(2), Mods::EMPTY)).unwrap();
        assert!(result.matched);
        assert_eq!(
            result.effects,
            vec![RouteEffect::Token(Token::press_utf8('x', Mods::EMPTY))],
        );
    }
}
