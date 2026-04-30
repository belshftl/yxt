// SPDX-License-Identifier: MIT

use std::time::{Duration, Instant};

use crate::model::{Key, KeyEventKind, Mods, Token};
use super::{
    control::{self, ControlPrefix, CsiScan, StringControlKind, StringScan},
    kitty, legacy, mode::TermMode
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    Token(Token),
    Unknown(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeedMore {
    Esc,
    Csi,
    Ss3,
    Utf8,
    StTerminatedString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecoderConfig {
    pub mode: TermMode,
    pub esc_byte_is_partial_esc: bool,
    pub partial_utf8_timeout: Duration,
    pub partial_esc_timeout: Duration,
    pub partial_st_timeout: Duration,
    pub max_pending_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct Decoder {
    cfg: DecoderConfig,
    buf: Vec<u8>,
    pending: Option<NeedMore>,
    last_input_at: Option<Instant>,
}

impl Decoder {
    pub fn new(cfg: DecoderConfig) -> Self {
        Self {
            cfg,
            buf: Vec::new(),
            pending: None,
            last_input_at: None,
        }
    }

    pub fn config(&self) -> DecoderConfig {
        self.cfg
    }

    pub fn set_mode(&mut self, mode: TermMode) {
        self.cfg.mode = mode;
    }

    pub fn is_idle(&self) -> bool {
        self.buf.is_empty() && self.pending.is_none()
    }

    pub fn pending(&self) -> Option<NeedMore> {
        self.pending
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        let need = self.pending?;
        let last = self.last_input_at?;
        Some(last + self.timeout_for(need))
    }

    pub fn push(&mut self, now: Instant, bytes: &[u8], out: &mut Vec<Decoded>) {
        if !bytes.is_empty() {
            self.last_input_at = Some(now);
            self.buf.extend_from_slice(bytes);
        }

        if self.buf.len() > self.cfg.max_pending_bytes {
            self.flush_unknown(out);
            return;
        }

        self.drain_complete(out);

        if self.buf.len() > self.cfg.max_pending_bytes {
            self.flush_unknown(out);
        }
    }

    pub fn flush_timed_out(&mut self, now: Instant, out: &mut Vec<Decoded>) {
        let Some(deadline) = self.next_deadline() else {
            return;
        };

        if now < deadline {
            return;
        }

        self.flush_pending(out);
        self.drain_complete(out);
    }

    pub fn flush_all_unknown(&mut self, out: &mut Vec<Decoded>) {
        self.flush_unknown(out);
    }

    fn timeout_for(&self, need: NeedMore) -> Duration {
        match need {
            NeedMore::Utf8 => self.cfg.partial_utf8_timeout,
            NeedMore::Esc | NeedMore::Csi | NeedMore::Ss3 => self.cfg.partial_esc_timeout,
            NeedMore::StTerminatedString => self.cfg.partial_st_timeout,
        }
    }

    fn drain_complete(&mut self, out: &mut Vec<Decoded>) {
        loop {
            match decode_one(&self.buf, self.cfg.mode, self.cfg.esc_byte_is_partial_esc) {
                DecodeOne::Emit { item, consumed } => {
                    debug_assert!(consumed > 0);
                    self.buf.drain(..consumed);
                    self.pending = None;
                    out.push(item);
                    if self.buf.is_empty() {
                        self.last_input_at = None;
                    }
                }
                DecodeOne::EmitMany { items, consumed } => {
                    debug_assert!(consumed > 0);
                    self.buf.drain(..consumed);
                    self.pending = None;
                    out.extend(items);
                    if self.buf.is_empty() {
                        self.last_input_at = None;
                    }
                }
                DecodeOne::NeedMore(need) => {
                    if self.buf.is_empty() {
                        self.pending = None;
                        self.last_input_at = None;
                    } else {
                        self.pending = Some(need);
                    }
                    break;
                }
            }
        }
    }

    fn flush_pending(&mut self, out: &mut Vec<Decoded>) {
        let Some(need) = self.pending.take() else {
            return;
        };

        match need {
            NeedMore::Esc if self.buf == [0x1b] => {
                self.buf.clear();
                self.last_input_at = None;

                out.push(Decoded::Token(Token::Key {
                    key: Key::Esc,
                    mods: Mods::EMPTY,
                    kind: KeyEventKind::Press,
                }));
            }
            _ => self.flush_unknown(out)
        }
    }

    fn flush_unknown(&mut self, out: &mut Vec<Decoded>) {
        if self.buf.is_empty() {
            self.pending = None;
            self.last_input_at = None;
            return;
        }
        let raw = std::mem::take(&mut self.buf);
        self.pending = None;
        self.last_input_at = None;
        out.push(Decoded::Unknown(raw));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DecodeOne {
    Emit {
        item: Decoded,
        consumed: usize,
    },
    EmitMany {
        items: Vec<Decoded>,
        consumed: usize,
    },
    NeedMore(NeedMore),
}

fn decode_one(buf: &[u8], mode: TermMode, esc_byte_is_partial_esc: bool) -> DecodeOne {
    let Some(&first) = buf.first() else {
        return DecodeOne::NeedMore(NeedMore::Utf8);
    };

    if first == 0x1b {
        return decode_esc(buf, mode, esc_byte_is_partial_esc);
    }

    if let Some(token) = legacy::decode_c0(first) {
        return DecodeOne::Emit {
            item: Decoded::Token(token),
            consumed: 1,
        };
    }

    decode_utf8(buf)
}

fn decode_esc(buf: &[u8], mode: TermMode, esc_byte_is_partial_esc: bool) -> DecodeOne {
    if buf.len() == 1 {
        return if esc_byte_is_partial_esc {
            DecodeOne::NeedMore(NeedMore::Esc)
        } else {
            DecodeOne::Emit {
                item: Decoded::Token(Token::Key {
                    key: Key::Esc,
                    mods: Mods::EMPTY,
                    kind: KeyEventKind::Press,
                }),
                consumed: 1,
            }
        }
    }

    match control::classify_esc_prefixed(buf) {
        Some(ControlPrefix::Ss2) => DecodeOne::Emit {
            item: Decoded::Unknown(buf[..2].to_vec()),
            consumed: 2,
        },
        Some(ControlPrefix::Ss3) => decode_ss3(buf, mode),
        Some(ControlPrefix::Csi) => decode_csi(buf, mode),
        Some(ControlPrefix::String(kind)) => decode_string_control(buf, kind),
        Some(ControlPrefix::Esc(_)) => decode_alt_prefixed(buf, mode),
        None => DecodeOne::Emit {
            item: Decoded::Unknown(vec![buf[0]]),
            consumed: 1,
        },
    }
}

fn decode_csi(buf: &[u8], mode: TermMode) -> DecodeOne {
    match control::scan_csi(buf, false) {
        CsiScan::NeedMore => DecodeOne::NeedMore(NeedMore::Csi),
        CsiScan::Complete { csi, consumed } => {
            if let Some(tokens) = kitty::decode_csi_u(csi) {
                return DecodeOne::EmitMany {
                    items: tokens.into_iter().map(Decoded::Token).collect(),
                    consumed,
                };
            }
            if let Some(token) = legacy::decode_csi(csi, mode) {
                return DecodeOne::Emit {
                    item: Decoded::Token(token),
                    consumed,
                };
            }
            DecodeOne::Emit {
                item: Decoded::Unknown(buf[..consumed].to_vec()),
                consumed,
            }
        }
        CsiScan::Malformed { consumed } => DecodeOne::Emit {
            item: Decoded::Unknown(buf[..consumed].to_vec()),
            consumed,
        }
    }
}

fn decode_ss3(buf: &[u8], mode: TermMode) -> DecodeOne {
    if buf.len() < 3 {
        DecodeOne::NeedMore(NeedMore::Ss3)
    } else if let Some(token) = legacy::decode_ss3(&buf[2..3], mode) {
        DecodeOne::Emit {
            item: Decoded::Token(token),
            consumed: 3,
        }
    } else {
        DecodeOne::Emit {
            item: Decoded::Unknown(buf[..3].to_vec()),
            consumed: 3,
        }
    }
}

fn decode_string_control(buf: &[u8], kind: StringControlKind) -> DecodeOne {
    match control::scan_string_control(buf, kind, false) {
        StringScan::NeedMore => DecodeOne::NeedMore(NeedMore::StTerminatedString),
        StringScan::Complete { consumed } | StringScan::Malformed { consumed } => DecodeOne::Emit {
            item: Decoded::Unknown(buf[..consumed].to_vec()),
            consumed,
        },
    }
}

fn decode_alt_prefixed(buf: &[u8], mode: TermMode) -> DecodeOne {
    let sub = &buf[1..];
    match decode_one_non_esc(sub, mode) {
        DecodeOne::Emit { item: Decoded::Token(mut token), consumed } => {
            add_alt(&mut token);
            DecodeOne::Emit {
                item: Decoded::Token(token),
                consumed: consumed + 1,
            }
        }
        DecodeOne::Emit { item: Decoded::Unknown(_), consumed } => DecodeOne::Emit {
            item: Decoded::Unknown(buf[..consumed + 1].to_vec()),
            consumed: consumed + 1,
        },
        DecodeOne::NeedMore(need) => DecodeOne::NeedMore(need),
        DecodeOne::EmitMany { .. } => DecodeOne::Emit {
            item: Decoded::Unknown(vec![0x1b]),
            consumed: 1,
        },
    }
}

fn decode_one_non_esc(buf: &[u8], _mode: TermMode) -> DecodeOne {
    let Some(&first) = buf.first() else {
        return DecodeOne::NeedMore(NeedMore::Utf8);
    };

    if first == 0x1b {
        DecodeOne::Emit {
            item: Decoded::Unknown(vec![0x1b]),
            consumed: 1,
        }
    } else if let Some(token) = legacy::decode_c0(first) {
        DecodeOne::Emit {
            item: Decoded::Token(token),
            consumed: 1,
        }
    } else {
        decode_utf8(buf)
    }
}

fn decode_utf8(buf: &[u8]) -> DecodeOne {
    let Some(len) = utf8_len(buf[0]) else {
        return DecodeOne::Emit {
            item: Decoded::Unknown(vec![buf[0]]),
            consumed: 1,
        };
    };
    if buf.len() < len {
        return DecodeOne::NeedMore(NeedMore::Utf8);
    }
    let slice = &buf[..len];

    let Ok(s) = std::str::from_utf8(slice) else {
        return DecodeOne::Emit {
            item: Decoded::Unknown(vec![buf[0]]),
            consumed: 1,
        };
    };
    let mut chars = s.chars();
    let Some(ch) = chars.next() else {
        return DecodeOne::Emit {
            item: Decoded::Unknown(slice.to_vec()),
            consumed: len,
        };
    };
    if chars.next().is_some() {
        return DecodeOne::Emit {
            item: Decoded::Unknown(slice.to_vec()),
            consumed: len,
        };
    }

    DecodeOne::Emit {
        item: Decoded::Token(Token::Utf8 {
            ch,
            mods: Mods::EMPTY,
            kind: KeyEventKind::Press,
        }),
        consumed: len,
    }
}

fn utf8_len(first: u8) -> Option<usize> {
    if first <= 0x7f {
        Some(1)
    } else if (first & 0xe0) == 0xc0 {
        Some(2)
    } else if (first & 0xf0) == 0xe0 {
        Some(3)
    } else if (first & 0xf8) == 0xf0 {
        Some(4)
    } else {
        None
    }
}

fn add_alt(token: &mut Token) {
    match token {
        Token::Utf8 { mods, .. } | Token::Key { mods, .. } => {
            *mods |= Mods::ALT;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::{Direction, Key, KeyEventKind, Mods, Token};
    use crate::term::{kitty, mode::TermMode};

    fn cfg(esc_byte_is_partial_esc: bool) -> DecoderConfig {
        DecoderConfig {
            mode: TermMode::LEGACY,
            esc_byte_is_partial_esc,
            partial_utf8_timeout: Duration::from_millis(10),
            partial_esc_timeout: Duration::from_millis(20),
            partial_st_timeout: Duration::from_millis(50),
            max_pending_bytes: 4096,
        }
    }

    fn kitty_cfg(esc_byte_is_partial_esc: bool) -> DecoderConfig {
        DecoderConfig {
            mode: TermMode {
                decckm: false,
                deckpam: false,
                kitty_flags: kitty::FLAG_REPORT_ALL_KEYS | kitty::FLAG_REPORT_EVENT_TYPES,
            },
            esc_byte_is_partial_esc,
            partial_utf8_timeout: Duration::from_millis(10),
            partial_esc_timeout: Duration::from_millis(20),
            partial_st_timeout: Duration::from_millis(50),
            max_pending_bytes: 4096,
        }
    }

    fn deckpam_cfg() -> DecoderConfig {
        DecoderConfig {
            mode: TermMode {
                decckm: false,
                deckpam: true,
                kitty_flags: 0,
            },
            esc_byte_is_partial_esc: true,
            partial_utf8_timeout: Duration::from_millis(10),
            partial_esc_timeout: Duration::from_millis(20),
            partial_st_timeout: Duration::from_millis(50),
            max_pending_bytes: 4096,
        }
    }

    fn utf8(ch: char, mods: Mods, kind: KeyEventKind) -> Decoded {
        Decoded::Token(Token::Utf8 { ch, mods, kind })
    }

    fn key(key: Key, mods: Mods, kind: KeyEventKind) -> Decoded {
        Decoded::Token(Token::Key { key, mods, kind })
    }

    fn decode_all(config: DecoderConfig, bytes: &[u8]) -> Vec<Decoded> {
        let mut d = Decoder::new(config);
        let mut out = Vec::new();
        d.push(Instant::now(), bytes, &mut out);
        out
    }

    #[test]
    fn decodes_ascii_utf8_immediately() {
        assert_eq!(
            decode_all(cfg(false), b"x"),
            vec![utf8('x', Mods::EMPTY, KeyEventKind::Press)],
        );
    }

    #[test]
    fn decodes_non_ascii_utf8_immediately() {
        assert_eq!(
            decode_all(cfg(false), "å".as_bytes()),
            vec![utf8('å', Mods::EMPTY, KeyEventKind::Press)],
        );
    }

    #[test]
    fn buffers_partial_utf8_until_complete() {
        let mut d = Decoder::new(cfg(false));
        let mut out = Vec::new();
        let t0 = Instant::now();

        let bytes = "å".as_bytes();

        d.push(t0, &bytes[..1], &mut out);
        assert!(out.is_empty());
        assert_eq!(d.pending(), Some(NeedMore::Utf8));

        d.flush_timed_out(t0 + Duration::from_millis(5), &mut out);
        assert!(out.is_empty());

        d.push(t0 + Duration::from_millis(6), &bytes[1..], &mut out);

        assert_eq!(out, vec![utf8('å', Mods::EMPTY, KeyEventKind::Press)]);
        assert!(d.is_idle());
    }

    #[test]
    fn partial_utf8_flushes_unknown_after_timeout() {
        let mut d = Decoder::new(cfg(false));
        let mut out = Vec::new();
        let t0 = Instant::now();

        d.push(t0, &"å".as_bytes()[..1], &mut out);
        assert!(out.is_empty());

        d.flush_timed_out(t0 + Duration::from_millis(9), &mut out);
        assert!(out.is_empty());

        d.flush_timed_out(t0 + Duration::from_millis(10), &mut out);

        assert_eq!(out, vec![Decoded::Unknown(vec![0xc3])]);
        assert!(d.is_idle());
    }

    #[test]
    fn bare_esc_decodes_immediately_when_partial_disabled() {
        let mut d = Decoder::new(cfg(false));
        let mut out = Vec::new();

        d.push(Instant::now(), b"\x1b", &mut out);

        assert_eq!(out, vec![key(Key::Esc, Mods::EMPTY, KeyEventKind::Press)]);
        assert!(d.is_idle());
    }

    #[test]
    fn bare_esc_waits_when_partial_enabled() {
        let mut d = Decoder::new(cfg(true));
        let mut out = Vec::new();
        let t0 = Instant::now();

        d.push(t0, b"\x1b", &mut out);
        assert!(out.is_empty());
        assert_eq!(d.pending(), Some(NeedMore::Esc));

        d.flush_timed_out(t0 + Duration::from_millis(19), &mut out);
        assert!(out.is_empty());

        d.flush_timed_out(t0 + Duration::from_millis(20), &mut out);

        assert_eq!(out, vec![key(Key::Esc, Mods::EMPTY, KeyEventKind::Press)]);
        assert!(d.is_idle());
    }

    #[test]
    fn split_csi_decodes_when_esc_partial_enabled() {
        let mut d = Decoder::new(cfg(true));
        let mut out = Vec::new();
        let t0 = Instant::now();

        d.push(t0, b"\x1b", &mut out);
        assert!(out.is_empty());

        d.push(t0 + Duration::from_millis(5), b"[A", &mut out);

        assert_eq!(out, vec![key(Key::Arrow(Direction::Up), Mods::EMPTY, KeyEventKind::Press)]);
        assert!(d.is_idle());
    }

    #[test]
    fn split_csi_does_not_decode_when_esc_partial_disabled() {
        let mut d = Decoder::new(cfg(false));
        let mut out = Vec::new();
        let t0 = Instant::now();

        d.push(t0, b"\x1b", &mut out);
        assert_eq!(
            out,
            vec![key(Key::Esc, Mods::EMPTY, KeyEventKind::Press)],
        );

        d.push(t0 + Duration::from_millis(5), b"[A", &mut out);

        assert_eq!(
            out,
            vec![
                key(Key::Esc, Mods::EMPTY, KeyEventKind::Press),
                utf8('[', Mods::EMPTY, KeyEventKind::Press),
                utf8('A', Mods::EMPTY, KeyEventKind::Press),
            ],
        );
        assert!(d.is_idle());
    }

    #[test]
    fn already_started_csi_waits_even_when_esc_partial_disabled() {
        let mut d = Decoder::new(cfg(false));
        let mut out = Vec::new();
        let t0 = Instant::now();

        d.push(t0, b"\x1b[", &mut out);
        assert!(out.is_empty());
        assert_eq!(d.pending(), Some(NeedMore::Csi));

        d.push(t0 + Duration::from_millis(5), b"A", &mut out);

        assert_eq!(out, vec![key(Key::Arrow(Direction::Up), Mods::EMPTY, KeyEventKind::Press)]);
    }

    #[test]
    fn partial_csi_flushes_unknown_after_timeout() {
        let mut d = Decoder::new(cfg(false));
        let mut out = Vec::new();
        let t0 = Instant::now();

        d.push(t0, b"\x1b[", &mut out);
        assert!(out.is_empty());
        assert_eq!(d.pending(), Some(NeedMore::Csi));

        d.flush_timed_out(t0 + Duration::from_millis(19), &mut out);
        assert!(out.is_empty());

        d.flush_timed_out(t0 + Duration::from_millis(20), &mut out);

        assert_eq!(out, vec![Decoded::Unknown(b"\x1b[".to_vec())]);
        assert!(d.is_idle());
    }

    #[test]
    fn complete_unknown_csi_emits_unknown_immediately() {
        assert_eq!(
            decode_all(cfg(false), b"\x1b[999z"),
            vec![Decoded::Unknown(b"\x1b[999z".to_vec())],
        );
    }

    #[test]
    fn split_ss3_decodes_when_complete() {
        let mut d = Decoder::new(cfg(true));
        let mut out = Vec::new();
        let t0 = Instant::now();

        d.push(t0, b"\x1bO", &mut out);
        assert!(out.is_empty());
        assert_eq!(d.pending(), Some(NeedMore::Ss3));

        d.push(t0 + Duration::from_millis(5), b"P", &mut out);

        assert_eq!(out, vec![key(Key::Function(1), Mods::EMPTY, KeyEventKind::Press)]);
    }

    #[test]
    fn partial_ss3_flushes_unknown_after_timeout() {
        let mut d = Decoder::new(cfg(true));
        let mut out = Vec::new();
        let t0 = Instant::now();

        d.push(t0, b"\x1bO", &mut out);
        assert!(out.is_empty());

        d.flush_timed_out(t0 + Duration::from_millis(20), &mut out);

        assert_eq!(out, vec![Decoded::Unknown(b"\x1bO".to_vec())]);
        assert!(d.is_idle());
    }

    #[test]
    fn alt_prefixed_partial_utf8_waits() {
        let mut d = Decoder::new(cfg(true));
        let mut out = Vec::new();
        let t0 = Instant::now();

        let bytes = "å".as_bytes();

        d.push(t0, &[0x1b, bytes[0]], &mut out);
        assert!(out.is_empty());
        assert_eq!(d.pending(), Some(NeedMore::Utf8));

        d.push(t0 + Duration::from_millis(5), &bytes[1..], &mut out);

        assert_eq!(out, vec![utf8('å', Mods::ALT, KeyEventKind::Press)]);
    }

    #[test]
    fn split_kitty_csi_u_decodes_when_complete() {
        let mut d = Decoder::new(kitty_cfg(true));
        let mut out = Vec::new();
        let t0 = Instant::now();

        d.push(t0, b"\x1b[114;", &mut out);
        assert!(out.is_empty());
        assert_eq!(d.pending(), Some(NeedMore::Csi));

        d.push(t0 + Duration::from_millis(5), b"5:2u", &mut out);

        assert_eq!(out, vec![utf8('r', Mods::CTRL, KeyEventKind::Repeat)]);
    }

    #[test]
    fn osc_bel_terminated_emits_unknown_complete_sequence() {
        assert_eq!(
            decode_all(cfg(false), b"\x1b]0;title\x07x"),
            vec![
                Decoded::Unknown(b"\x1b]0;title\x07".to_vec()),
                utf8('x', Mods::EMPTY, KeyEventKind::Press),
            ],
        );
    }

    #[test]
    fn osc_st_terminated_emits_unknown_complete_sequence() {
        assert_eq!(
            decode_all(cfg(false), b"\x1b]0;title\x1b\\x"),
            vec![
                Decoded::Unknown(b"\x1b]0;title\x1b\\".to_vec()),
                utf8('x', Mods::EMPTY, KeyEventKind::Press),
            ],
        );
    }

    #[test]
    fn partial_osc_waits_and_flushes_unknown_after_st_timeout() {
        let mut d = Decoder::new(cfg(false));
        let mut out = Vec::new();
        let t0 = Instant::now();

        d.push(t0, b"\x1b]0;title", &mut out);
        assert!(out.is_empty());
        assert_eq!(d.pending(), Some(NeedMore::StTerminatedString));

        d.flush_timed_out(t0 + Duration::from_millis(49), &mut out);
        assert!(out.is_empty());

        d.flush_timed_out(t0 + Duration::from_millis(50), &mut out);

        assert_eq!(out, vec![Decoded::Unknown(b"\x1b]0;title".to_vec())]);
        assert!(d.is_idle());
    }

    #[test]
    fn dcs_requires_st_not_bel() {
        let mut d = Decoder::new(cfg(false));
        let mut out = Vec::new();
        let t0 = Instant::now();

        d.push(t0, b"\x1bPabc\x07", &mut out);
        assert!(out.is_empty());
        assert_eq!(d.pending(), Some(NeedMore::StTerminatedString));

        d.push(t0 + Duration::from_millis(5), b"\x1b\\x", &mut out);

        assert_eq!(
            out,
            vec![
                Decoded::Unknown(b"\x1bPabc\x07\x1b\\".to_vec()),
                utf8('x', Mods::EMPTY, KeyEventKind::Press),
            ],
        );
    }

    #[test]
    fn next_deadline_uses_last_input_time_and_pending_kind() {
        let mut d = Decoder::new(cfg(true));
        let mut out = Vec::new();
        let t0 = Instant::now();

        assert_eq!(d.next_deadline(), None);

        d.push(t0, b"\x1b", &mut out);

        assert_eq!(d.next_deadline(), Some(t0 + Duration::from_millis(20)));

        d.push(t0 + Duration::from_millis(5), b"[", &mut out);

        assert_eq!(d.next_deadline(), Some(t0 + Duration::from_millis(25)));
    }

    #[test]
    fn max_pending_bytes_flushes_unknown() {
        let mut c = cfg(false);

        c.max_pending_bytes = 4;

        let mut d = Decoder::new(c);
        let mut out = Vec::new();

        d.push(Instant::now(), b"\x1b]12345", &mut out);

        assert_eq!(out, vec![Decoded::Unknown(b"\x1b]12345".to_vec())]);
        assert!(d.is_idle());
    }

    #[test]
    fn flush_all_unknown_clears_pending_buffer() {
        let mut d = Decoder::new(cfg(true));
        let mut out = Vec::new();

        d.push(Instant::now(), b"\x1b[", &mut out);
        assert!(out.is_empty());
        assert_eq!(d.pending(), Some(NeedMore::Csi));

        d.flush_all_unknown(&mut out);

        assert_eq!(out, vec![Decoded::Unknown(b"\x1b[".to_vec())]);
        assert!(d.is_idle());
    }
}
