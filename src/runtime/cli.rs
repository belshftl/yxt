// SPDX-FileCopyrightText: 2026 belshftl
// SPDX-License-Identifier: MIT

use anyhow::{Context as _, bail};
use lexopt::prelude::*;
use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Cli {
    pub config: Option<PathBuf>,
    pub sock: Option<PathBuf>,
    pub no_implicit_config: bool,
    pub high_latency: bool,
    pub allow_sensitive_child: bool,
    pub check_config: bool,
    pub dump_config: bool,
    pub help: bool,
    pub version: bool,
    pub command: Vec<OsString>,
}

impl Cli {
    pub fn parse() -> anyhow::Result<Self> {
        let mut parser = lexopt::Parser::from_env();
        let mut cli = Self {
            config: None,
            sock: None,
            no_implicit_config: false,
            high_latency: false,
            allow_sensitive_child: false,
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
                Short('L') | Long("high-latency") => cli.high_latency = true,
                Long("allow-sensitive-child") => cli.allow_sensitive_child = true,
                Long("check-config") => cli.check_config = true,
                Long("dump-config") => cli.dump_config = true,
                Short('h') | Long("help") => cli.help = true,
                Short('V') | Long("version") => cli.version = true,
                Value(value) => {
                    cli.command.push(value);
                    cli.command.extend(parser.raw_args()?);
                    break;
                }
                _ => return Err(arg.unexpected().into()),
            }
        }

        Ok(cli)
    }
}

pub fn config_path<'a>(cli: &'a Cli, implicit_config_dir: &Path) -> anyhow::Result<Cow<'a, Path>> {
    if let Some(path) = &cli.config {
        return Ok(Cow::Borrowed(path));
    }

    if cli.command.is_empty() {
        bail!("no command was provided");
    }
    let basename = command_basename(&cli.command[0])?;
    let implicit_path = implicit_config_dir.join(basename).with_extension("conf");

    let shown = implicit_path.display();
    if cli.no_implicit_config {
        bail!(
            "no config path was provided (implicit config lookup would use '{shown}' if enabled)"
        )
    } else if refuse_implicit_config() {
        bail!(
            "refusing to use implicit config '{shown}' under UID 0 / setuid / setgid; pass --config explicitly"
        )
    } else if !implicit_path.exists() {
        bail!("no config path was provided and implicit config '{shown}' does not exist")
    }

    Ok(Cow::Owned(implicit_path))
}

fn command_basename(command: &OsStr) -> anyhow::Result<&OsStr> {
    Path::new(command)
        .file_name()
        .filter(|name| !name.is_empty())
        .with_context(|| format!("command '{}' has no basename", command.display()))
}

fn refuse_implicit_config() -> bool {
    // SAFETY: all four calls take no inputs and have no rust-side safety requirements
    unsafe {
        let ruid = libc::getuid();
        let euid = libc::geteuid();
        let rgid = libc::getgid();
        let egid = libc::getegid();
        ruid == 0 || euid == 0 || ruid != euid || rgid != egid
    }
}
