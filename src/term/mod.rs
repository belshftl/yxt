// SPDX-License-Identifier: MIT

pub mod kitty;
pub mod legacy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermMode {
    pub decckm: bool,
    pub deckpam: bool,
    pub kitty_flags: u8,
}

impl TermMode {
    pub const LEGACY: Self = Self {
        decckm: false,
        deckpam: false,
        kitty_flags: 0,
    };
}
