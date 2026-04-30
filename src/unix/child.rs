// SPDX-License-Identifier: MIT

use std::ffi::OsString;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use super::tty::{dup_fd, open_pty_pair, set_winsize, switch_to_ctty, PtyOpenError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSpec {
    Exec {
        argv: Vec<OsString>,
    },
    Shell {
        command: OsString,
    },
}

#[derive(Debug, Clone)]
pub struct ChildEnv {
    pub vars: Vec<(OsString, OsString)>,
}

impl Default for ChildEnv {
    fn default() -> Self {
        Self {
            vars: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ChildStdio {
    Null,
    Inherit,
    // TODO: Log, Pipe, etc.
}

#[derive(Debug, Clone)]
pub struct ChildSpawnOptions {
    pub env: ChildEnv,
    pub cwd: Option<PathBuf>,
    pub stdin: ChildStdio,
    pub stdout: ChildStdio,
    pub stderr: ChildStdio,
}

impl Default for ChildSpawnOptions {
    fn default() -> Self {
        Self {
            env: ChildEnv::default(),
            cwd: None,
            stdin: ChildStdio::Null,
            stdout: ChildStdio::Inherit,
            stderr: ChildStdio::Inherit,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PtyChildSpawnOptions {
    pub env: ChildEnv,
    pub cwd: Option<PathBuf>,
    pub window_size: Option<libc::winsize>,
}

impl Default for PtyChildSpawnOptions {
    fn default() -> Self {
        Self {
            env: ChildEnv::default(),
            cwd: None,
            window_size: None,
        }
    }
}

pub struct PtyChild {
    pub master: OwnedFd,
    pub child: Child,
}

#[derive(Debug, thiserror::Error)]
pub enum ChildError {
    #[error("empty argv")]
    EmptyArgv,

    #[error("empty program name")]
    EmptyProgram,

    #[error("empty shell command")]
    EmptyShellCommand,

    #[error("empty service name")]
    EmptyServiceName,

    #[error("pty open failed: {0}")]
    PtyOpen(#[from] PtyOpenError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub fn spawn(spec: &CommandSpec, opts: &ChildSpawnOptions) -> Result<Child, ChildError> {
    let mut cmd = make_command(spec)?;
    apply(&mut cmd, &opts.env, opts.cwd.as_ref());
    cmd.stdin(stdio(&opts.stdin));
    cmd.stdout(stdio(&opts.stdout));
    cmd.stderr(stdio(&opts.stderr));
    Ok(cmd.spawn()?)
}

pub fn spawn_pty_attached(spec: &CommandSpec, opts: &PtyChildSpawnOptions) -> Result<PtyChild, ChildError> {
    let pair = open_pty_pair()?;
    if let Some(ws) = opts.window_size {
        set_winsize(&pair.slave, &ws)?;
    }

    let slave_raw_fd = pair.slave.as_raw_fd();
    let stdin = dup_fd(&pair.slave)?;
    let stdout = dup_fd(&pair.slave)?;
    let stderr = dup_fd(&pair.slave)?;
    let mut cmd = make_command(spec)?;
    apply(&mut cmd, &opts.env, opts.cwd.as_ref());
    cmd.stdin(stdin);
    cmd.stdout(stdout);
    cmd.stderr(stderr);

    // SAFETY: pre_exec() closure must be async-signal-safe; switch_to_ctty() is safe
    unsafe {
        cmd.pre_exec(move || switch_to_ctty(slave_raw_fd));
    }

    let child = cmd.spawn()?;
    Ok(PtyChild { master: pair.master, child })
}

fn make_command(spec: &CommandSpec) -> Result<Command, ChildError> {
    match spec {
        CommandSpec::Exec { argv } => {
            let Some(program) = argv.first() else {
                return Err(ChildError::EmptyArgv);
            };
            if program.is_empty() {
                return Err(ChildError::EmptyProgram);
            }
            let mut cmd = Command::new(program);
            cmd.args(&argv[1..]);
            Ok(cmd)
        }
        CommandSpec::Shell { command } => {
            if command.is_empty() {
                return Err(ChildError::EmptyShellCommand);
            }
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c");
            cmd.arg(command);
            Ok(cmd)
        }
    }
}

fn apply(cmd: &mut Command, env: &ChildEnv, cwd: Option<&PathBuf>) {
    for (key, value) in &env.vars {
        cmd.env(key, value);
    }
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
}

fn stdio(spec: &ChildStdio) -> Stdio {
    match spec {
        ChildStdio::Null => Stdio::null(),
        ChildStdio::Inherit => Stdio::inherit(),
    }
}
