// SPDX-License-Identifier: MIT

mod config;
mod model;
mod runtime;
mod term;
mod unix;

use std::borrow::Cow;
use std::ffi::OsStr;

use crate::runtime::cli::{config_path, Cli};

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
    ConfigLoadError(#[from] crate::config::loader::ConfigLoadError),

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

    let cfg = config_path(&cli)?;
    println!("{}", cfg.path.as_ref().display());
    Ok(())
}
