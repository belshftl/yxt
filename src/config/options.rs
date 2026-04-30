// SPDX-License-Identifier: MIT

use super::line::{Literal, Span};
use super::lower::{ConfigError, LiteralKind};

#[derive(Debug, Clone)]
pub struct Options {
    pub log_file: String,
    pub esc_byte_is_partial_esc: bool,
    pub partial_utf8_timeout_ms: i32,
    pub partial_esc_timeout_ms: i32,
    pub partial_st_timeout_ms: i32,
    pub max_pending_decoder_bytes: i32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            log_file: String::new(),
            esc_byte_is_partial_esc: false,
            partial_utf8_timeout_ms: 10,
            partial_esc_timeout_ms: 15,
            partial_st_timeout_ms: 40,
            max_pending_decoder_bytes: 4096,
        }
    }
}

impl Options {
    pub fn set(&mut self, name: String, value: Literal, span: Span) -> Result<(), ConfigError> {
        match name.as_str() {
            "log_file" => self.log_file = expect_string(value, span)?,
            "esc_byte_is_partial_esc" => self.esc_byte_is_partial_esc = expect_bool(value, span)?,
            "partial_utf8_timeout" => self.partial_utf8_timeout_ms = expect_int(value, span)?,
            "partial_esc_timeout" => self.partial_esc_timeout_ms = expect_int(value, span)?,
            "partial_st_timeout" => self.partial_st_timeout_ms = expect_int(value, span)?,
            "max_pending_decoder_bytes" => self.max_pending_decoder_bytes = expect_int(value, span)?,
            _ => return Err(ConfigError::UnknownOption { name, span }),
        }
        Ok(())
    }
}

fn expect_string(value: Literal, span: Span) -> Result<String, ConfigError> {
    match value {
        Literal::String(v) => Ok(v),
        other => Err(ConfigError::WrongLiteralType {
            expected: LiteralKind::String,
            got: LiteralKind::of(&other),
            span,
        }),
    }
}

fn expect_bool(value: Literal, span: Span) -> Result<bool, ConfigError> {
    match value {
        Literal::Bool(v) => Ok(v),
        other => Err(ConfigError::WrongLiteralType {
            expected: LiteralKind::Bool,
            got: LiteralKind::of(&other),
            span,
        }),
    }
}

fn expect_int(value: Literal, span: Span) -> Result<i32, ConfigError> {
    match value {
        Literal::Int(v) => Ok(v),
        other => Err(ConfigError::WrongLiteralType {
            expected: LiteralKind::Int,
            got: LiteralKind::of(&other),
            span,
        }),
    }
}
