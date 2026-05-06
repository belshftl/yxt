// SPDX-License-Identifier: MIT

#![allow(dead_code)]

mod config;
mod model;
mod runtime;
mod term;
mod unix;

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use crate::config::loader::ConfigLoader;
use crate::model::{Action, Event, Signal, Source};
use crate::runtime::children::{ActionManager, ServiceManager};
use crate::runtime::cli::{Cli, config_path};
use crate::runtime::io::{
    ByteQueue, ReadResult, ReadToQueueResult, WriteResult, WriteToPtyResult,
    read, read_pty_to_queue, drain_from_queue, drain_to_pty_from_queue,
};
use crate::runtime::router::{RouteEffect, RouteInput, Router};
use crate::term::decode::{Decoded, Decoder, DecoderConfig};
use crate::term::encode::Encoder;
use crate::term::mode::{TermMode, TerminalModeTracker};
use crate::unix::child::{
    ChildEnv, ChildExt, ChildSpawnOptions, ChildStdio, OsCommandSpec, PtyChild,
    PtyChildSpawnOptions, spawn_pty_attached,
};
use crate::unix::fd::{NonblockingFd, ReadyFds, SelectFds, select};
use crate::unix::pledge::try_pledge;
use crate::unix::signal::{SignalError, SignalRegistry};
use crate::unix::sock::{ControlSock, default_sock_path};
use crate::unix::tty::{RawTerminal, get_winsize, set_winsize};

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

    #[error("PTY input queue of {0} bytes is full; child is not consuming its input or mapping expanded too much, bailing out")]
    MasterQueueFull(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FdKey {
    Stdin,
    Sock,
    PtyMaster,
    Signals,
    Stdout
}

fn main() {
    // use a tmp var because otherwise it's `temporary value dropped while borrowed`
    let a0_binding = std::env::args_os().next();
    let argv0 = a0_binding.as_deref().map(OsStr::to_string_lossy).unwrap_or(Cow::Borrowed("yxt"));
    match run(argv0.as_ref()) {
        Ok(rv) => std::process::exit(rv),
        Err(e) => {
            eprintln!("{argv0}: {e}");
            std::process::exit(1);
        }
    }
}

fn run(argv0: &str) -> Result<i32, AppError> {
    try_pledge("stdio rpath wpath cpath unix tty proc exec", None)?;

    let cli = Cli::parse()?;
    if cli.help {
        eprint!("\
usage: {argv0} [options] command [args ...]
remap/inject/filter for terminal input based on config rules

options:
  -c, --config <PATH>       config file to use
      --sock <PATH>         path of the created socket (computes a unique one by default)
      --no-implicit-config  don't use an implicit config if found
      --check-config        parse config and exit
      --dump-config         parse config, print parse result, and exit
  -h, --help                display this help and exit
  -V, --version             output version information and exit
");
        return Ok(0);
    }
    if cli.version {
        eprintln!("yxt v0.1.0-alpha");
        return Ok(0);
    }
    if !cli.check_config && !cli.dump_config && cli.command.is_empty() {
        eprint!("\
usage: {argv0} [options] command [args ...]
try '--help' for more info
");
        return Ok(2);
    }

    let config_path = config_path(&cli)?;
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

    let sock_path = default_sock_path("yxt")?;
    let sock = ControlSock::bind(&sock_path, 8192)?;

    try_pledge("stdio rpath tty proc exec", None)?;

    const SHUTDOWN_GRACE: Duration = Duration::from_millis(300);
    let env = ChildEnv {
        vars: vec![
            (OsString::from("YXT_PID"), OsString::from(std::process::id().to_string())),
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
    let mut actions = ActionManager::new(child_opts.clone());
    let mut services = ServiceManager::start(&config.services, child_opts.clone(), SHUTDOWN_GRACE)?;

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let winsize = get_winsize(&stdin).ok();

    let child_spec = OsCommandSpec::Exec { argv: cli.command };
    let mut pty_child = spawn_pty_attached(&child_spec, &PtyChildSpawnOptions {
        env: env.clone(),
        cwd: None,
        window_size: winsize,
    })?;

    let _raw = RawTerminal::enter(&stdin)?;
    let _sock_nonblock = NonblockingFd::new(sock.as_fd())?;
    let _stdin_nonblock = NonblockingFd::new(stdin.as_fd())?;
    let _stdout_nonblock = NonblockingFd::new(stdin.as_fd())?;
    let _pty_nonblock = NonblockingFd::new(pty_child.pty_master.as_fd())?;

    let mut signals = SignalRegistry::new()?;
    signals.register(libc::SIGINT)?;
    signals.register(libc::SIGTERM)?;
    signals.register(libc::SIGWINCH)?;
    for src in config.mappings.iter().map(|m| &m.from) {
        if let Source::Event(Event::Signal(Signal(sig))) = src {
            if let Err(e) = signals.register(*sig) && !matches!(e, SignalError::AlreadyRegistered(_)) {
                return Err(AppError::Signal(e));
            }
        }
    }

    let mut decoder = Decoder::new(DecoderConfig {
        mode: TermMode::LEGACY,
        esc_byte_is_partial_esc: config.options.esc_byte_is_partial_esc,
        partial_utf8_timeout: Duration::from_millis(config.options.partial_utf8_timeout_ms as u64),
        partial_esc_timeout: Duration::from_millis(config.options.partial_esc_timeout_ms as u64),
        partial_st_timeout: Duration::from_millis(config.options.partial_st_timeout_ms as u64),
        max_pending_bytes: 4096, // TODO
    });
    let mut encoder = Encoder::new(TermMode::LEGACY);
    let mut tracker = TerminalModeTracker::new();
    let router = Router::new(&config);

    fn apply_effect(
        effect: &RouteEffect,
        encoder: &Encoder,
        master_queue: &mut ByteQueue,
        actions: &mut ActionManager,
    ) -> Result<(), AppError> {
        match effect {
            RouteEffect::Token(tok) => {
                if let Some(bytes) = encoder.encode_token(&tok) {
                    master_queue.push(&bytes).map_err(|_| AppError::MasterQueueFull(master_queue.capacity()))?;
                }
            }
            RouteEffect::Action(act) => {
                match act {
                    Action::Command(cmd) => actions.spawn(&cmd)?,
                }
            }
        }
        Ok(())
    }

    fn handle_decoded(
        decoded: &[Decoded],
        encoder: &Encoder,
        router: &Router,
        master_queue: &mut ByteQueue,
        actions: &mut ActionManager,
    ) -> Result<(), AppError> {
        for item in decoded {
            match item {
                Decoded::Token(tok) => {
                    let r = router.fire(RouteInput::Token(tok))?;
                    if !r.matched && let Some(bytes) = encoder.encode_token(&tok) {
                        master_queue.push(&bytes).map_err(|_| AppError::MasterQueueFull(master_queue.capacity()))?;
                    }
                    for effect in r.effects {
                        apply_effect(&effect, encoder, master_queue, actions)?;
                    }
                }
                Decoded::Unknown(bytes) =>
                    master_queue.push(&bytes).map_err(|_| AppError::MasterQueueFull(master_queue.capacity()))?,
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

    let mut stdin_buf = vec![0u8; 8192].into_boxed_slice();
    let mut master_queue = ByteQueue::new(32768);
    let mut stdout_queue = ByteQueue::new(8192);
    let mut mode_dirty = false;
    let mut stopping = false;
    let mut child_down_or_forgotten = false;
    let mut services_down = false;
    let mut child_kill_deadline = None;
    'mainloop: loop {
        let now = Instant::now();

        if !child_down_or_forgotten && pty_child.child.try_wait()?.is_some() {
            begin_shutdown(&mut stopping, &mut services, &pty_child, None, &mut child_kill_deadline, now)?;
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

            if !child_down_or_forgotten && let Some(d) = child_kill_deadline && now >= d {
                pty_child.child.kill()?;
                child_down_or_forgotten = true;
            }

            if services_down && child_down_or_forgotten {
                break 'mainloop;
            }
        }

        let timeout = [decoder.next_deadline(), services.next_deadline(), child_kill_deadline].
            into_iter().flatten().min().map(|d| d.saturating_duration_since(now));

        let ready = {
            let mut read = Vec::new();
            let mut write = Vec::new();

            read.push((FdKey::Signals, signals.as_fd()));
            // avoid using a terminal mode the real terminal isn't in yet
            if !stopping && !(mode_dirty && !stdout_queue.is_empty()) && master_queue.remaining() > 0 {
                read.push((FdKey::Stdin, stdin.as_fd()));
                read.push((FdKey::Sock, sock.as_fd()));
            }
            if stdout_queue.remaining() > 0 {
                read.push((FdKey::PtyMaster, pty_child.pty_master.as_fd()));
            }
            if !stdout_queue.is_empty() {
                write.push((FdKey::Stdout, stdout.as_fd()));
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

        if ready.writable(FdKey::Stdout) {
            while matches!(drain_from_queue(&stdout, &mut stdout_queue)?, WriteResult::Success(_)) {}
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
                        begin_shutdown(&mut stopping, &mut services, &pty_child, None, &mut child_kill_deadline, now)?;
                        child_down_or_forgotten = true;
                        continue 'mainloop;
                    }
                }
            }
        }

        if ready.readable(FdKey::PtyMaster) {
            match read_pty_to_queue(&pty_child.pty_master, &mut stdout_queue)? {
                ReadToQueueResult::Success { offset, len } => {
                    let new = &stdout_queue.pending()[offset..offset + len];
                    if tracker.observe_child_output(new) {
                        decoder.set_mode(tracker.mode());
                        encoder.set_mode(tracker.mode());
                        mode_dirty = true;
                    }
                }
                ReadToQueueResult::Eof => {
                    // child hung up, don't bother shutting it down, just quit
                    begin_shutdown(&mut stopping, &mut services, &pty_child, None, &mut child_kill_deadline, now)?;
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
                        begin_shutdown(&mut stopping, &mut services, &pty_child, Some(sig), &mut child_kill_deadline, now)?;
                        continue 'mainloop;
                    }
                    libc::SIGWINCH => {
                        let ws = get_winsize(&stdin)?;
                        set_winsize(&pty_child.pty_master, &ws)?;
                        let r = router.fire(RouteInput::Event(&Event::Signal(Signal(libc::SIGWINCH))))?;
                        for effect in r.effects {
                            apply_effect(&effect, &encoder, &mut master_queue, &mut actions)?;
                        }
                    }
                    other => {
                        let r = router.fire(RouteInput::Event(&Event::Signal(Signal(other))))?;
                        for effect in r.effects {
                            apply_effect(&effect, &encoder, &mut master_queue, &mut actions)?;
                        }
                    }
                }
            }
        }

        if ready.readable(FdKey::Stdin) {
            loop {
                match read(&stdin, &mut stdin_buf)? {
                    ReadResult::Success(n) => {
                        let mut decoded = Vec::new();
                        decoder.push(now, &stdin_buf[..n], &mut decoded);
                        handle_decoded(&decoded, &encoder, &router, &mut master_queue, &mut actions)?;
                    }
                    ReadResult::WouldBlock => break,
                    ReadResult::Eof => {
                        // actual terminal hung up, nothing useful to do anymore, quit
                        begin_shutdown(&mut stopping, &mut services, &pty_child, Some(libc::SIGTERM), &mut child_kill_deadline, now)?;
                        continue 'mainloop;
                    }
                    ReadResult::EmptyInput => panic!("unexpected EmptyInput read result"),
                }
            }
        }

        if ready.readable(FdKey::Sock) {
            while let Some(b) = sock.recv()? {
                let r = router.fire(RouteInput::Event(&Event::Sockdata(b)))?;
                for effect in r.effects {
                    apply_effect(&effect, &encoder, &mut master_queue, &mut actions)?;
                }
            }
        }

        // don't fire tokens if we have a pending mode to avoid using a terminal mode the real
        // terminal isn't in yet
        if !stopping && !(mode_dirty && !stdout_queue.is_empty()) && master_queue.remaining() > 0 {
            let mut decoded = Vec::new();
            decoder.flush_timed_out(now, &mut decoded);
            handle_decoded(&decoded, &encoder, &router, &mut master_queue, &mut actions)?;
        }
    }

    Ok(0)
}
