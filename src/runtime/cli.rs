// SPDX-License-Identifier: MIT

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use lexopt::prelude::*;

#[derive(Debug, Clone)]
pub struct Cli {
    pub config: Option<PathBuf>,
    pub sock: Option<PathBuf>,
    pub no_implicit_config: bool,
    pub check_config: bool,
    pub dump_config: bool,
    pub command: Vec<OsString>,
}

impl Cli {
    pub fn parse() -> Result<Self, CliError> {
        let mut parser = lexopt::Parser::from_env();
        let mut cli = Self {
            config: None,
            sock: None,
            no_implicit_config: false,
            check_config: false,
            dump_config: false,
            command: Vec::new(),
        };

        while let Some(arg) = parser.next()? {
            match arg {
                Short('c') | Long("config") => cli.config = Some(PathBuf::from(parser.value()?)),
                Long("sock") => cli.sock = Some(PathBuf::from(parser.value()?)),
                Long("no-implicit-config") => cli.no_implicit_config = true,
                Long("check-config") => cli.check_config = true,
                Long("dump-config") => cli.dump_config = true,
                Short('h') | Long("help") => {
                    eprint!("\
usage: {} [options] command [args ...]
remap/inject/filter for terminal input based on config rules

options:
  -c, --config <PATH>       config file to use
      --sock <PATH>         path of the created socket (computes a unique one by default)
      --no-implicit-config  don't use an implicit config if found
      --check-config        parse config and exit
      --dump-config         parse config, print parse result, and exit
  -h, --help                display this help and exit
  -V, --version             output version information and exit
", std::env::args_os().next().as_deref().map(OsStr::to_string_lossy).unwrap_or(Cow::Borrowed("yxt")));
                    std::process::exit(2);
                }
                Short('V') | Long("version") => {
                    eprintln!("yxt v0.1.0-alpha");
                    std::process::exit(2);
                }
                Value(value) => {
                    cli.command.push(value);
                    cli.command.extend(parser.raw_args()?);
                    break;
                }
                _ => return Err(CliError::Lexopt(arg.unexpected())),
            }
        }

        if cli.command.is_empty() {
            Err(CliError::MissingCommand)
        } else {
            Ok(cli)
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Lexopt(#[from] lexopt::Error),

    #[error("missing command")]
    MissingCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPathKind {
    Explicit,
    Implicit,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfigPath<'a> {
    pub path: Cow<'a, Path>,
    pub kind: ConfigPathKind,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigPathError {
    #[error("command path {0:?} has no basename")]
    CommandHasNoBasename(OsString),

    #[error("no config path was provided (implicit config lookup would use '{implicit_path}' if enabled)")]
    NoConfigPath {
        implicit_path: PathBuf,
    },

    #[error("refusing to use implicit config '{implicit_path}' under UID 0 / setuid / setgid; pass --config explicitly")]
    ImplicitConfigRefused {
        implicit_path: PathBuf,
    },

    #[error("no config path was provided and implicit config '{implicit_path}' does not exist")]
    MissingImplicitConfig {
        implicit_path: PathBuf,
    },
}

pub fn config_path<'a>(cli: &'a Cli) -> Result<ResolvedConfigPath<'a>, ConfigPathError> {
    if let Some(path) = &cli.config {
        return Ok(ResolvedConfigPath {
            path: Cow::Borrowed(path.as_path()),
            kind: ConfigPathKind::Explicit,
        });
    }

    let basename = command_basename(&cli.command[0])?;
    let implicit_path = implicit_config_path(basename);

    if cli.no_implicit_config {
        Err(ConfigPathError::NoConfigPath { implicit_path })
    } else if refuse_implicit_config() {
        Err(ConfigPathError::ImplicitConfigRefused { implicit_path })
    } else if !implicit_path.exists() {
        Err(ConfigPathError::MissingImplicitConfig { implicit_path })
    } else {
        Ok(ResolvedConfigPath {
            path: Cow::Owned(implicit_path),
            kind: ConfigPathKind::Implicit,
        })
    }
}

fn command_basename(command: &OsStr) -> Result<&OsStr, ConfigPathError> {
    Path::new(command).file_name().filter(|name| !name.is_empty())
        .ok_or_else(|| ConfigPathError::CommandHasNoBasename(command.to_owned()))
}

fn implicit_config_path(basename: &OsStr) -> PathBuf {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(xdg) => PathBuf::from(xdg),
        None => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join(".config"),
    }.join("yxt").join("implicit").join(basename).with_extension("conf")
}

fn refuse_implicit_config() -> bool {
    let ruid = unsafe { libc::getuid() };
    let euid = unsafe { libc::geteuid() };
    let rgid = unsafe { libc::getgid() };
    let egid = unsafe { libc::getegid() };
    ruid == 0 || euid == 0 || ruid != euid || rgid != egid
}
