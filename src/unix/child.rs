// SPDX-FileCopyrightText: 2026 belshftl
// SPDX-License-Identifier: MIT

use anyhow::{Context as _, bail};
use std::ffi::OsString;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use super::tty::{dup_fd, open_pty_pair, set_winsize, switch_to_ctty};

pub trait ChildExt {
    fn signal(&self, sig: libc::c_int) -> std::io::Result<()>;
}

impl ChildExt for Child {
    fn signal(&self, sig: libc::c_int) -> std::io::Result<()> {
        let pid = libc::pid_t::try_from(self.id()).expect("child PID should fit into libc::pid_t");
        // SAFETY: no pointer inputs, invalid `pid`/`sig` surfaces as a syscall error
        if unsafe { libc::kill(pid, sig) } < 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ESRCH) {
                return Err(e);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsCommandSpec {
    Exec { argv: Vec<OsString> },
    Shell { command: OsString },
}

impl OsCommandSpec {
    pub fn from_model(command: &crate::model::CommandSpec) -> Self {
        match command {
            crate::model::CommandSpec::Exec { argv } => Self::Exec {
                argv: argv.iter().map(OsString::from).collect(),
            },
            crate::model::CommandSpec::Shell { command } => Self::Shell {
                command: OsString::from(command),
            },
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChildEnv {
    pub vars: Vec<(OsString, OsString)>,
}

#[derive(Debug, Clone)]
pub enum ChildStdio {
    Null,
    Inherit,
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

#[derive(Debug, Clone, Copy)]
pub enum PtyChildStdin<'a> {
    Pty,                         // the pty slave, for terminal input routing
    Passthrough(BorrowedFd<'a>), // the given fd verbatim, with the parent not involved
}

#[derive(Debug, Clone)]
pub struct PtyChildSpawnOptions<'a> {
    pub env: ChildEnv,
    pub cwd: Option<PathBuf>,
    pub window_size: Option<libc::winsize>,
    pub stdin: PtyChildStdin<'a>,
}

#[derive(Debug)]
pub struct PtyChild {
    pub pty_master: OwnedFd,
    pub child: Child,
}

pub fn spawn(spec: &OsCommandSpec, opts: &ChildSpawnOptions) -> anyhow::Result<Child> {
    let mut cmd = make_command(spec)?;
    apply(&mut cmd, &opts.env, opts.cwd.as_ref());
    cmd.stdin(stdio(&opts.stdin));
    cmd.stdout(stdio(&opts.stdout));
    cmd.stderr(stdio(&opts.stderr));
    cmd.spawn().context("spawning the child process")
}

pub fn spawn_pty_attached(
    spec: &OsCommandSpec,
    opts: &PtyChildSpawnOptions<'_>,
) -> anyhow::Result<PtyChild> {
    let pair = open_pty_pair()?;
    if let Some(ws) = opts.window_size {
        set_winsize(&pair.slave, ws)?;
    }

    let slave_raw_fd = pair.slave.as_raw_fd();
    let stdin = match opts.stdin {
        PtyChildStdin::Pty => dup_fd(&pair.slave)?,
        PtyChildStdin::Passthrough(fd) => dup_fd(&fd)?,
    };
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

    let child = cmd.spawn().context("spawning the pty child process")?;
    Ok(PtyChild {
        pty_master: pair.master,
        child,
    })
}

fn make_command(spec: &OsCommandSpec) -> anyhow::Result<Command> {
    match spec {
        OsCommandSpec::Exec { argv } => {
            let Some(program) = argv.first() else {
                bail!("empty argv");
            };
            if program.is_empty() {
                bail!("empty program name");
            }
            let mut cmd = Command::new(program);
            cmd.args(&argv[1..]);
            Ok(cmd)
        }
        OsCommandSpec::Shell { command } => {
            if command.is_empty() {
                bail!("empty shell command");
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
