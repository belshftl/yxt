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
    pub help: bool,
    pub version: bool,
    pub command: Vec<OsString>,
}

impl Cli {
    pub fn parse() -> Result<Self, lexopt::Error> {
        let mut parser = lexopt::Parser::from_env();
        let mut cli = Self {
            config: None,
            sock: None,
            no_implicit_config: false,
            check_config: false,
            dump_config: false,
            help: false,
            version: false,
            command: Vec::new(),
        };

        while let Some(arg) = parser.next()? {
            match arg {
                Short('c') | Long("config") => cli.config = Some(PathBuf::from(parser.value()?)),
                Long("sock") => cli.sock = Some(PathBuf::from(parser.value()?)),
                Long("no-implicit-config") => cli.no_implicit_config = true,
                Long("check-config") => cli.check_config = true,
                Long("dump-config") => cli.dump_config = true,
                Short('h') | Long("help") => cli.help = true,
                Short('V') | Long("version") => cli.version = true,
                Value(value) => {
                    cli.command.push(value);
                    cli.command.extend(parser.raw_args()?);
                    break;
                }
                _ => return Err(arg.unexpected()),
            }
        }

        Ok(cli)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigPathError {
    #[error("no command was provided")]
    NoCommand,

    #[error("command {0:?} has no basename")]
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

pub fn config_path<'a>(cli: &'a Cli) -> Result<Cow<'a, Path>, ConfigPathError> {
    if let Some(path) = &cli.config {
        return Ok(Cow::Borrowed(path));
    }

    if cli.command.is_empty() {
        return Err(ConfigPathError::NoCommand);
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
        Ok(Cow::Owned(implicit_path))
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
