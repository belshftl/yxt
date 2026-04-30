// SPDX-License-Identifier: MIT

use crate::model::{Key, KeyEventKind, KeypadKey, MediaKey, ModifierKey, Mods, Token};
use super::control::{read_u32, CsiSeq};

pub const FLAG_REPORT_EVENT_TYPES: u8 = 0x02;
pub const FLAG_REPORT_ALL_KEYS: u8 = 0x08;
pub const FLAG_REPORT_ASSOCIATED_TEXT: u8 = 0x10;

pub fn decode_csi_u(csi: CsiSeq<'_>) -> Option<Vec<Token>> {
    if csi.final_byte != b'u' {
        return None;
    }
    if !csi.intermediates.is_empty() {
        return None;
    }
    decode_csi_u_params(csi.params)
}

pub fn decode_csi_u_params(params: &[u8]) -> Option<Vec<Token>> {
    // kitty csi keyboard events are of the format:
    //
    // CSI code [: alt1 [: alt2 ...]] [; mods [: kind]] [; text1 [: text2 ...]] u
    //
    // alt[0-9]+ and text[0-9]+ are single unicode codepoints
    let mut idx = 0;
    let code = read_u32(params, &mut idx, 0x10ffff)?;

    // skip `:alt`s if they're there
    loop {
        skip_ws(params, &mut idx);
        if params.get(idx) != Some(&b':') {
            break;
        }
        idx += 1;
        skip_ws(params, &mut idx);
        // validate and skip number
        _ = read_u32(params, &mut idx, 0x10ffff)?;
    }
    skip_ws(params, &mut idx);

    // parse ;mods if it's there
    let (mods, kind) = if params.get(idx) == Some(&b';') {
        idx += 1;
        parse_mods_and_type(params, &mut idx)?
    } else {
        (Mods::EMPTY, KeyEventKind::Press)
    };
    skip_ws(params, &mut idx);

    // parse `text`s if they're there
    if params.get(idx) == Some(&b';') {
        idx += 1;
        let text_tokens = parse_text_fields(params, &mut idx, mods, kind)?;
        if !text_tokens.is_empty() {
            return Some(text_tokens);
        }
    }
    skip_ws(params, &mut idx);

    if idx != params.len() {
        None
    } else {
        Some(vec![token_from_codepoint(code, mods, kind)])
    }
}

fn skip_ws(buf: &[u8], idx: &mut usize) {
    while *idx < buf.len() && matches!(buf[*idx], b' ' | b'\t') {
        *idx += 1;
    }
}

fn parse_mods_and_type(body: &[u8], idx: &mut usize) -> Option<(Mods, KeyEventKind)> {
    // this is also a bitfield with 1 added
    let m = read_u32(body, idx, u8::MAX as u32 + 1)?;
    let mods = if m > 0 {
        let bits = (m - 1) & !(Mods::KITTY_IGNORED_LOCK_BITS as u32);
        if bits & !0b111111 != 0 {
            return None; // malformed input
        }

        let mut mods = Mods::EMPTY;
        if bits & 1 != 0 { mods |= Mods::SHIFT; }
        if bits & 2 != 0 { mods |= Mods::ALT; }
        if bits & 4 != 0 { mods |= Mods::CTRL; }
        if bits & 8 != 0 { mods |= Mods::SUPER; }
        if bits & 16 != 0 { mods |= Mods::HYPER; }
        if bits & 32 != 0 { mods |= Mods::META; }
        mods
    } else {
        return None;
    };
    skip_ws(body, idx);

    let kind = if body.get(*idx) == Some(&b':') {
        *idx += 1;
        skip_ws(body, idx);
        match read_u32(body, idx, 3)? {
            0 => return None,
            1 => KeyEventKind::Press,
            2 => KeyEventKind::Repeat,
            3 => KeyEventKind::Release,
            _ => unreachable!()
        }
    } else {
        KeyEventKind::Press
    };
    Some((mods, kind))
}

fn media_or_modifier_from_kitty_offset(offset: u32) -> Option<Key> {
    let key = match offset {
        0 => Key::Media(MediaKey::Play),
        1 => Key::Media(MediaKey::Pause),
        2 => Key::Media(MediaKey::PlayPause),
        3 => Key::Media(MediaKey::Reverse),
        4 => Key::Media(MediaKey::Stop),
        5 => Key::Media(MediaKey::FastForward),
        6 => Key::Media(MediaKey::Rewind),
        7 => Key::Media(MediaKey::TrackNext),
        8 => Key::Media(MediaKey::TrackPrevious),
        9 => Key::Media(MediaKey::Record),
        10 => Key::Media(MediaKey::LowerVolume),
        11 => Key::Media(MediaKey::RaiseVolume),
        12 => Key::Media(MediaKey::MuteVolume),

        13 => Key::ModifierKey(ModifierKey::LeftShift),
        14 => Key::ModifierKey(ModifierKey::LeftCtrl),
        15 => Key::ModifierKey(ModifierKey::LeftAlt),
        16 => Key::ModifierKey(ModifierKey::LeftSuper),
        17 => Key::ModifierKey(ModifierKey::LeftHyper),
        18 => Key::ModifierKey(ModifierKey::LeftMeta),

        19 => Key::ModifierKey(ModifierKey::RightShift),
        20 => Key::ModifierKey(ModifierKey::RightCtrl),
        21 => Key::ModifierKey(ModifierKey::RightAlt),
        22 => Key::ModifierKey(ModifierKey::RightSuper),
        23 => Key::ModifierKey(ModifierKey::RightHyper),
        24 => Key::ModifierKey(ModifierKey::RightMeta),

        25 => Key::IsoLevel3Shift,
        26 => Key::IsoLevel5Shift,

        _ => return None,
    };
    Some(key)
}

fn key_from_kitty_codepoint(code: u32) -> Option<Key> {
    // https://sw.kovidgoyal.net/kitty/keyboard-protocol/#functional-key-definitions
    match code {
        0x1b => return Some(Key::Esc),
        0x0d => return Some(Key::Enter),
        0x09 => return Some(Key::Tab),
        0x7f => return Some(Key::Backspace),

        57358 => return Some(Key::CapsLock),
        57359 => return Some(Key::ScrollLock),
        57360 => return Some(Key::NumLock),
        57361 => return Some(Key::PrintScreen),
        57362 => return Some(Key::Pause),
        57363 => return Some(Key::Menu),

        57427 => return Some(Key::Keypad(KeypadKey::Begin)),

        _ => {}
    }

    if (57376..=57398).contains(&code) {
        return Some(Key::Function((13 + (code - 57376)) as u8));
    }

    if (57399..=57408).contains(&code) {
        let key = match code - 57399 {
            0 => KeypadKey::Left,
            1 => KeypadKey::Right,
            2 => KeypadKey::Up,
            3 => KeypadKey::Down,
            4 => KeypadKey::PageUp,
            5 => KeypadKey::PageDown,
            6 => KeypadKey::Home,
            7 => KeypadKey::End,
            8 => KeypadKey::Insert,
            9 => KeypadKey::Delete,
            _ => unreachable!(),
        };
        return Some(Key::Keypad(key));
    }

    if (57428..=57454).contains(&code) {
        return media_or_modifier_from_kitty_offset(code - 57428);
    }

    None
}

fn token_from_codepoint(code: u32, mods: Mods, kind: KeyEventKind) -> Token {
    if let Some(key) = key_from_kitty_codepoint(code) {
        Token::Key { key, mods, kind }
    } else {
        // replace invalid codepoints with U+FFFD REPLACEMENT CHARACTER
        Token::Utf8 { ch: char::from_u32(code).unwrap_or('\u{fffd}'), mods, kind }
    }
}

fn parse_text_fields(params: &[u8], idx: &mut usize, mods: Mods, kind: KeyEventKind) -> Option<Vec<Token>> {
    let mut out = Vec::new();
    loop {
        skip_ws(params, idx);
        if *idx == params.len() {
            break;
        }
        if params[*idx] == b':' {
            // allow empty segments
            *idx += 1;
            continue;
        }
        // replace invalid codepoints with U+FFFD REPLACEMENT CHARACTER
        let code = read_u32(params, idx, 0x10ffff)?;
        let ch = char::from_u32(code).unwrap_or('\u{fffd}');
        out.push(Token::Utf8 { ch, mods, kind });
        skip_ws(params, idx);
        if *idx == params.len() {
            break;
        }
        if params[*idx] != b':' {
            return None;
        }
        *idx += 1;
    }
    Some(out)
}

pub fn encode_token(token: &Token, kitty_flags: u8) -> Option<Vec<u8>> {
    if kitty_flags == 0 {
        return None;
    }

    let report_type = (kitty_flags & FLAG_REPORT_EVENT_TYPES) != 0;
    let report_all_keys = (kitty_flags & FLAG_REPORT_ALL_KEYS) != 0;
    let report_text = (kitty_flags & FLAG_REPORT_ASSOCIATED_TEXT) != 0;

    let (code, mods, kind) = match token {
        Token::Utf8 { ch, mods, kind } => (*ch as u32, *mods, *kind),
        Token::Key { key, mods, kind } => (key_to_kitty_codepoint(*key)?, *mods, *kind),
    };
    let effective_kind = if report_type {
        kind
    } else {
        match kind {
            KeyEventKind::Press | KeyEventKind::Repeat => KeyEventKind::Press,
            KeyEventKind::Release => return None,
        }
    };

    let mod_param = mods.raw() + 1;

    if !report_all_keys && (code == 0 || (code <= 0x7f && mod_param == 1)) {
        return None;
    }

    let mut out = String::from("\x1b[");
    out.push_str(&code.to_string());

    if !report_type && !report_text && mod_param == 1 {
        out.push('u');
        return Some(out.into_bytes());
    }

    out.push(';');
    out.push_str(&mod_param.to_string());

    if report_type {
        out.push(':');
        out.push(match effective_kind {
            KeyEventKind::Press => '1',
            KeyEventKind::Repeat => '2',
            KeyEventKind::Release => '3',
        });
    }

    if report_text {
        out.push(';');
    }

    out.push('u');

    Some(out.into_bytes())
}

fn key_to_kitty_codepoint(key: Key) -> Option<u32> {
    match key {
        Key::Esc => Some(0x1b),
        Key::Enter => Some(0x0d),
        Key::Tab => Some(0x09),
        Key::Backspace => Some(0x7f),

        Key::CapsLock => Some(57358),
        Key::ScrollLock => Some(57359),
        Key::NumLock => Some(57360),
        Key::PrintScreen => Some(57361),
        Key::Pause => Some(57362),
        Key::Menu => Some(57363),

        Key::Function(n @ 13..=35) => Some(57376 + (n as u32 - 13)),

        Key::Keypad(KeypadKey::Left) => Some(57399),
        Key::Keypad(KeypadKey::Right) => Some(57400),
        Key::Keypad(KeypadKey::Up) => Some(57401),
        Key::Keypad(KeypadKey::Down) => Some(57402),
        Key::Keypad(KeypadKey::PageUp) => Some(57403),
        Key::Keypad(KeypadKey::PageDown) => Some(57404),
        Key::Keypad(KeypadKey::Home) => Some(57405),
        Key::Keypad(KeypadKey::End) => Some(57406),
        Key::Keypad(KeypadKey::Insert) => Some(57407),
        Key::Keypad(KeypadKey::Delete) => Some(57408),
        Key::Keypad(KeypadKey::Begin) => Some(57427),

        Key::Media(media) => media_to_kitty_codepoint(media),
        Key::ModifierKey(modkey) => modifier_to_kitty_codepoint(modkey),

        Key::IsoLevel3Shift => Some(57453),
        Key::IsoLevel5Shift => Some(57454),

        _ => None,
    }
}

fn media_to_kitty_codepoint(media: MediaKey) -> Option<u32> {
    let offset = match media {
        MediaKey::Play => 0,
        MediaKey::Pause => 1,
        MediaKey::PlayPause => 2,
        MediaKey::Reverse => 3,
        MediaKey::Stop => 4,
        MediaKey::FastForward => 5,
        MediaKey::Rewind => 6,
        MediaKey::TrackNext => 7,
        MediaKey::TrackPrevious => 8,
        MediaKey::Record => 9,
        MediaKey::LowerVolume => 10,
        MediaKey::RaiseVolume => 11,
        MediaKey::MuteVolume => 12,
    };
    Some(57428 + offset)
}

fn modifier_to_kitty_codepoint(modkey: ModifierKey) -> Option<u32> {
    let offset = match modkey {
        ModifierKey::LeftShift => 13,
        ModifierKey::LeftCtrl => 14,
        ModifierKey::LeftAlt => 15,
        ModifierKey::LeftSuper => 16,
        ModifierKey::LeftHyper => 17,
        ModifierKey::LeftMeta => 18,

        ModifierKey::RightShift => 19,
        ModifierKey::RightCtrl => 20,
        ModifierKey::RightAlt => 21,
        ModifierKey::RightSuper => 22,
        ModifierKey::RightHyper => 23,
        ModifierKey::RightMeta => 24,
    };
    Some(57428 + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::{Key, KeyEventKind, KeypadKey, MediaKey, ModifierKey, Mods, Token};
    use crate::term::control;

    fn csi(raw: &'static [u8]) -> CsiSeq<'static> {
        control::split_csi_body(raw).expect("valid CSI body")
    }

    fn utf8(ch: char, mods: Mods, kind: KeyEventKind) -> Token {
        Token::Utf8 { ch, mods, kind }
    }

    fn key(key: Key, mods: Mods, kind: KeyEventKind) -> Token {
        Token::Key { key, mods, kind }
    }

    #[test]
    fn decodes_plain_text_press() {
        assert_eq!(
            decode_csi_u(csi(b"114u")),
            Some(vec![utf8('r', Mods::EMPTY, KeyEventKind::Press)]),
        );
    }

    #[test]
    fn decodes_text_with_modifiers_as_press_when_event_type_omitted() {
        assert_eq!(
            decode_csi_u(csi(b"114;6u")),
            Some(vec![utf8('r', Mods::SHIFT | Mods::CTRL, KeyEventKind::Press)]),
        );
    }

    #[test]
    fn decodes_press_repeat_release_event_types() {
        assert_eq!(
            decode_csi_u(csi(b"114;5:1u")),
            Some(vec![utf8('r', Mods::CTRL, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"114;5:2u")),
            Some(vec![utf8('r', Mods::CTRL, KeyEventKind::Repeat)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"114;5:3u")),
            Some(vec![utf8('r', Mods::CTRL, KeyEventKind::Release)]),
        );
    }

    #[test]
    fn rejects_invalid_event_types() {
        assert_eq!(decode_csi_u(csi(b"114;5:0u")), None);
        assert_eq!(decode_csi_u(csi(b"114;5:4u")), None);
    }

    #[test]
    fn decodes_associated_text_field_preferred_over_primary_codepoint() {
        assert_eq!(
            decode_csi_u(csi(b"97;1;120u")),
            Some(vec![utf8('x', Mods::EMPTY, KeyEventKind::Press)]),
        );
    }

    #[test]
    fn decodes_multiple_associated_text_codepoints() {
        assert_eq!(
            decode_csi_u(csi(b"97;1;120:121u")),
            Some(vec![
                utf8('x', Mods::EMPTY, KeyEventKind::Press),
                utf8('y', Mods::EMPTY, KeyEventKind::Press),
            ]),
        );
    }

    #[test]
    fn associated_text_preserves_modifiers_and_kind() {
        assert_eq!(
            decode_csi_u(csi(b"97;5:2;120:121u")),
            Some(vec![
                utf8('x', Mods::CTRL, KeyEventKind::Repeat),
                utf8('y', Mods::CTRL, KeyEventKind::Repeat),
            ]),
        );
    }

    #[test]
    fn decodes_basic_named_keys() {
        assert_eq!(
            decode_csi_u(csi(b"27u")),
            Some(vec![key(Key::Esc, Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"13u")),
            Some(vec![key(Key::Enter, Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"9u")),
            Some(vec![key(Key::Tab, Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"127u")),
            Some(vec![key(Key::Backspace, Mods::EMPTY, KeyEventKind::Press)]),
        );
    }

    #[test]
    fn decodes_lock_and_misc_keys() {
        assert_eq!(
            decode_csi_u(csi(b"57358u")),
            Some(vec![key(Key::CapsLock, Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57359u")),
            Some(vec![key(Key::ScrollLock, Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57360u")),
            Some(vec![key(Key::NumLock, Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57361u")),
            Some(vec![key(Key::PrintScreen, Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57362u")),
            Some(vec![key(Key::Pause, Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57363u")),
            Some(vec![key(Key::Menu, Mods::EMPTY, KeyEventKind::Press)]),
        );
    }

    #[test]
    fn decodes_f13_to_f35() {
        assert_eq!(
            decode_csi_u(csi(b"57376u")),
            Some(vec![key(Key::Function(13), Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57398u")),
            Some(vec![key(Key::Function(35), Mods::EMPTY, KeyEventKind::Press)]),
        );
    }

    #[test]
    fn decodes_keypad_keys() {
        assert_eq!(
            decode_csi_u(csi(b"57427u")),
            Some(vec![key(Key::Keypad(KeypadKey::Begin), Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57399u")),
            Some(vec![key(Key::Keypad(KeypadKey::Left), Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57408;5:3u")),
            Some(vec![key(Key::Keypad(KeypadKey::Delete), Mods::CTRL, KeyEventKind::Release)]),
        );
    }

    #[test]
    fn decodes_media_keys() {
        assert_eq!(
            decode_csi_u(csi(b"57428u")),
            Some(vec![key(Key::Media(MediaKey::Play), Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57430u")),
            Some(vec![key(Key::Media(MediaKey::PlayPause), Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57440u")),
            Some(vec![key(Key::Media(MediaKey::MuteVolume), Mods::EMPTY, KeyEventKind::Press)]),
        );
    }

    #[test]
    fn decodes_modifier_keys_and_iso_level_keys() {
        assert_eq!(
            decode_csi_u(csi(b"57441u")),
            Some(vec![key(Key::ModifierKey(ModifierKey::LeftShift), Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57452u")),
            Some(vec![key(Key::ModifierKey(ModifierKey::RightMeta), Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57453u")),
            Some(vec![key(Key::IsoLevel3Shift, Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57454u")),
            Some(vec![key(Key::IsoLevel5Shift, Mods::EMPTY, KeyEventKind::Press)]),
        );
    }

    #[test]
    fn ignores_caps_lock_and_num_lock_modifier_bits() {
        assert_eq!(
            decode_csi_u(csi(b"57427;65u")), // capslock
            Some(vec![key(Key::Keypad(KeypadKey::Begin), Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57427;129u")), // numlock
            Some(vec![key(Key::Keypad(KeypadKey::Begin), Mods::EMPTY, KeyEventKind::Press)]),
        );
        assert_eq!(
            decode_csi_u(csi(b"57427;197u")), // ctrl + capslock + numlock
            Some(vec![key(Key::Keypad(KeypadKey::Begin), Mods::CTRL, KeyEventKind::Press)]),
        );
    }

    #[test]
    fn does_not_encode_when_kitty_disabled() {
        assert_eq!(
            encode_token(
                &utf8('r', Mods::CTRL, KeyEventKind::Press),
                0,
            ),
            None,
        );
    }

    #[test]
    fn does_not_encode_plain_ascii_without_report_all_keys() {
        assert_eq!(
            encode_token(
                &utf8('r', Mods::EMPTY, KeyEventKind::Press),
                FLAG_REPORT_EVENT_TYPES,
            ),
            None,
        );
    }

    #[test]
    fn encodes_modified_text_with_report_all_keys() {
        assert_eq!(
            encode_token(
                &utf8('r', Mods::CTRL, KeyEventKind::Press),
                FLAG_REPORT_ALL_KEYS,
            ),
            Some(b"\x1b[114;5u".to_vec()),
        );
        assert_eq!(
            encode_token(
                &utf8('r', Mods::SUPER, KeyEventKind::Press),
                FLAG_REPORT_ALL_KEYS,
            ),
            Some(b"\x1b[114;9u".to_vec()),
        );
    }

    #[test]
    fn encodes_press_with_event_type_when_requested() {
        assert_eq!(
            encode_token(
                &utf8('r', Mods::CTRL, KeyEventKind::Press),
                FLAG_REPORT_ALL_KEYS | FLAG_REPORT_EVENT_TYPES,
            ),
            Some(b"\x1b[114;5:1u".to_vec()),
        );
    }

    #[test]
    fn encodes_repeat_as_repeat_when_event_type_reporting_enabled() {
        assert_eq!(
            encode_token(
                &utf8('r', Mods::CTRL, KeyEventKind::Repeat),
                FLAG_REPORT_ALL_KEYS | FLAG_REPORT_EVENT_TYPES,
            ),
            Some(b"\x1b[114;5:2u".to_vec()),
        );
    }

    #[test]
    fn encodes_release_as_release_when_event_type_reporting_enabled() {
        assert_eq!(
            encode_token(
                &utf8('r', Mods::CTRL, KeyEventKind::Release),
                FLAG_REPORT_ALL_KEYS | FLAG_REPORT_EVENT_TYPES,
            ),
            Some(b"\x1b[114;5:3u".to_vec()),
        );
    }

    #[test]
    fn encodes_repeat_as_press_when_event_type_reporting_disabled() {
        assert_eq!(
            encode_token(
                &utf8('r', Mods::CTRL, KeyEventKind::Repeat),
                FLAG_REPORT_ALL_KEYS,
            ),
            Some(b"\x1b[114;5u".to_vec()),
        );
    }

    #[test]
    fn does_not_encode_release_when_event_type_reporting_disabled() {
        assert_eq!(
            encode_token(
                &utf8('r', Mods::CTRL, KeyEventKind::Release),
                FLAG_REPORT_ALL_KEYS,
            ),
            None,
        );
    }

    #[test]
    fn encodes_named_kitty_keys() {
        assert_eq!(
            encode_token(
                &key(Key::Function(13), Mods::EMPTY, KeyEventKind::Press),
                FLAG_REPORT_ALL_KEYS,
            ),
            Some(b"\x1b[57376u".to_vec()),
        );
        assert_eq!(
            encode_token(
                &key(Key::Keypad(KeypadKey::Begin), Mods::CTRL, KeyEventKind::Press),
                FLAG_REPORT_ALL_KEYS,
            ),
            Some(b"\x1b[57427;5u".to_vec()),
        );
        assert_eq!(
            encode_token(
                &key(Key::Media(MediaKey::PlayPause), Mods::EMPTY, KeyEventKind::Press),
                FLAG_REPORT_ALL_KEYS,
            ),
            Some(b"\x1b[57430u".to_vec()),
        );
        assert_eq!(
            encode_token(
                &key(Key::ModifierKey(ModifierKey::LeftShift), Mods::EMPTY, KeyEventKind::Press),
                FLAG_REPORT_ALL_KEYS,
            ),
            Some(b"\x1b[57441u".to_vec()),
        );
    }

    #[test]
    fn encodes_named_kitty_key_release_when_event_type_reporting_enabled() {
        assert_eq!(
            encode_token(
                &key(Key::Keypad(KeypadKey::Begin), Mods::CTRL, KeyEventKind::Release),
                FLAG_REPORT_ALL_KEYS | FLAG_REPORT_EVENT_TYPES,
            ),
            Some(b"\x1b[57427;5:3u".to_vec()),
        );
    }

    #[test]
    fn does_not_encode_named_kitty_key_release_without_event_type_reporting() {
        assert_eq!(
            encode_token(
                &key(Key::Keypad(KeypadKey::Begin), Mods::CTRL, KeyEventKind::Release),
                FLAG_REPORT_ALL_KEYS,
            ),
            None,
        );
    }

    #[test]
    fn does_not_encode_legacy_only_keys_in_kitty_encoder() {
        assert_eq!(
            encode_token(
                &key(Key::Function(1), Mods::EMPTY, KeyEventKind::Press),
                FLAG_REPORT_ALL_KEYS,
            ),
            None,
        );
    }
}
