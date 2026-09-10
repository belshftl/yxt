// SPDX-FileCopyrightText: 2026 belshftl
// SPDX-License-Identifier: MIT

//! xterm's `modifyOtherKeys`. Only decoding. The `CSI 27 ... ~` sequences are unambiguous (nothing
//! else uses parameter 27) so unconditionally decoding them is safe, same as with kitty's `CSI u`
//! sequences.

use super::control::{self, CsiSeq};
use super::legacy;
use crate::model::{Key, KeyEventKind, Token};

pub fn decode_csi_tilde(csi: CsiSeq<'_>) -> Option<Token> {
    if csi.final_byte != b'~' || !csi.intermediates.is_empty() || csi.private_marker().is_some() {
        return None;
    }

    let params = control::parse_simple_params(csi.params)?;
    let [27, mods, code] = params[..] else {
        return None;
    };
    let mods = legacy::decode_mod_param(mods)?;

    Some(match named_key_for_code(code) {
        Some(key) => Token::Key {
            key,
            mods,
            kind: KeyEventKind::Press,
        },
        None => Token::Utf8 {
            ch: char::from_u32(code)?,
            mods,
            kind: KeyEventKind::Press,
        },
    })
}

fn named_key_for_code(code: u32) -> Option<Key> {
    match code {
        0x08 | 0x7f => Some(Key::Backspace),
        0x09 => Some(Key::Tab),
        0x0a | 0x0d => Some(Key::Enter),
        0x1b => Some(Key::Esc),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::Mods;
    use crate::term::control::{CsiScan, scan_csi};

    fn decode(bytes: &[u8]) -> Option<Token> {
        let CsiScan::Complete { csi, consumed } = scan_csi(bytes, false) else {
            panic!("{bytes:?} is not a complete csi sequence");
        };
        assert_eq!(consumed, bytes.len());
        decode_csi_tilde(csi)
    }

    fn decoded(bytes: &[u8]) -> Token {
        decode(bytes).expect("{bytes:?} should have decoded")
    }

    // the byte sequences below were captured from real xterm at modifyOtherKeys=2
    #[test]
    fn decodes_the_reports_xterm_actually_sends() {
        let utf8 = Token::press_utf8;

        assert_eq!(decoded(b"\x1b[27;5;97~"), utf8('a', Mods::CTRL));
        assert_eq!(decoded(b"\x1b[27;5;104~"), utf8('h', Mods::CTRL));
        assert_eq!(decoded(b"\x1b[27;5;105~"), utf8('i', Mods::CTRL));
        assert_eq!(decoded(b"\x1b[27;5;109~"), utf8('m', Mods::CTRL));
        assert_eq!(decoded(b"\x1b[27;5;91~"), utf8('[', Mods::CTRL));
        assert_eq!(decoded(b"\x1b[27;5;49~"), utf8('1', Mods::CTRL));
        assert_eq!(decoded(b"\x1b[27;3;91~"), utf8('[', Mods::ALT));
        assert_eq!(decoded(b"\x1b[27;3;97~"), utf8('a', Mods::ALT));

        // the code has the shifted char and shift is also set in the bitfield
        assert_eq!(decoded(b"\x1b[27;2;65~"), utf8('A', Mods::SHIFT));
        assert_eq!(
            decoded(b"\x1b[27;6;65~"),
            utf8('A', Mods::SHIFT | Mods::CTRL)
        );
    }

    #[test]
    fn a_control_byte_code_is_the_corresponding_named_key() {
        // the base character of these keys is a control byte, so they are not text
        for (bytes, key) in [
            (b"\x1b[27;5;13~".as_slice(), Key::Enter),
            (b"\x1b[27;5;10~", Key::Enter),
            (b"\x1b[27;5;9~", Key::Tab),
            (b"\x1b[27;5;27~", Key::Esc),
            (b"\x1b[27;5;8~", Key::Backspace),
            (b"\x1b[27;5;127~", Key::Backspace),
        ] {
            assert_eq!(decoded(bytes), Token::press_key(key, Mods::CTRL));
        }
    }

    #[test]
    fn every_modifier_bit_round_trips() {
        let utf8 = Token::press_utf8;

        assert_eq!(decoded(b"\x1b[27;1;97~"), utf8('a', Mods::EMPTY));
        assert_eq!(decoded(b"\x1b[27;2;97~"), utf8('a', Mods::SHIFT));
        assert_eq!(decoded(b"\x1b[27;3;97~"), utf8('a', Mods::ALT));
        assert_eq!(decoded(b"\x1b[27;5;97~"), utf8('a', Mods::CTRL));
        assert_eq!(decoded(b"\x1b[27;9;97~"), utf8('a', Mods::META));
        assert_eq!(
            decoded(b"\x1b[27;16;97~"),
            utf8('a', Mods::SHIFT | Mods::ALT | Mods::CTRL | Mods::META),
        );
    }

    #[test]
    fn anything_that_is_not_this_report_is_left_alone() {
        // a vt-style key, which parameter 27 is never part of
        assert_eq!(decode(b"\x1b[5;5~"), None);
        assert_eq!(decode(b"\x1b[27~"), None);
        assert_eq!(decode(b"\x1b[27;5~"), None);
        // wrong final byte, and kitty's own report shape
        assert_eq!(decode(b"\x1b[27;5;97u"), None);
        // modifier bits above the four legacy ones are malformed here
        assert_eq!(decode(b"\x1b[27;0;97~"), None);
        assert_eq!(decode(b"\x1b[27;99;97~"), None);
        // intermediates and private markers are somebody else's sequence
        assert_eq!(decode(b"\x1b[27;5;97 ~"), None);
        assert_eq!(decode(b"\x1b[?27;5;97~"), None);
    }
}
