// SPDX-License-Identifier: MIT

use std::collections::HashMap;

use crate::model::{Action, Config, GroupId, Source, Target, Token};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteEffect {
    Token(Token),
    Action(Action),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    GroupCycle {
        stack: Vec<GroupId>,
        repeated: GroupId,
    },
}

struct FireCtx {
    active_groups: Vec<bool>,
    group_stack: Vec<GroupId>,
    out: Vec<RouteEffect>,
}

impl FireCtx {
    fn new(group_count: usize) -> Self {
        Self {
            active_groups: vec![false; group_count],
            group_stack: Vec::new(),
            out: Vec::new(),
        }
    }

    fn finish(self) -> Vec<RouteEffect> {
        self.out
    }

    fn is_group_active(&self, group: GroupId) -> bool {
        self.active_groups[group.0]
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

    fn push_effect(&mut self, effect: RouteEffect) {
        self.out.push(effect);
    }
}

#[derive(Debug)]
pub struct Router {
    by_source: HashMap<Source, Vec<Target>>,
    group_count: usize,
}

impl Router {
    pub fn new(config: &Config) -> Self {
        let mut by_source: HashMap<Source, Vec<Target>> = HashMap::new();
        for mapping in &config.mappings {
            by_source.entry(mapping.from.clone()).or_default().push(mapping.to.clone());
        }
        Self {
            by_source,
            group_count: config.groups.len(),
        }
    }

    pub fn fire(&self, source: &Source) -> Result<Vec<RouteEffect>, RouteError> {
        let mut ctx = FireCtx::new(self.group_count);
        self.fire_source(source, &mut ctx)?;
        Ok(ctx.finish())
    }

    fn fire_source(&self, source: &Source, ctx: &mut FireCtx) -> Result<(), RouteError> {
        let Some(targets) = self.by_source.get(source) else {
            return Ok(());
        };
        for target in targets {
            self.fire_target(target, ctx)?;
        }
        Ok(())
    }

    fn fire_target(&self, target: &Target, ctx: &mut FireCtx) -> Result<(), RouteError> {
        match target {
            Target::Token(token) => {
                ctx.push_effect(RouteEffect::Token(token.clone()));
                Ok(())
            }
            Target::Action(action) => {
                ctx.push_effect(RouteEffect::Action(action.clone()));
                Ok(())
            }
            Target::Group(group) => self.fire_group(*group, ctx)
        }
    }

    fn fire_group(&self, group: GroupId, ctx: &mut FireCtx) -> Result<(), RouteError> {
        ctx.push_group(group)?;
        let result = self.fire_source(&Source::Group(group), ctx);
        ctx.pop_group(group);
        result
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
        Source::Token(Token::Key {
            key,
            mods,
        })
    }

    fn utf8_source(ch: char, mods: Mods) -> Source {
        Source::Token(Token::Utf8 {
            ch,
            mods,
        })
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
        let effects = router.fire(&key_source(Key::Function(2), Mods::EMPTY)).unwrap();
        assert!(effects.is_empty());
    }

    #[test]
    fn direct_token_mapping() {
        let router = router(r#"
@version 1
tok_key(f1) => tok_utf8("x")
"#);
        let effects = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
        assert_eq!(
            effects,
            vec![
                RouteEffect::Token(Token::Utf8 {
                    ch: 'x',
                    mods: Mods::EMPTY,
                }),
            ],
        );
    }

    #[test]
    fn direct_action_mapping() {
        let router = router(r#"
@version 1
tok_key(f1) => act_shell("echo hi")
"#);
        let effects = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
        assert_eq!(
            effects,
            vec![
                RouteEffect::Action(Action::Shell("echo hi".to_owned())),
            ],
        );
    }

    #[test]
    fn event_source_mapping() {
        let router = router(r#"
@version 1
evt_sockdata("reload") => act_shell("reload")
"#);
        let effects = router.fire(&sockdata_source(b"reload")).unwrap();
        assert_eq!(
            effects,
            vec![
                RouteEffect::Action(Action::Shell("reload".to_owned())),
            ],
        );
        assert!(router.fire(&sockdata_source(b"reload\n")).unwrap().is_empty());
    }

    #[test]
    fn source_matching_is_exact_for_modifiers() {
        let router = router(r#"
@version 1
tok_key(f1, ctrl) => tok_utf8("x")
"#);
        assert!(router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap().is_empty());
        assert!(router.fire(&key_source(Key::Function(1), Mods::ALT)).unwrap().is_empty());
        assert_eq!(
            router
                .fire(&key_source(Key::Function(1), Mods::CTRL))
                .unwrap(),
            vec![
                RouteEffect::Token(Token::Utf8 {
                    ch: 'x',
                    mods: Mods::EMPTY,
                }),
            ],
        );
    }

    #[test]
    fn target_token_modifiers_are_preserved() {
        let router = router(r#"
@version 1
tok_key(f1) => tok_utf8("x", ctrl, alt)
"#);
        let effects = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
        assert_eq!(
            effects,
            vec![
                RouteEffect::Token(Token::Utf8 {
                    ch: 'x',
                    mods: Mods::CTRL | Mods::ALT,
                }),
            ],
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
        let effects = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
        assert_eq!(
            effects,
            vec![
                RouteEffect::Token(Token::Utf8 {
                    ch: 'a',
                    mods: Mods::EMPTY,
                }),
                RouteEffect::Token(Token::Utf8 {
                    ch: 'b',
                    mods: Mods::EMPTY,
                }),
                RouteEffect::Action(Action::Shell("c".to_owned())),
                RouteEffect::Token(Token::Utf8 {
                    ch: 'd',
                    mods: Mods::EMPTY,
                }),
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
        let effects = router.fire(&key_source(Key::Function(5), Mods::EMPTY)).unwrap();
        assert_eq!(
            effects,
            vec![
                RouteEffect::Token(Token::Utf8 {
                    ch: 'r',
                    mods: Mods::EMPTY,
                }),
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
        let effects = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
        assert_eq!(
            effects,
            vec![
                RouteEffect::Token(Token::Utf8 {
                    ch: 'a',
                    mods: Mods::EMPTY,
                }),
                RouteEffect::Token(Token::Utf8 {
                    ch: 'b',
                    mods: Mods::EMPTY,
                }),
                RouteEffect::Token(Token::Utf8 {
                    ch: 'c',
                    mods: Mods::EMPTY,
                }),
                RouteEffect::Token(Token::Utf8 {
                    ch: 'd',
                    mods: Mods::EMPTY,
                }),
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
        let effects = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
        assert_eq!(
            effects,
            vec![
                RouteEffect::Token(Token::Utf8 {
                    ch: 'x',
                    mods: Mods::EMPTY,
                }),
            ],
        );

        let effects = router.fire(&utf8_source('x', Mods::EMPTY)).unwrap();
        assert_eq!(
            effects,
            vec![
                RouteEffect::Action(Action::Shell(
                    "should run only for external x".to_owned(),
                )),
            ],
        );
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
        let effects = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
        assert_eq!(
            effects,
            vec![
                RouteEffect::Token(Token::Utf8 {
                    ch: 'x',
                    mods: Mods::EMPTY,
                }),
                RouteEffect::Token(Token::Utf8 {
                    ch: 'x',
                    mods: Mods::EMPTY,
                }),
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
        let effects = router.fire(&key_source(Key::Function(1), Mods::EMPTY)).unwrap();
        assert_eq!(
            effects,
            vec![
                RouteEffect::Token(Token::Utf8 {
                    ch: 'x',
                    mods: Mods::EMPTY,
                }),
                RouteEffect::Token(Token::Utf8 {
                    ch: 'x',
                    mods: Mods::EMPTY,
                }),
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
        assert_eq!(
            router.fire(&key_source(Key::Function(2), Mods::EMPTY)).unwrap(),
            vec![
                RouteEffect::Token(Token::Utf8 {
                    ch: 'x',
                    mods: Mods::EMPTY,
                }),
            ],
        );
    }
}
