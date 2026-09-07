// SPDX-FileCopyrightText: 2026 belshftl
// SPDX-License-Identifier: MIT

use super::{
    control::{self, CsiSeq},
    mode::TermMode,
};
use crate::model::{Direction, Key, KeyEventKind, KeypadKey, Mods, Token};

pub fn decode_c0(byte: u8) -> Option<Token> {
    // the name is slightly inaccurate, this also checks DEL and c < 0x20
    match byte {
        b'\0' => Some(Token::press_utf8(' ', Mods::CTRL)),
        b'\t' => Some(Token::press_key(Key::Tab, Mods::EMPTY)),
        b'\n' | b'\r' => Some(Token::press_key(Key::Enter, Mods::EMPTY)),
        0x1b => Some(Token::press_key(Key::Esc, Mods::EMPTY)),
        0x08 | 0x7f => Some(Token::press_key(Key::Backspace, Mods::EMPTY)),
        1..=31 => {
            let table = b"abcdefghijklmnopqrstuvwxyz[\\]^_";
            let ch = char::from(table[usize::from(byte - 1)]);
            Some(Token::press_utf8(ch, Mods::CTRL))
        }
        _ => None,
    }
}

pub fn decode_ss3(body: &[u8], mode: TermMode) -> Option<Token> {
    let [c] = *body else {
        return None;
    };
    let key = match c {
        b'A' => Key::Arrow(Direction::Up),
        b'B' => Key::Arrow(Direction::Down),
        b'C' => Key::Arrow(Direction::Right),
        b'D' => Key::Arrow(Direction::Left),
        b'H' => Key::Home,
        b'F' => Key::End,

        b'P' => Key::Function(1),
        b'Q' => Key::Function(2),
        b'R' => Key::Function(3),
        b'S' => Key::Function(4),

        _ if mode.deckpam => match c {
            b'p' => Key::Keypad(KeypadKey::Digit(0)),
            b'q' => Key::Keypad(KeypadKey::Digit(1)),
            b'r' => Key::Keypad(KeypadKey::Digit(2)),
            b's' => Key::Keypad(KeypadKey::Digit(3)),
            b't' => Key::Keypad(KeypadKey::Digit(4)),
            b'u' => Key::Keypad(KeypadKey::Digit(5)),
            b'v' => Key::Keypad(KeypadKey::Digit(6)),
            b'w' => Key::Keypad(KeypadKey::Digit(7)),
            b'x' => Key::Keypad(KeypadKey::Digit(8)),
            b'y' => Key::Keypad(KeypadKey::Digit(9)),

            b'n' => Key::Keypad(KeypadKey::Decimal),
            b'o' => Key::Keypad(KeypadKey::Divide),
            b'j' => Key::Keypad(KeypadKey::Multiply),
            b'm' => Key::Keypad(KeypadKey::Subtract),
            b'k' => Key::Keypad(KeypadKey::Add),
            b'M' => Key::Keypad(KeypadKey::Enter),
            b'X' => Key::Keypad(KeypadKey::Equal),
            b'l' => Key::Keypad(KeypadKey::Separator),

            _ => return None,
        },

        _ => return None,
    };

    Some(Token::press_key(key, Mods::EMPTY))
}

pub fn decode_csi(csi: CsiSeq<'_>, mode: TermMode) -> Option<Token> {
    if !csi.intermediates.is_empty() {
        return None;
    }

    let params = control::parse_simple_params(csi.params)?;
    let n = params.first().copied().unwrap_or(0);
    let m = params.get(1).copied().unwrap_or(0);
    let param_count = params.len();

    // https://invisible-island.net/xterm/ctlseqs/ctlseqs.html
    // PC-Style Function Keys
    //
    // it's a table there but it's basically a bitfield with 1 added
    let mods = if param_count > 1 && m > 1 {
        // do nothing on 0 or 1 (1-1 = 0)
        let bits = m - 1; // remove the aforementioned 1
        if bits & !0b1111 != 0 {
            return None; // malformed input
        }

        let mut mods = Mods::EMPTY;
        if bits & 1 != 0 {
            mods |= Mods::SHIFT;
        }
        if bits & 2 != 0 {
            mods |= Mods::ALT;
        }
        if bits & 4 != 0 {
            mods |= Mods::CTRL;
        }
        if bits & 8 != 0 {
            mods |= Mods::META;
        }
        mods
    } else {
        Mods::EMPTY
    };

    // vt-style sequences (CSI N ~)
    if csi.final_byte == b'~' {
        if param_count < 1 || (param_count > 1 && m == 0) {
            return None;
        }
        let key = match n {
            2 => Key::Insert,
            3 => Key::Delete,
            5 => Key::PageUp,
            6 => Key::PageDown,

            // https://github.com/mobile-shell/mosh/issues/178
            1 | 7 => Key::Home,
            4 | 8 => Key::End,

            11 => Key::Function(1),
            12 => Key::Function(2),
            13 => Key::Function(3),
            14 => Key::Function(4),
            15 => Key::Function(5),
            17 => Key::Function(6),
            18 => Key::Function(7),
            19 => Key::Function(8),
            20 => Key::Function(9),
            21 => Key::Function(10),
            23 => Key::Function(11),
            24 => Key::Function(12),

            _ => return None,
        };

        return Some(Token::press_key(key, mods));
    }

    // xterm-style sequences (CSI <X>)
    if param_count > 0 && n != 1 {
        return None;
    }
    let key = match csi.final_byte {
        b'A' => Key::Arrow(Direction::Up),
        b'B' => Key::Arrow(Direction::Down),
        b'C' => Key::Arrow(Direction::Right),
        b'D' => Key::Arrow(Direction::Left),
        b'H' => Key::Home,
        b'F' => Key::End,

        b'P' => Key::Function(1),
        b'Q' => Key::Function(2),
        b'R' => Key::Function(3),
        b'S' => Key::Function(4),

        b'E' if mode.deckpam || mode.kitty_flags != 0 => Key::Keypad(KeypadKey::Begin),

        _ => return None,
    };

    Some(Token::press_key(key, mods))
}

pub fn encode_token(token: &Token, mode: TermMode) -> Option<Vec<u8>> {
    if token.kind() != KeyEventKind::Press {
        return None;
    }
    match token {
        Token::Utf8 { ch, mods, .. } => encode_utf8(*ch, *mods),
        Token::Key { key, mods, .. } => encode_key(*key, *mods, mode),
    }
}

fn encode_utf8(ch: char, mods: Mods) -> Option<Vec<u8>> {
    // the shiftedness is already baked into the character and legacy has no way to report it
    // separately so shift is meaningless here
    let mods = mods & !Mods::SHIFT;

    if mods == Mods::EMPTY {
        let mut out = [0u8; 4];
        let s = ch.encode_utf8(&mut out);
        return Some(s.as_bytes().to_vec());
    }

    if mods == Mods::CTRL {
        let mut c = ch;
        if c.is_ascii_lowercase() {
            c = c.to_ascii_uppercase();
        }
        if ('@'..='_').contains(&c) {
            let Ok(byte) = u8::try_from(c) else {
                unreachable!()
            };
            return Some(vec![byte - b'@']);
        }
    }

    if mods == Mods::ALT {
        let mut out = vec![0x1b];
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        return Some(out);
    }

    if mods == (Mods::ALT | Mods::CTRL) {
        let mut inner = encode_utf8(ch, Mods::CTRL)?;
        let mut out = vec![0x1b];
        out.append(&mut inner);
        return Some(out);
    }

    None
}

fn encode_key(key: Key, mods: Mods, mode: TermMode) -> Option<Vec<u8>> {
    let param = if (mods & !(Mods::SHIFT | Mods::ALT | Mods::CTRL | Mods::META)) == Mods::EMPTY {
        let mut param = 1u8;
        if (mods & Mods::SHIFT) != Mods::EMPTY {
            param += 1;
        }
        if (mods & Mods::ALT) != Mods::EMPTY {
            param += 2;
        }
        if (mods & Mods::CTRL) != Mods::EMPTY {
            param += 4;
        }
        if (mods & Mods::META) != Mods::EMPTY {
            param += 8;
        }
        param
    } else {
        return None;
    };

    match key {
        // c0 + DEL
        Key::Esc | Key::Enter | Key::Tab | Key::Backspace => encode_c0(key, mods),

        // keypad keys, in application (DECKPAM) or numeric mode
        Key::Keypad(kp) => encode_keypad(kp, param, mode),

        // vt-style sequences (CSI N ~)
        Key::Insert => Some(encode_vt(2, param)),
        Key::Delete => Some(encode_vt(3, param)),
        Key::PageUp => Some(encode_vt(5, param)),
        Key::PageDown => Some(encode_vt(6, param)),
        Key::Function(n @ 1..=12) => {
            let id = match n {
                1 => 11,
                2 => 12,
                3 => 13,
                4 => 14,
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => unreachable!(),
            };
            Some(encode_vt(id, param))
        }

        // cursor keys, either xterm-style (CSI <X>) or DECCKM (SS3 <X>)
        Key::Arrow(dir) => {
            let final_byte = match dir {
                Direction::Left => b'D',
                Direction::Right => b'C',
                Direction::Up => b'A',
                Direction::Down => b'B',
            };
            Some(encode_cursor(final_byte, param, mode))
        }
        Key::Home => Some(encode_cursor(b'H', param, mode)),
        Key::End => Some(encode_cursor(b'F', param, mode)),

        _ => None,
    }
}

fn encode_c0(key: Key, mods: Mods) -> Option<Vec<u8>> {
    let alt = (mods & Mods::ALT) != Mods::EMPTY;
    let rest = mods & !Mods::ALT;

    // backtab (shift + tab) is separately encoded as CSI Z
    if key == Key::Tab && !alt && rest == Mods::SHIFT {
        return Some(b"\x1b[Z".to_vec());
    }

    let base: &[u8] = match key {
        Key::Esc if rest == Mods::EMPTY => &[0x1b],
        // ctrl+enter is plain CR, like ctrl+m
        Key::Enter if rest == Mods::EMPTY || rest == Mods::CTRL => b"\r",
        Key::Tab if rest == Mods::EMPTY => b"\t",
        Key::Backspace if rest == Mods::EMPTY => &[0x7f],
        Key::Backspace if rest == Mods::CTRL => &[0x08],
        _ => return None,
    };

    if alt {
        let mut out = vec![0x1b];
        out.extend_from_slice(base);
        Some(out)
    } else {
        Some(base.to_vec())
    }
}

fn encode_keypad(kp: KeypadKey, param: u8, mode: TermMode) -> Option<Vec<u8>> {
    if mode.deckpam
        && let Some(bytes) = encode_deckpam(kp, param)
    {
        return Some(bytes);
    }

    // with numlock off keypad keys are reported as their corresponding edit/nav keys which carry
    // modifiers just like the non-keypad counterparts do
    match kp {
        KeypadKey::Left => return Some(encode_cursor(b'D', param, mode)),
        KeypadKey::Right => return Some(encode_cursor(b'C', param, mode)),
        KeypadKey::Up => return Some(encode_cursor(b'A', param, mode)),
        KeypadKey::Down => return Some(encode_cursor(b'B', param, mode)),
        KeypadKey::Home => return Some(encode_cursor(b'H', param, mode)),
        KeypadKey::End => return Some(encode_cursor(b'F', param, mode)),
        KeypadKey::PageUp => return Some(encode_vt(5, param)),
        KeypadKey::PageDown => return Some(encode_vt(6, param)),
        KeypadKey::Insert => return Some(encode_vt(2, param)),
        KeypadKey::Delete => return Some(encode_vt(3, param)),
        KeypadKey::Begin => return Some(encode_begin(param)),
        _ => {}
    }

    if param != 1 {
        return None;
    }
    let b = match kp {
        KeypadKey::Digit(n @ 0..=9) => b'0' + n,
        KeypadKey::Decimal => b'.',
        KeypadKey::Divide => b'/',
        KeypadKey::Multiply => b'*',
        KeypadKey::Subtract => b'-',
        KeypadKey::Add => b'+',
        KeypadKey::Enter => b'\r',
        KeypadKey::Equal => b'=',
        KeypadKey::Separator => b',',
        _ => return None,
    };
    Some(vec![b])
}

fn encode_begin(param: u8) -> Vec<u8> {
    if param == 1 {
        b"\x1b[E".to_vec()
    } else {
        format!("\x1b[1;{param}E").into_bytes()
    }
}

fn encode_deckpam(kp: KeypadKey, param: u8) -> Option<Vec<u8>> {
    if kp == KeypadKey::Begin {
        Some(encode_begin(param))
    } else {
        if param != 1 {
            return None;
        }

        let b = match kp {
            KeypadKey::Digit(0) => b'p',
            KeypadKey::Digit(1) => b'q',
            KeypadKey::Digit(2) => b'r',
            KeypadKey::Digit(3) => b's',
            KeypadKey::Digit(4) => b't',
            KeypadKey::Digit(5) => b'u',
            KeypadKey::Digit(6) => b'v',
            KeypadKey::Digit(7) => b'w',
            KeypadKey::Digit(8) => b'x',
            KeypadKey::Digit(9) => b'y',

            KeypadKey::Decimal => b'n',
            KeypadKey::Divide => b'o',
            KeypadKey::Multiply => b'j',
            KeypadKey::Subtract => b'm',
            KeypadKey::Add => b'k',
            KeypadKey::Enter => b'M',
            KeypadKey::Equal => b'X',
            KeypadKey::Separator => b'l',

            _ => return None,
        };
        Some(vec![0x1b, b'O', b])
    }
}

fn encode_vt(id: u8, param: u8) -> Vec<u8> {
    if param == 1 {
        format!("\x1b[{id}~").into_bytes()
    } else {
        format!("\x1b[{id};{param}~").into_bytes()
    }
}

fn encode_cursor(final_byte: u8, param: u8, mode: TermMode) -> Vec<u8> {
    if param == 1 {
        if mode.decckm {
            vec![0x1b, b'O', final_byte]
        } else {
            vec![0x1b, b'[', final_byte]
        }
    } else {
        format!("\x1b[1;{}{}", param, char::from(final_byte)).into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::{Direction, Key, KeypadKey, Mods, Token};
    use crate::term::mode::TermMode;

    fn mode(decckm: bool, deckpam: bool) -> TermMode {
        TermMode {
            decckm,
            deckpam,
            kitty_flags: 0,
        }
    }

    #[test]
    fn decodes_c0_control_letters() {
        assert_eq!(decode_c0(1), Some(Token::press_utf8('a', Mods::CTRL)),);
        assert_eq!(decode_c0(26), Some(Token::press_utf8('z', Mods::CTRL)),);
        assert_eq!(decode_c0(27), Some(Token::press_key(Key::Esc, Mods::EMPTY)),);
    }

    #[test]
    fn decodes_c0_common_keys() {
        assert_eq!(
            decode_c0(b'\t'),
            Some(Token::press_key(Key::Tab, Mods::EMPTY)),
        );
        assert_eq!(
            decode_c0(b'\r'),
            Some(Token::press_key(Key::Enter, Mods::EMPTY)),
        );
        assert_eq!(
            decode_c0(0x7f),
            Some(Token::press_key(Key::Backspace, Mods::EMPTY)),
        );
    }

    #[test]
    fn decodes_ss3_arrows_and_functions() {
        let m = mode(false, false);
        assert_eq!(
            decode_ss3(b"A", m),
            Some(Token::press_key(Key::Arrow(Direction::Up), Mods::EMPTY)),
        );
        assert_eq!(
            decode_ss3(b"D", m),
            Some(Token::press_key(Key::Arrow(Direction::Left), Mods::EMPTY)),
        );
        assert_eq!(
            decode_ss3(b"P", m),
            Some(Token::press_key(Key::Function(1), Mods::EMPTY)),
        );
        assert_eq!(
            decode_ss3(b"S", m),
            Some(Token::press_key(Key::Function(4), Mods::EMPTY)),
        );
    }

    #[test]
    fn ss3_keypad_requires_deckpam() {
        assert_eq!(decode_ss3(b"p", mode(false, false)), None);
        assert_eq!(
            decode_ss3(b"p", mode(false, true)),
            Some(Token::press_key(
                Key::Keypad(KeypadKey::Digit(0)),
                Mods::EMPTY
            )),
        );
        assert_eq!(
            decode_ss3(b"M", mode(false, true)),
            Some(Token::press_key(Key::Keypad(KeypadKey::Enter), Mods::EMPTY)),
        );
    }

    #[test]
    fn encodes_utf8_plain_and_legacy_mods() {
        assert_eq!(
            encode_token(&Token::press_utf8('x', Mods::EMPTY), mode(false, false),),
            Some(b"x".to_vec()),
        );
        assert_eq!(
            encode_token(&Token::press_utf8('x', Mods::CTRL), mode(false, false),),
            Some(vec![0x18]),
        );
        assert_eq!(
            encode_token(&Token::press_utf8('x', Mods::ALT), mode(false, false),),
            Some(b"\x1bx".to_vec()),
        );
    }

    #[test]
    fn encodes_arrows_decckm_dependent() {
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Arrow(Direction::Up), Mods::EMPTY),
                mode(false, false),
            ),
            Some(b"\x1b[A".to_vec()),
        );
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Arrow(Direction::Up), Mods::EMPTY),
                mode(true, false),
            ),
            Some(b"\x1bOA".to_vec()),
        );
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Arrow(Direction::Up), Mods::CTRL),
                mode(true, false),
            ),
            Some(b"\x1b[1;5A".to_vec()),
        );
    }

    #[test]
    fn encodes_vt_keys() {
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Insert, Mods::EMPTY),
                mode(false, false),
            ),
            Some(b"\x1b[2~".to_vec()),
        );
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Delete, Mods::CTRL),
                mode(false, false),
            ),
            Some(b"\x1b[3;5~".to_vec()),
        );
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Function(12), Mods::ALT),
                mode(false, false),
            ),
            Some(b"\x1b[24;3~".to_vec()),
        );
    }

    #[test]
    fn encodes_keypad_as_text_when_not_deckpam() {
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Keypad(KeypadKey::Digit(0)), Mods::EMPTY),
                mode(false, false),
            ),
            Some(b"0".to_vec()),
        );
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Keypad(KeypadKey::Add), Mods::EMPTY),
                mode(false, false),
            ),
            Some(b"+".to_vec()),
        );
        // a modified keypad key has no legacy text encoding
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Keypad(KeypadKey::Digit(0)), Mods::CTRL),
                mode(false, false),
            ),
            None,
        );
    }

    #[test]
    fn encodes_keypad_when_deckpam() {
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Keypad(KeypadKey::Digit(0)), Mods::EMPTY),
                mode(false, true),
            ),
            Some(b"\x1bOp".to_vec()),
        );
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Keypad(KeypadKey::Enter), Mods::EMPTY),
                mode(false, true),
            ),
            Some(b"\x1bOM".to_vec()),
        );
    }

    #[test]
    fn encodes_keypad_begin_in_both_keypad_modes() {
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Keypad(KeypadKey::Begin), Mods::EMPTY),
                mode(false, false),
            ),
            Some(b"\x1b[E".to_vec()),
        );
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Keypad(KeypadKey::Begin), Mods::EMPTY),
                mode(false, true),
            ),
            Some(b"\x1b[E".to_vec()),
        );
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Keypad(KeypadKey::Begin), Mods::CTRL),
                mode(false, true),
            ),
            Some(b"\x1b[1;5E".to_vec()),
        );
    }

    #[test]
    fn encodes_cursor_keypad_keys_like_their_normal_counterparts() {
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Keypad(KeypadKey::Left), Mods::EMPTY),
                mode(false, false),
            ),
            Some(b"\x1b[D".to_vec()),
        );
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Keypad(KeypadKey::Left), Mods::CTRL),
                mode(false, true),
            ),
            Some(b"\x1b[1;5D".to_vec()),
        );
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Keypad(KeypadKey::Delete), Mods::EMPTY),
                mode(false, true),
            ),
            Some(b"\x1b[3~".to_vec()),
        );
    }

    #[test]
    fn shift_is_implied_by_the_character_itself() {
        let m = mode(false, false);

        assert_eq!(
            encode_token(&Token::press_utf8('@', Mods::SHIFT), m),
            Some(b"@".to_vec()),
        );
        assert_eq!(
            encode_token(&Token::press_utf8('A', Mods::SHIFT), m),
            Some(b"A".to_vec()),
        );
        assert_eq!(
            encode_token(&Token::press_utf8('a', Mods::SHIFT | Mods::CTRL), m),
            Some(vec![0x01]),
        );
        assert_eq!(
            encode_token(&Token::press_utf8('a', Mods::SHIFT | Mods::ALT), m),
            Some(b"\x1ba".to_vec()),
        );
    }

    #[test]
    fn encodes_modified_c0_keys() {
        let m = mode(false, false);

        assert_eq!(
            encode_token(&Token::press_key(Key::Tab, Mods::SHIFT), m),
            Some(b"\x1b[Z".to_vec()),
        );
        assert_eq!(
            encode_token(&Token::press_key(Key::Tab, Mods::ALT), m),
            Some(b"\x1b\t".to_vec()),
        );
        assert_eq!(
            encode_token(&Token::press_key(Key::Esc, Mods::ALT), m),
            Some(b"\x1b\x1b".to_vec()),
        );
        assert_eq!(
            encode_token(&Token::press_key(Key::Enter, Mods::CTRL), m),
            Some(b"\r".to_vec()),
        );
        assert_eq!(
            encode_token(&Token::press_key(Key::Enter, Mods::ALT), m),
            Some(b"\x1b\r".to_vec()),
        );
        assert_eq!(
            encode_token(&Token::press_key(Key::Backspace, Mods::CTRL), m),
            Some(vec![0x08]),
        );
        assert_eq!(
            encode_token(&Token::press_key(Key::Backspace, Mods::ALT), m),
            Some(b"\x1b\x7f".to_vec()),
        );
    }

    #[test]
    fn drops_c0_keys_with_mods_legacy_cant_encode() {
        let m = mode(false, false);

        assert_eq!(
            encode_token(&Token::press_key(Key::Tab, Mods::CTRL), m),
            None,
        );
        assert_eq!(
            encode_token(&Token::press_key(Key::Esc, Mods::CTRL), m),
            None,
        );
        assert_eq!(
            encode_token(&Token::press_key(Key::Tab, Mods::SHIFT | Mods::ALT), m),
            None,
        );
    }

    #[test]
    fn legacy_rejects_mods_it_cannot_encode() {
        assert_eq!(
            encode_token(
                &Token::press_key(Key::Arrow(Direction::Up), Mods::SUPER),
                mode(false, false),
            ),
            None,
        );
    }
}
