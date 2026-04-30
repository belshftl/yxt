// SPDX-License-Identifier: MIT

mod config;
mod model;
mod runtime;
mod term;
mod unix;

use crate::runtime::cli::{config_path, Cli};

fn main() {
    let cli = Cli::parse().unwrap();
    let cfg = config_path(&cli).unwrap();
    println!("{}", cfg.path.as_ref().display());
    todo!();
}
