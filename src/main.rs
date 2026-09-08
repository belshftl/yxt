// SPDX-FileCopyrightText: 2026 belshftl
// SPDX-License-Identifier: MIT

#![warn(clippy::pedantic)]
#![forbid(unsafe_op_in_unsafe_fn)]
#![forbid(clippy::as_conversions)]
#![forbid(clippy::borrow_as_ptr)]
#![forbid(clippy::tests_outside_test_module)]
#![forbid(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::debug_assert_with_mut_call)]
#![warn(clippy::error_impl_error)]
#![warn(clippy::exit)]
#![warn(
    clippy::partial_pub_fields,
    reason = "private fields are usually extra state that'd need to somehow be kept in lockstep with the arbitrarily user modifiable public state"
)]
#![warn(clippy::str_to_string)]
#![warn(clippy::useless_let_if_seq)]
#![allow(clippy::option_option)]
#![allow(clippy::similar_names)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::too_many_lines)]

mod config;
mod model;
mod runtime;
mod term;
mod unix;

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::io::IsTerminal;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::config::loader::ConfigLoader;
use crate::model::{Action, Event, Signal, Source};
use crate::runtime::children::{ActionManager, ServiceManager};
use crate::runtime::cli::{Cli, config_path};
use crate::runtime::io::{
    ByteQueue, ReadResult, WriteResult, WriteToPtyResult, drain_from_queue,
    drain_to_pty_from_queue, read_tty, write_all_until,
};
use crate::runtime::router::{RouteEffect, RouteInput, Router};
use crate::term::decode::{Decoded, Decoder, DecoderConfig};
use crate::term::encode::Encoder;
use crate::term::negotiate::{self, RestoreGuard, TermProxy};
use crate::term::query::query_term_mode;
use crate::unix::child::{
    ChildEnv, ChildExt, ChildSpawnOptions, ChildStdio, OsCommandSpec, PtyChild,
    PtyChildSpawnOptions, PtyChildStdin, spawn_pty_attached,
};
use crate::unix::fd::{NonblockingFd, ReadyFds, SelectFds, is_rdwr, select};
use crate::unix::pledge::try_pledge;
use crate::unix::signal::{SignalError, SignalRegistry};
use crate::unix::sock::{ControlSock, default_sock_path};
use crate::unix::tty::{RawTerminal, get_winsize, same_terminal, set_winsize};

const SENSITIVE_CHILD_BASENAMES: &[&str] = &[
    // privilege/auth
    "sudo",
    "su",
    "doas",
    "pkexec",
    "login",
    "newgrp",
    "sg",
    // user/password
    "passwd",
    "chpasswd",
    "chsh",
    "chfn",
    "vipw",
    "vigr",
    "kpasswd",
    "htpasswd",
    // crypto/sign
    "gpg",
    "gpg2",
    "gpg-agent",
    "pinentry",
    "pinentry-curses",
    "pinentry-tty",
    "pinentry-gnome3",
    "pinentry-gtk-2",
    "pinentry-qt",
    "pinentry-mac",
    "ssh-keygen",
    "openssl",
    "age",
    "rage",
    "sq",
    "sequoia-sq",
    // password/secret manager
    "pass",
    "op",
    "bw",
    "rbw",
    "keepassxc-cli",
    "secret-tool",
    "security",
    "ykman",
    "ykchalresp",
    // remote sessions
    "ssh",
    "sftp",
    "scp",
    "mosh",
    "telnet",
    "rlogin",
    "rsh",
    "ftp",
    "lftp",
    // unlock
    "cryptsetup",
    "zfs",
];

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Pledge(#[from] crate::unix::pledge::PledgeError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Cli(#[from] lexopt::Error),

    #[error(transparent)]
    ConfigPath(#[from] crate::runtime::cli::ConfigPathError),

    #[error(transparent)]
    ConfigLoad(#[from] crate::config::loader::ConfigLoadError),

    #[error(
        "\
refusing to run for sensitive child '{0}'
this program internally tracks/routes terminal input, which you probably don't want here
run with --allow-sensitive-child to run anyways"
    )]
    SensitiveChild(String),

    #[error("stdout must be a terminal")]
    NotATerminal,

    #[error("stdout must be open read-write, since it's the terminal input is read from")]
    TerminalNotReadWrite,

    #[error("stdin is a terminal but not the same one as stdout")]
    NotOneTerminal,

    #[error(transparent)]
    ControlSock(#[from] crate::unix::sock::ControlSockError),

    #[error(transparent)]
    Child(#[from] crate::unix::child::ChildError),

    #[error(transparent)]
    PtyOpen(#[from] crate::unix::tty::PtyOpenError),

    #[error(transparent)]
    Signal(#[from] crate::unix::signal::SignalError),

    #[error(transparent)]
    Service(#[from] crate::runtime::children::ServiceError),

    #[error(transparent)]
    Route(#[from] crate::runtime::router::RouteError),

    #[error(transparent)]
    Negotiate(#[from] crate::term::negotiate::NegotiateError),

    #[error("timed out writing the keyboard protocol setup to the terminal")]
    NegotiateWriteTimeout,

    #[error(
        "PTY input queue of {0} bytes is full; child is not consuming its input or mapping expanded too much, bailing out"
    )]
    MasterQueueFull(usize),

    #[error("terminal output queue of {0} bytes is full, bailing out")]
    StdoutQueueFull(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FdKey {
    TermIn,
    Sock,
    PtyMaster,
    Signals,
    TermOut,
}

fn main() {
    // use a temporary binding as to not `temporary value dropped while borrowed`
    let a0_binding = std::env::args_os().next();
    let argv0 = a0_binding
        .as_deref()
        .map_or(Cow::Borrowed("yxt"), OsStr::to_string_lossy);
    match run(argv0.as_ref()) {
        Ok(rv) => std::process::exit(rv),
        Err(e) => {
            eprintln!("{argv0}: {e}");
            std::process::exit(1);
        }
    }
}

fn run(argv0: &str) -> Result<i32, AppError> {
    // --------------------------------------------------------------

    const SHUTDOWN_GRACE: Duration = Duration::from_millis(300);
    const NEGOTIATE_WRITE_TIMEOUT: Duration = Duration::from_millis(300);
    const PROXY_INJECT_HEADROOM: usize = 32; // at most CSI = flags u per chunk of child output

    fn apply_effect(
        effect: &RouteEffect,
        encoder: &Encoder,
        master_queue: &mut ByteQueue,
        actions: &mut ActionManager,
        mappings_enabled: &mut bool,
    ) -> Result<(), AppError> {
        match effect {
            RouteEffect::Token(tok) => {
                if let Some(bytes) = encoder.encode_token(tok) {
                    master_queue
                        .push(&bytes)
                        .map_err(|_| AppError::MasterQueueFull(master_queue.capacity()))?;
                }
            }
            RouteEffect::Action(act) => match act {
                Action::Command(cmd) => actions.spawn(cmd)?,
                Action::ToggleMappings(op) => *mappings_enabled = op.apply(*mappings_enabled),
            },
        }
        Ok(())
    }

    fn handle_decoded(
        decoded: &[Decoded],
        encoder: &Encoder,
        router: &Router,
        master_queue: &mut ByteQueue,
        actions: &mut ActionManager,
        mappings_enabled: &mut bool,
    ) -> Result<(), AppError> {
        for item in decoded {
            match item {
                Decoded::Token(tok) => {
                    let r = router.fire(RouteInput::Token(tok), *mappings_enabled)?;
                    if !r.matched
                        && let Some(bytes) = encoder.encode_token(tok)
                    {
                        master_queue
                            .push(&bytes)
                            .map_err(|_| AppError::MasterQueueFull(master_queue.capacity()))?;
                    }
                    for effect in r.effects {
                        apply_effect(&effect, encoder, master_queue, actions, mappings_enabled)?;
                    }
                }
                Decoded::Unknown(bytes) => master_queue
                    .push(bytes)
                    .map_err(|_| AppError::MasterQueueFull(master_queue.capacity()))?,
            }
        }
        Ok(())
    }

    fn begin_shutdown(
        stopping: &mut bool,
        services: &mut ServiceManager,
        pty_child: &PtyChild,
        sig: Option<libc::c_int>,
        child_kill_deadline: &mut Option<Instant>,
        now: Instant,
    ) -> Result<(), AppError> {
        if *stopping {
            return Ok(());
        }
        *stopping = true;
        services.begin_shutdown(now)?;
        if let Some(sig) = sig {
            pty_child.child.signal(sig).ok();
            *child_kill_deadline = Some(now + SHUTDOWN_GRACE);
        }
        Ok(())
    }

    // --------------------------------------------------------------

    try_pledge("stdio rpath wpath cpath unix tty proc exec", None)?;

    let cli = Cli::parse()?;
    if cli.help {
        eprint!("\
usage: {argv0} [options] command [args ...]
remap/inject/filter for terminal input based on config rules

options:
  -c, --config <PATH>          config file to use
      --sock <PATH>            path of the created socket (computes a unique one by default)
      --no-implicit-config     don't use an implicit config if found
      --allow-sensitive-child  allow running with children in a \"sensitive\" blocklist (e.g sudo, pinentry...)
      --check-config           parse config and exit
      --dump-config            parse config, print parse result, and exit
  -h, --help                   display this help and exit
  -V, --version                output version information and exit
");
        return Ok(0);
    }
    if cli.version {
        eprintln!("yxt v0.1.0-beta");
        return Ok(0);
    }
    if !cli.check_config && !cli.dump_config && cli.command.is_empty() {
        eprint!(
            "\
usage: {argv0} [options] command [args ...]
try '--help' for more info
"
        );
        return Ok(2);
    }

    let implicit_config_dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(xdg) => PathBuf::from(xdg),
        None => std::env::var_os("HOME")
            .map_or_else(|| PathBuf::from("."), PathBuf::from)
            .join(".config"),
    }
    .join("yxt")
    .join("implicit");
    std::fs::create_dir_all(&implicit_config_dir)?;
    let config_path = config_path(&cli, &implicit_config_dir)?;
    let mut loader = ConfigLoader::new();
    let config = match loader.parse_file(config_path.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            loader.report_err(e);
            return Ok(2);
        }
    };

    if cli.check_config {
        return Ok(0);
    }

    if cli.dump_config {
        println!("{config:#?}");
        return Ok(0);
    }

    if !cli.allow_sensitive_child
        && let Some(child_name) = cli.command[0].to_str()
        && SENSITIVE_CHILD_BASENAMES.contains(&child_name)
    {
        return Err(AppError::SensitiveChild(child_name.to_owned()));
    }

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();

    // stdout is just the terminal; it's what gets put into raw mode, queried for capabilities, and
    // read/written into
    // stdin is only for the child's own input and gets passed through as is if it's not a tty
    // no need to check if they're open, as if an fd 0-2 is not open before main() runs the runtime
    // opens /dev/null into it
    if !stdout.is_terminal() {
        return Err(AppError::NotATerminal);
    }
    if !is_rdwr(&stdout)? {
        return Err(AppError::TerminalNotReadWrite);
    }
    let stdin_is_terminal = stdin.is_terminal();
    if stdin_is_terminal && !same_terminal(&stdin, &stdout)? {
        return Err(AppError::NotOneTerminal);
    }

    let sock_path = cli.sock.map_or_else(|| default_sock_path("yxt"), Ok)?;
    let sock = ControlSock::bind(&sock_path, 8192)?;

    try_pledge("stdio rpath tty proc exec", None)?;

    let env = ChildEnv {
        vars: vec![
            (
                OsString::from("YXT_PID"),
                OsString::from(std::process::id().to_string()),
            ),
            (OsString::from("YXT_SOCK"), sock_path.as_os_str().to_owned()),
        ],
    };
    let child_opts = ChildSpawnOptions {
        env: env.clone(),
        cwd: None,
        stdin: ChildStdio::Null,
        stdout: ChildStdio::Null,
        stderr: ChildStdio::Null,
    };
    let winsize = get_winsize(&stdout).ok();

    let _raw = RawTerminal::enter(&stdout)?;
    let _term_nonblock = NonblockingFd::new(stdout.as_fd())?;

    let queried = query_term_mode(
        &stdout,
        Duration::from_millis(config.options.mode_query_timeout_ms),
    )?;
    let negotiation = negotiate::plan(&config, queried)?;
    let mut proxy = TermProxy::new(queried, negotiation);
    let _restore = if let Some(enable) = proxy.enable_sequence() {
        let guard = RestoreGuard::new(stdout.as_fd());
        if !write_all_until(
            stdout.as_fd(),
            &enable,
            Instant::now() + NEGOTIATE_WRITE_TIMEOUT,
        )? {
            return Err(AppError::NegotiateWriteTimeout);
        }
        Some(guard)
    } else {
        None
    };

    let mut actions = ActionManager::new(child_opts.clone());
    let mut services = ServiceManager::start(&config.services, &child_opts, SHUTDOWN_GRACE)?;

    let child_spec = OsCommandSpec::Exec { argv: cli.command };
    let mut pty_child = spawn_pty_attached(
        &child_spec,
        &PtyChildSpawnOptions {
            env: env.clone(),
            cwd: None,
            window_size: winsize,
            stdin: if stdin_is_terminal {
                PtyChildStdin::Pty
            } else {
                PtyChildStdin::Passthrough(stdin.as_fd())
            },
        },
    )?;

    let _sock_nonblock = NonblockingFd::new(sock.as_fd())?;
    let _pty_nonblock = NonblockingFd::new(pty_child.pty_master.as_fd())?;

    let mut signals = SignalRegistry::new()?;
    signals.register(libc::SIGINT)?;
    signals.register(libc::SIGTERM)?;
    signals.register(libc::SIGWINCH)?;
    for src in config.mappings.iter().map(|m| &m.from) {
        if let Source::Event(Event::Signal(Signal(sig))) = src
            && let Err(e) = signals.register(*sig)
            && !matches!(e, SignalError::AlreadyRegistered(_))
        {
            return Err(AppError::Signal(e));
        }
    }

    let mut decoder = Decoder::new(DecoderConfig {
        mode: proxy.upstream_mode(),
        esc_byte_is_partial_esc: config.options.esc_byte_is_partial_esc,
        partial_utf8_timeout: Duration::from_millis(config.options.partial_utf8_timeout_ms),
        partial_esc_timeout: Duration::from_millis(config.options.partial_esc_timeout_ms),
        partial_st_timeout: Duration::from_millis(config.options.partial_st_timeout_ms),
        max_pending_bytes: config.options.max_pending_decoder_bytes,
    });
    let mut encoder = Encoder::new(proxy.downstream_mode());
    let router = Router::new(&config);

    let mut term_buf = vec![0u8; 8192].into_boxed_slice();
    let mut pty_buf = vec![0u8; 8192].into_boxed_slice();
    let mut to_terminal = Vec::new();
    let mut to_child = Vec::new();
    let mut master_queue = ByteQueue::new(32768);
    let mut stdout_queue = ByteQueue::new(8192);
    let mut mode_dirty = false;
    let mut mappings_enabled = true;
    let mut stopping = false;
    let mut child_down_or_forgotten = false;
    let mut services_down = false;
    let mut child_kill_deadline = None;
    'mainloop: loop {
        let now = Instant::now();

        if !child_down_or_forgotten && pty_child.child.try_wait()?.is_some() {
            begin_shutdown(
                &mut stopping,
                &mut services,
                &pty_child,
                None,
                &mut child_kill_deadline,
                now,
            )?;
            child_down_or_forgotten = true;
        }
        if !stopping {
            services.check_exits()?;
        }
        actions.reap();

        if stopping {
            if !services_down {
                services.poll_shutdown(now)?;
                if services.is_shutdown_complete() {
                    services_down = true;
                }
            }

            if !child_down_or_forgotten
                && let Some(d) = child_kill_deadline
                && now >= d
            {
                pty_child.child.kill()?;
                child_down_or_forgotten = true;
            }

            if services_down && child_down_or_forgotten {
                break 'mainloop;
            }
        }

        let timeout = [
            decoder.next_deadline(),
            services.next_deadline(),
            child_kill_deadline,
        ]
        .into_iter()
        .flatten()
        .min()
        .map(|d| d.saturating_duration_since(now));

        let ready = {
            let mut read = Vec::new();
            let mut write = Vec::new();

            read.push((FdKey::Signals, signals.as_fd()));
            // avoid using a terminal mode the real terminal isn't in yet
            if (stdout_queue.is_empty() || !mode_dirty) && master_queue.remaining() > 0 && !stopping
            {
                read.push((FdKey::TermIn, stdout.as_fd()));
                read.push((FdKey::Sock, sock.as_fd()));
            }
            if stdout_queue.remaining() > PROXY_INJECT_HEADROOM {
                read.push((FdKey::PtyMaster, pty_child.pty_master.as_fd()));
            }
            if !stdout_queue.is_empty() {
                write.push((FdKey::TermOut, stdout.as_fd()));
            }
            if !master_queue.is_empty() {
                write.push((FdKey::PtyMaster, pty_child.pty_master.as_fd()));
            }

            let fds = SelectFds { read, write };
            match select(&fds, timeout) {
                Ok(ready) => ready,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => ReadyFds::empty(),
                Err(e) => return Err(AppError::Io(e)),
            }
        };

        let now = Instant::now();

        if ready.writable(FdKey::TermOut) {
            while matches!(
                drain_from_queue(&stdout, &mut stdout_queue)?,
                WriteResult::Success(_)
            ) {}
            if stdout_queue.is_empty() {
                mode_dirty = false;
            }
        }

        if ready.writable(FdKey::PtyMaster) {
            loop {
                match drain_to_pty_from_queue(&pty_child.pty_master, &mut master_queue)? {
                    WriteToPtyResult::Success(_) => {}
                    WriteToPtyResult::WouldBlock | WriteToPtyResult::EmptyInput => break,
                    WriteToPtyResult::Hangup => {
                        // child hung up, don't bother shutting it down, just quit
                        begin_shutdown(
                            &mut stopping,
                            &mut services,
                            &pty_child,
                            None,
                            &mut child_kill_deadline,
                            now,
                        )?;
                        child_down_or_forgotten = true;
                        continue 'mainloop;
                    }
                }
            }
        }

        if ready.readable(FdKey::PtyMaster) {
            let room = stdout_queue
                .remaining()
                .saturating_sub(PROXY_INJECT_HEADROOM)
                .min(pty_buf.len());
            debug_assert!(
                room > 0,
                "pty master should only be polled when there's room"
            );

            match read_tty(&pty_child.pty_master, &mut pty_buf[..room])? {
                ReadResult::Success(n) => {
                    to_terminal.clear();
                    to_child.clear();
                    let outcome = proxy.push(&pty_buf[..n], &mut to_terminal, &mut to_child);

                    stdout_queue
                        .push(&to_terminal)
                        .map_err(|_| AppError::StdoutQueueFull(stdout_queue.capacity()))?;
                    master_queue
                        .push(&to_child)
                        .map_err(|_| AppError::MasterQueueFull(master_queue.capacity()))?;

                    if outcome.upstream_changed {
                        decoder.set_mode(proxy.upstream_mode());
                        mode_dirty = true;
                    }
                    if outcome.downstream_changed {
                        encoder.set_mode(proxy.downstream_mode());
                    }
                }
                ReadResult::Eof => {
                    // child hung up, don't bother shutting it down, just quit
                    begin_shutdown(
                        &mut stopping,
                        &mut services,
                        &pty_child,
                        None,
                        &mut child_kill_deadline,
                        now,
                    )?;
                    child_down_or_forgotten = true;
                    continue 'mainloop;
                }
                _ => {}
            }
        }

        if ready.readable(FdKey::Signals) {
            for sig in signals.drain()? {
                match sig {
                    libc::SIGINT | libc::SIGTERM => {
                        // propagate signal to child and quit
                        begin_shutdown(
                            &mut stopping,
                            &mut services,
                            &pty_child,
                            Some(sig),
                            &mut child_kill_deadline,
                            now,
                        )?;
                        continue 'mainloop;
                    }
                    libc::SIGWINCH => {
                        let ws = get_winsize(&stdout)?;
                        set_winsize(&pty_child.pty_master, ws)?;
                        let r = router.fire(
                            RouteInput::Event(&Event::Signal(Signal(libc::SIGWINCH))),
                            mappings_enabled,
                        )?;
                        for effect in r.effects {
                            apply_effect(
                                &effect,
                                &encoder,
                                &mut master_queue,
                                &mut actions,
                                &mut mappings_enabled,
                            )?;
                        }
                    }
                    other => {
                        let r = router.fire(
                            RouteInput::Event(&Event::Signal(Signal(other))),
                            mappings_enabled,
                        )?;
                        for effect in r.effects {
                            apply_effect(
                                &effect,
                                &encoder,
                                &mut master_queue,
                                &mut actions,
                                &mut mappings_enabled,
                            )?;
                        }
                    }
                }
            }
        }

        if ready.readable(FdKey::TermIn) {
            loop {
                match read_tty(&stdout, &mut term_buf)? {
                    ReadResult::Success(n) => {
                        let mut decoded = Vec::new();
                        decoder.push(now, &term_buf[..n], &mut decoded);
                        handle_decoded(
                            &decoded,
                            &encoder,
                            &router,
                            &mut master_queue,
                            &mut actions,
                            &mut mappings_enabled,
                        )?;
                    }
                    ReadResult::WouldBlock => break,
                    ReadResult::Eof => {
                        // actual terminal hung up, nothing useful to do anymore, quit
                        begin_shutdown(
                            &mut stopping,
                            &mut services,
                            &pty_child,
                            Some(libc::SIGTERM),
                            &mut child_kill_deadline,
                            now,
                        )?;
                        continue 'mainloop;
                    }
                    ReadResult::EmptyInput => unreachable!(),
                }
            }
        }

        if ready.readable(FdKey::Sock) {
            while let Some(b) = sock.recv()? {
                let r = router.fire(RouteInput::Event(&Event::Sockdata(b)), mappings_enabled)?;
                for effect in r.effects {
                    apply_effect(
                        &effect,
                        &encoder,
                        &mut master_queue,
                        &mut actions,
                        &mut mappings_enabled,
                    )?;
                }
            }
        }

        // don't fire tokens if we have a pending mode to avoid using a terminal mode the real
        // terminal isn't in yet
        if (stdout_queue.is_empty() || !mode_dirty) && master_queue.remaining() > 0 && !stopping {
            let mut decoded = Vec::new();
            decoder.flush_timed_out(now, &mut decoded);
            handle_decoded(
                &decoded,
                &encoder,
                &router,
                &mut master_queue,
                &mut actions,
                &mut mappings_enabled,
            )?;
        }
    }

    Ok(0)
}
