// SPDX-License-Identifier: MIT

mod config;
mod model;
mod runtime;
mod term;
mod unix;

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::time::Duration;

use config::loader::ConfigLoader;
use runtime::children::ServiceManager;
use runtime::cli::{config_path, Cli};
use term::mode::TerminalModeTracker;
use unix::child::{
    spawn_pty_attached, ChildEnv, ChildSpawnOptions, ChildStdio, OsCommandSpec, PtyChildSpawnOptions,
};
use unix::pledge::try_pledge;
use unix::signal::SignalRegistry;
use unix::sock::{default_sock_path, ControlSock};
use unix::tty::{get_winsize, RawTerminal};

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
    Route(#[from] crate::runtime::router::RouteError),
}

fn main() {
    // use a tmp var because otherwise it's `temporary value dropped while borrowed`
    let a0_binding = std::env::args_os().next();
    let argv0 = a0_binding.as_deref().map(OsStr::to_string_lossy).unwrap_or(Cow::Borrowed("yxt"));
    if let Err(e) = run(argv0.as_ref()) {
        eprintln!("{argv0}: {e}");
        std::process::exit(1);
    }
}

fn run(argv0: &str) -> Result<(), AppError> {
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
        return Ok(());
    }
    if cli.version {
        eprintln!("yxt v0.1.0-alpha");
        return Ok(());
    }
    if cli.command.is_empty() {
        eprint!("\
usage: {argv0} [options] command [args ...]
try '--help' for more info
");
        return Ok(());
    }

    let config_path = config_path(&cli)?;
    let config = ConfigLoader::new().parse_file(config_path.path.as_ref())?;

    if cli.check_config {
        return Ok(());
    }

    if cli.dump_config {
        println!("{config:#?}");
        return Ok(());
    }

    let sock_path = default_sock_path("yxt")?;
    let sock = ControlSock::bind(&sock_path, 8192)?;

    try_pledge("stdio rpath tty proc exec", None)?;

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
    let services = ServiceManager::start(&config.services, child_opts.clone(), Duration::from_millis(1000));

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let winsize = get_winsize(&stdin).ok();

    let child_spec = OsCommandSpec::Exec { argv: cli.command };
    let child = spawn_pty_attached(&child_spec, &PtyChildSpawnOptions {
        env: env.clone(),
        cwd: None,
        window_size: winsize,
    })?;

    let _term = RawTerminal::enter(&stdin)?;

    let mut signals = SignalRegistry::new()?;
    signals.register(libc::SIGINT)?;
    signals.register(libc::SIGTERM)?;
    signals.register(libc::SIGWINCH)?;
    // TODO: register signals from config for actions

    let tracker = TerminalModeTracker::new();

    todo!()
}
