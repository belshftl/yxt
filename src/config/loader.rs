// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};

use crate::model::Config;
use super::line::{Expr, FileId, LineCtx, Literal, ParseError, Span, Stmt, parse_line};
use super::lower::{ConfigBuilder, ConfigError};

#[derive(Debug, Default)]
pub struct SourceMap {
    files: Vec<PathBuf>,
}

impl SourceMap {
    pub fn add_file(&mut self, path: PathBuf) -> FileId {
        let id = FileId(self.files.len());
        self.files.push(path);
        id
    }

    pub fn path(&self, id: FileId) -> &Path {
        &self.files[id.0]
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigLoadError {
    #[error("I/O error while parsing file '{path}': {err}")]
    Io { path: PathBuf, err: std::io::Error },

    #[error("@include cycle (TODO: format message)")]
    IncludeCycle { path: PathBuf, stack: Vec<PathBuf> },

    #[error("unsupported version '{version}'")]
    UnsupportedVersion { version: u32, span: Span },

    #[error("missing @version directive")]
    MissingVersion { path: PathBuf },

    #[error("@version directive must be the first statement in the root config")]
    VersionMustBeFirst { span: Span },

    #[error("duplicate @version directive in file")]
    DuplicateVersion { span: Span },

    #[error("@version directive versions don't match across included files: expected {expected}, got {got}")]
    VersionMismatch { expected: u32, got: u32, span: Span },

    #[error("invalid arguments for directive '@version'")]
    BadVersionArgs { span: Span },

    #[error("invalid arguments for directive '@include'")]
    BadIncludeArgs { span: Span },

    #[error(transparent)]
    Syntax(#[from] ParseError),

    #[error(transparent)]
    Semantic(#[from] ConfigError),
}

pub struct ConfigLoader {
    sources: SourceMap,
    include_stack: Vec<PathBuf>,
    version: Option<u32>,
    seen_non_version_stmt: bool,
    builder: ConfigBuilder,
}

impl ConfigLoader {
    pub fn new() -> Self {
        Self {
            sources: SourceMap::default(),
            include_stack: Vec::new(),
            version: None,
            seen_non_version_stmt: false,
            builder: ConfigBuilder::default(),
        }
    }

    pub fn parse_file(mut self, path: &Path) -> Result<Config, ConfigLoadError> {
        let path = std::fs::canonicalize(path).map_err(|e| ConfigLoadError::Io {
            path: path.to_owned(),
            err: e,
        })?;
        self.parse_one_file(&path, true)?;
        if self.version.is_none() {
            return Err(ConfigLoadError::MissingVersion { path });
        }
        self.builder.finish().map_err(ConfigLoadError::Semantic)
    }

    fn parse_one_file(&mut self, path: &Path, is_root: bool) -> Result<(), ConfigLoadError> {
        let path = std::fs::canonicalize(path).map_err(|e| ConfigLoadError::Io {
            path: path.to_owned(),
            err: e,
        })?;

        if self.include_stack.iter().any(|p| p == &path) {
            return Err(ConfigLoadError::IncludeCycle {
                path,
                stack: self.include_stack.clone(),
            });
        }
        self.include_stack.push(path.clone());

        let file = self.sources.add_file(path.clone());
        let text = std::fs::read_to_string(&path).map_err(|e| ConfigLoadError::Io {
            path: path.clone(),
            err: e,
        })?;
        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));

        let mut seen_non_version_stmt_here = false;
        for (idx, line) in text.lines().enumerate() {
            let line_no = idx + 1;
            let ctx = LineCtx {
                file,
                line: line_no,
            };
            if let Some(stmt) = parse_line(line, ctx)? {
                self.apply_parsed_stmt(stmt, is_root, base_dir, &mut seen_non_version_stmt_here)?;
            }
        }
        self.include_stack.pop();
        Ok(())
    }

    fn apply_parsed_stmt(
        &mut self, stmt: Stmt, is_root: bool, base_dir: &Path,
        seen_non_version_stmt_here: &mut bool
    ) -> Result<(), ConfigLoadError> {
        match stmt {
            Stmt::Directive { name, args, span } if name == "version" => {
                if *seen_non_version_stmt_here {
                    return Err(ConfigLoadError::VersionMustBeFirst { span });
                }
                self.apply_version(args, span, is_root)
            }
            Stmt::Directive { name, args, span } if name == "include" => {
                if is_root && self.version.is_none() {
                    return Err(ConfigLoadError::VersionMustBeFirst { span });
                }
                *seen_non_version_stmt_here = true;
                self.seen_non_version_stmt = true;
                self.apply_include(args, span, base_dir)
            }
            other => {
                if is_root && self.version.is_none() {
                    return Err(ConfigLoadError::VersionMustBeFirst { span: other.span() });
                }
                *seen_non_version_stmt_here = true;
                self.seen_non_version_stmt = true;
                self.builder.apply_stmt(other).map_err(ConfigLoadError::Semantic)
            }
        }
    }

    fn apply_version(&mut self, args: Vec<Expr>, span: Span, is_root: bool) -> Result<(), ConfigLoadError> {
        let version = expect_version_arg(args, span)?;
        if version != 1 {
            return Err(ConfigLoadError::UnsupportedVersion { version, span });
        }

        if is_root {
            if self.seen_non_version_stmt {
                return Err(ConfigLoadError::VersionMustBeFirst { span });
            }
            if self.version.replace(version).is_some() {
                return Err(ConfigLoadError::DuplicateVersion { span });
            }
            return Ok(());
        }

        let Some(root_version) = self.version else {
            return Err(ConfigLoadError::VersionMustBeFirst { span });
        };

        if version != root_version {
            return Err(ConfigLoadError::VersionMismatch {
                expected: root_version,
                got: version,
                span,
            });
        }
        Ok(())
    }

    fn apply_include(&mut self, args: Vec<Expr>, span: Span, base_dir: &Path) -> Result<(), ConfigLoadError> {
        let include_path = expect_include_arg(args, span)?;
        let path = if include_path.is_absolute() {
            include_path
        } else {
            base_dir.join(include_path)
        };
        self.parse_one_file(&path, false)
    }
}

fn expect_version_arg(args: Vec<Expr>, span: Span) -> Result<u32, ConfigLoadError> {
    let mut args = args.into_iter();
    let Some(Expr::Literal { value: Literal::Int(version), .. }) = args.next() else {
        return Err(ConfigLoadError::BadVersionArgs { span });
    };
    if args.next().is_some() {
        return Err(ConfigLoadError::BadVersionArgs { span });
    }
    u32::try_from(version).map_err(|_| ConfigLoadError::BadVersionArgs { span })
}

fn expect_include_arg(args: Vec<Expr>, span: Span) -> Result<PathBuf, ConfigLoadError> {
    let mut args = args.into_iter();
    let Some(Expr::Literal { value: Literal::String(path), .. }) = args.next() else {
        return Err(ConfigLoadError::BadIncludeArgs { span });
    };
    if args.next().is_some() {
        return Err(ConfigLoadError::BadIncludeArgs { span });
    }
    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    use crate::config::lower::ErrorKind;
    use crate::model::{
        Action, CommandSpec, Event, Key, KeyPattern, Mods, ModsPattern, Source, Target, TokenPattern,
    };

    fn write_file(dir: &TempDir, rel: &str, text: &str) -> std::path::PathBuf {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, text).unwrap();
        path
    }

    fn parse(path: &std::path::Path) -> Result<crate::model::Config, ConfigLoadError> {
        ConfigLoader::new().parse_file(path)
    }

    fn parse_err(path: &std::path::Path) -> ConfigLoadError {
        parse(path).unwrap_err()
    }

    #[test]
    fn empty_root_requires_version() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_file(&dir, "root.conf", r#"
# comment
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::MissingVersion { .. }));
    }

    #[test]
    fn root_requires_version() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_file(&dir, "root.conf", r#"
define group "x"
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::VersionMustBeFirst { .. }));
    }

    #[test]
    fn root_version_may_follow_empty_lines_and_comments() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_file(&dir, "root.conf", r#"
# comment
    # comment with leading whitespace
@version 1

key(f1) => send_key('x')
"#);

        let cfg = parse(&root).unwrap();
        assert_eq!(cfg.mappings.len(), 1);
    }

    #[test]
    fn root_version_must_be_first_stmt_definition_before_version() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_file(&dir, "root.conf", r#"
define group "x"
@version 1
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::VersionMustBeFirst { .. }));
    }

    #[test]
    fn root_version_must_be_first_stmt_mapping_before_version() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_file(&dir, "root.conf", r#"
key(f1) => send_key('x')
@version 1
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::VersionMustBeFirst { .. }));
    }

    #[test]
    fn root_version_must_be_first_stmt_include_before_version() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "inc.conf", r#"
key(f1) => send_key('x')
"#);
        let root = write_file(&dir, "root.conf", r#"
@include "inc.conf"
@version 1
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::VersionMustBeFirst { .. }));
    }

    #[test]
    fn root_duplicate_version_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_file(&dir, "root.conf", r#"
@version 1
@version 1
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::DuplicateVersion { .. }));
    }

    #[test]
    fn unsupported_root_version_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_file(&dir, "root.conf", r#"
@version 2
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::UnsupportedVersion { version: 2, .. }));
    }

    #[test]
    fn bad_root_version_args_are_errors() {
        for (idx, text) in [
            r#"@version"#,
            r#"@version "1""#,
            r#"@version 1 2"#,
            r#"@version -1"#,
        ].into_iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let root = write_file(&dir, &format!("root-{idx}.conf"), text);

            let err = parse_err(&root);
            assert!(matches!(err, ConfigLoadError::BadVersionArgs { .. }), "{text:?}: {err:?}");
        }
    }

    #[test]
    fn include_expands_inline_and_relative_to_root() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "groups.conf", r#"
define group "reload"
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "groups.conf"
key(f5) => group("reload")
group("reload") => sh("reload")
"#);

        let cfg = parse(&root).unwrap();
        assert_eq!(cfg.groups.len(), 1);
        assert_eq!(cfg.groups.name(cfg.groups.lookup("reload").unwrap()), "reload");
        assert_eq!(cfg.mappings.len(), 2);
        assert_eq!(
            cfg.mappings[0].from,
            Source::Token(TokenPattern::Key {
                key: KeyPattern::Named(Key::Function(5)),
                mods: ModsPattern::AnyOf(vec![Mods::EMPTY]),
            }),
        );
        assert!(matches!(cfg.mappings[0].to, Target::Group(_)));
        assert_eq!(
            cfg.mappings[1].to,
            Target::Action(Action::Command(CommandSpec::Shell {
                command: "reload".to_owned()
            })),
        );
    }

    #[test]
    fn include_order_is_semantically_inline() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "groups.conf", r#"
define group "x"
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
key(f1) => group("x")
@include "groups.conf"
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::Semantic(ConfigError {
            kind: ErrorKind::UnknownGroup { name }, ..
        }) if name == "x"));
    }

    #[test]
    fn nested_include_resolves_relative_to_including_file() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "sub/groups.conf", r#"
define group "nested"
"#);
        write_file(&dir, "sub/include-groups.conf", r#"
@include "groups.conf"
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "sub/include-groups.conf"
key(f1) => group("nested")
"#);

        let cfg = parse(&root).unwrap();
        assert_eq!(cfg.groups.len(), 1);
        assert!(cfg.groups.lookup("nested").is_some());
        assert_eq!(cfg.mappings.len(), 1);
    }

    #[test]
    fn included_file_inherits_root_version_when_omitted() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "inc.conf", r#"
key(f1) => send_key('x')
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "inc.conf"
"#);

        let cfg = parse(&root).unwrap();
        assert_eq!(cfg.mappings.len(), 1);
    }

    #[test]
    fn included_file_may_repeat_matching_version() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "inc.conf", r#"
@version 1
key(f1) => send_key('x')
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "inc.conf"
"#);

        let cfg = parse(&root).unwrap();
        assert_eq!(cfg.mappings.len(), 1);
    }

    #[test]
    fn included_version_must_match_root_version() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "inc.conf", r#"
@version 2
key(f1) => send_key('x')
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "inc.conf"
"#);

        let err = parse_err(&root);
        assert!(matches!(
            err,
            ConfigLoadError::VersionMismatch {
                expected: 1,
                got: 2,
                ..
            }
            | ConfigLoadError::UnsupportedVersion {
                version: 2,
                ..
            }
        ));
    }

    #[test]
    fn included_version_must_appear_before_stmts() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "inc.conf", r#"
key(f1) => send_key('x')
@version 1
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "inc.conf"
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::VersionMustBeFirst { .. }));
    }

    #[test]
    fn bad_include_args_are_error() {
        for (idx, text) in [
            r#"
@version 1
@include
"#,
            r#"
@version 1
@include 123
"#,
            r#"
@version 1
@include "a" "b"
"#,
        ].into_iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let root = write_file(&dir, &format!("root-{idx}.conf"), text);

            let err = parse_err(&root);
            assert!(matches!(err, ConfigLoadError::BadIncludeArgs { .. }), "{text:?}: {err:?}");
        }
    }

    #[test]
    fn missing_include_file_is_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "missing.conf"
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::Io { .. }));
    }

    #[test]
    fn direct_include_cycle_is_error() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "a.conf", r#"
@include "a.conf"
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "a.conf"
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::IncludeCycle { .. }));
    }

    #[test]
    fn indirect_include_cycle_is_error() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "a.conf", r#"
@include "b.conf"
"#);
        write_file(&dir, "b.conf", r#"
@include "c.conf"
"#);
        write_file(&dir, "c.conf", r#"
@include "a.conf"
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "a.conf"
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::IncludeCycle { .. }));
    }

    #[test]
    fn repeated_include_is_allowed_but_semantics_may_reject_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "group.conf", r#"
define group "x"
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "group.conf"
@include "group.conf"
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::Semantic(ConfigError {
            kind: ErrorKind::DuplicateGroup { name }, ..
        }) if name == "x"));
    }

    #[test]
    fn nested_include_without_duplicate_definitions_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "common-a.conf", r#"
key(f1) => send_key('a')
"#);
        write_file(&dir, "common-b.conf", r#"
key(f2) => send_key('b')
"#);
        write_file(&dir, "left.conf", r#"
@include "common-a.conf"
"#);
        write_file(&dir, "right.conf", r#"
@include "common-b.conf"
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "left.conf"
@include "right.conf"
"#);

        let cfg = parse(&root).unwrap();
        assert_eq!(cfg.mappings.len(), 2);
    }

    #[test]
    fn syntax_error_in_root_is_reported_as_syntax() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_file(&dir, "root.conf", r#"
@version 1
key(f1) => send_key('x') => send_key('y')
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::Syntax { .. }));
    }

    #[test]
    fn syntax_error_in_included_file_is_reported_as_syntax() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "bad.conf", r#"
key(f1) => send_key('x') => send_key('y')
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "bad.conf"
"#);

        let err = parse_err(&root);
        assert!(matches!(err, ConfigLoadError::Syntax { .. }));
    }

    #[test]
    fn semantic_error_in_included_file_has_included_line_span() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "bad.conf", r#"
# line 1
# line 2
key(nope) => send_key('x')
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "bad.conf"
"#);

        let err = parse_err(&root);
        match err {
            ConfigLoadError::Semantic(ConfigError { kind: ErrorKind::UnknownKey { name }, span }) => {
                assert_eq!(name, "nope");
                assert_eq!(span.ctx.line, 4);
            }
            other => panic!("expected unknown key, got {other:?}"),
        }
    }

    #[test]
    fn include_path_with_comment_and_hash_inside_string() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir, "a#b.conf", r#"
key(f1) => send_key('x')
"#);
        let root = write_file(&dir, "root.conf", r#"
@version 1
@include "a#b.conf" # this is a comment
"#);

        let cfg = parse(&root).unwrap();
        assert_eq!(cfg.mappings.len(), 1);
    }

    #[test]
    fn absolute_include_path_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let inc = write_file(&dir, "inc.conf", r#"
key(f1) => send_key('x')
"#);
        let root = write_file(&dir, "root.conf", &format!(r#"
@version 1
@include "{}"
"#, inc.display()));

        let cfg = parse(&root).unwrap();
        assert_eq!(cfg.mappings.len(), 1);
    }

    #[test]
    fn parser_finishes_builder_into_config() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_file(&dir, "root.conf", r#"
@version 1
define group "reload"

key(f5) => group("reload")
sockdata_utf8("reload") => group("reload")
group("reload") => send_key('r', ctrl)
group("reload") => sh("reload")
"#);
        let cfg = parse(&root).unwrap();

        assert_eq!(cfg.groups.len(), 1);
        assert_eq!(cfg.mappings.len(), 4);
        assert_eq!(
            cfg.mappings[1].from,
            Source::Event(Event::Sockdata(b"reload".to_vec())),
        );
    }
}
