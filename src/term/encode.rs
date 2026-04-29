// SPDX-License-Identifier: MIT

use crate::model::Token;

use super::{kitty, legacy, TermMode};

#[derive(Debug, Clone, Copy)]
pub struct Encoder {
    mode: TermMode,
}

impl Encoder {
    pub fn new(mode: TermMode) -> Self {
        Self {
            mode,
        }
    }

    pub fn encode_token(&self, token: &Token) -> Option<Vec<u8>> {
        if self.mode.kitty_flags != 0 && let Some(bytes) = kitty::encode_token(token, self.mode.kitty_flags) {
            Some(bytes)
        } else {
            legacy::encode_token(token, self.mode)
        }
    }
}
