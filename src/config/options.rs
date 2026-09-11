// SPDX-FileCopyrightText: 2026 belshftl
// SPDX-License-Identifier: MIT

use super::ast::{Literal, Span};
use super::lower::{ConfigError, ErrorKind, LiteralKind};
use crate::term::negotiate::PUSH_OVERHEAD_BYTES;

#[derive(Debug, Clone)]
pub struct Options {
    pub log_file: String,

    /// Asks the terminal to encode backspace as DEL rather than BS (DECBKM reset), which is the
    /// only thing under legacy that makes ctrl+h distinguishable from backspace. Off by default
    /// because treating BS as ctrl+h on a terminal that didn't agree to encode backspace as DEL
    /// breaks backspace instead.
    pub force_backspace_sends_del: bool,
    pub esc_byte_is_partial_esc: bool,

    pub mode_query_timeout_ms: u64,
    pub partial_utf8_timeout_ms: u64,
    pub partial_esc_timeout_ms: u64,
    pub partial_st_timeout_ms: u64,
    pub terminal_write_timeout_ms: u64,
    pub shutdown_grace_ms: u64,

    pub max_pending_decoder_bytes: usize,
    pub pty_input_queue_bytes: usize,
    pub terminal_output_queue_bytes: usize,
}

impl Default for Options {
    /// Tuned for a terminal emulator on the same machine, where a roundtrip is effectively free and
    /// a sequence almost never arrives split up.
    fn default() -> Self {
        Self {
            log_file: String::new(),

            force_backspace_sends_del: false,
            esc_byte_is_partial_esc: false,

            // roughly 3 frames at 60hz
            mode_query_timeout_ms: 50,
            partial_utf8_timeout_ms: 10,
            // matches neovim's `ttimeoutlen`; doesn't introduce any input delay for esc itself as
            // long as `esc_byte_is_partial_esc` is off
            partial_esc_timeout_ms: 50,
            // string controls are the long ones, so the likeliest to arrive in pieces
            partial_st_timeout_ms: 100,
            terminal_write_timeout_ms: 100,
            shutdown_grace_ms: 300,

            max_pending_decoder_bytes: 4096,
            pty_input_queue_bytes: 32768,
            terminal_output_queue_bytes: 8192,
        }
    }
}

impl Options {
    /// Tuned for a link with around 200-250ms of ping, e.g. a slow/distant ssh connection. Anything
    /// even worse than that should use a custom config. Only the timeouts that are affected by RTT
    /// increasing actually change.
    pub fn high_latency() -> Self {
        Self {
            // three round trips, for jitter. the whole budget is only ever spent when the
            // terminal answers nothing at all, since the DA1 sentinel ends it early otherwise
            mode_query_timeout_ms: 750,
            // one round trip, in case a retransmit lands in the middle of a sequence
            partial_utf8_timeout_ms: 250,
            partial_esc_timeout_ms: 300,
            partial_st_timeout_ms: 750,
            terminal_write_timeout_ms: 1000,
            ..Self::default()
        }
    }

    pub fn set(&mut self, name: String, value: Literal, span: Span) -> Result<(), ConfigError> {
        // currently, the only options with a legal minimum value are the ones where it's derivable
        // rather than just arbitrarily chosen to guard from bad input;
        // `terminal_write_timeout_ms = 0` is pathological but there isn't really a lower bound that
        // can be derived from some non-arbitrary rule/constant; 1 is the only other kind of
        // defensible one
        match name.as_str() {
            "log_file" => self.log_file = expect_string(value, span)?,
            "force_backspace_sends_del" => {
                self.force_backspace_sends_del = expect_bool(value, span)?;
            }
            "esc_byte_is_partial_esc" => self.esc_byte_is_partial_esc = expect_bool(value, span)?,
            "mode_query_timeout_ms" => {
                self.mode_query_timeout_ms = expect_non_negative(name, value, span)?;
            }
            "partial_utf8_timeout_ms" => {
                self.partial_utf8_timeout_ms = expect_non_negative(name, value, span)?;
            }
            "partial_esc_timeout_ms" => {
                self.partial_esc_timeout_ms = expect_non_negative(name, value, span)?;
            }
            "partial_st_timeout_ms" => {
                self.partial_st_timeout_ms = expect_non_negative(name, value, span)?;
            }
            "terminal_write_timeout_ms" => {
                self.terminal_write_timeout_ms = expect_non_negative(name, value, span)?;
            }
            "shutdown_grace_ms" => {
                self.shutdown_grace_ms = expect_non_negative(name, value, span)?;
            }
            "max_pending_decoder_bytes" => {
                self.max_pending_decoder_bytes = expect_non_negative(name, value, span)?;
            }
            "pty_input_queue_bytes" => {
                self.pty_input_queue_bytes =
                    expect_usize_above(name, value, span, 0, "a queue needs room for something")?;
            }
            "terminal_output_queue_bytes" => {
                self.terminal_output_queue_bytes = expect_usize_above(
                    name,
                    value,
                    span,
                    PUSH_OVERHEAD_BYTES,
                    "the child's output is only read while that much room is spare, so anything \
                     smaller never reads it at all",
                )?;
            }
            _ => {
                return Err(ConfigError {
                    kind: ErrorKind::UnknownOption { name },
                    span,
                });
            }
        }
        Ok(())
    }
}

fn expect_string(value: Literal, span: Span) -> Result<String, ConfigError> {
    match value {
        Literal::String(v) => Ok(v),
        other => Err(ConfigError {
            kind: ErrorKind::WrongLiteralType {
                expected: LiteralKind::String,
                got: LiteralKind::of(&other),
            },
            span,
        }),
    }
}

fn expect_bool(value: Literal, span: Span) -> Result<bool, ConfigError> {
    match value {
        Literal::Bool(v) => Ok(v),
        other => Err(ConfigError {
            kind: ErrorKind::WrongLiteralType {
                expected: LiteralKind::Bool,
                got: LiteralKind::of(&other),
            },
            span,
        }),
    }
}

fn expect_usize_above(
    name: String,
    value: Literal,
    span: Span,
    min: usize,
    why: &'static str,
) -> Result<usize, ConfigError> {
    let parsed = expect_non_negative::<usize>(name.clone(), value, span)?;
    if parsed <= min {
        return Err(ConfigError {
            kind: ErrorKind::OptionTooSmall { name, min, why },
            span,
        });
    }
    Ok(parsed)
}

fn expect_non_negative<T: Copy + TryFrom<i32>>(
    name: String,
    value: Literal,
    span: Span,
) -> Result<T, ConfigError> {
    match value {
        Literal::Int(v) => T::try_from(v).map_err(|_| ConfigError {
            kind: ErrorKind::BadOptionValue {
                name,
                desc: "value must be non-negative",
            },
            span,
        }),
        other => Err(ConfigError {
            kind: ErrorKind::WrongLiteralType {
                expected: LiteralKind::Int,
                got: LiteralKind::of(&other),
            },
            span,
        }),
    }
}
