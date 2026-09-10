// SPDX-FileCopyrightText: 2026 belshftl
// SPDX-License-Identifier: MIT

use std::os::fd::BorrowedFd;
use std::time::{Duration, Instant};

use super::control::{ControlEvent, ControlScanner};
use super::kitty;
use super::mode::{TermMode, TerminalModeTracker};
use super::query::QueriedTermMode;
use crate::model::{Config, KeyPattern, Mapping, Protocol, Source, TokenPattern};
use crate::runtime::io::write_all_until;

// always turned on for kitty
const BASE_KITTY_FLAGS: u8 = kitty::FLAG_DISAMBIGUATE_ESCAPE_CODES | kitty::FLAG_REPORT_EVENT_TYPES;

// extra that's turned on for kitty if a mapping needs a key only ever reported as an escape code
const ALL_KEYS_KITTY_FLAGS: u8 = kitty::FLAG_REPORT_ALL_KEYS | kitty::FLAG_REPORT_ASSOCIATED_TEXT;

// how long to wait for the terminal to take the sequence to restore terminal state on shutdown
const RESTORE_TIMEOUT: Duration = Duration::from_millis(40);

// length beyond which a partially scanned sequence is for sure not one we need to suppress
// intentionally a big overestimate
const MAX_CANDIDATE_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NegotiateError {
    #[error(
        "the terminal does not support the '{0}' keyboard protocol or any higher-ranked protocol"
    )]
    Unsupported(&'static str),

    #[error(
        "\
the terminal didn't finish answering the capability query, so whether it supports the '{0}' \
keyboard protocol couldn't be confirmed; raise 'mode_query_timeout_ms' if the terminal is merely \
slow (e.g. slow ssh/serial connection)"
    )]
    QueryIncomplete(&'static str),

    #[error(
        "\
the config sets 'force_backspace_sends_del', but this terminal doesn't support it; it reports \
backspace as permamently locked to sending BS (i.e. DECBKM is perm-set)"
    )]
    BackspaceLockedToBs,
}

pub const BACKSPACE_DEL_SEQUENCE: &[u8] = b"\x1b[?67l";
pub const BACKSPACE_BS_SEQUENCE: &[u8] = b"\x1b[?67h";

/// Whether [`BACKSPACE_DEL_SEQUENCE`] needs to be sent, given what the terminal reported.
///
/// This only refuses if the terminal reports DECBKM as perm-set. Not implementing DECBKM is not the
/// same as encoding backspace as BS, and most terminals that don't implement it send DEL (TODO:
/// that claim needs some more concrete backing).
pub fn plan_backspace_del(
    config: &Config,
    queried: QueriedTermMode,
) -> Result<bool, NegotiateError> {
    if !config.options.force_backspace_sends_del {
        return Ok(false);
    }

    match queried.decbkm {
        Some(decbkm) if decbkm.sends_bs() && decbkm.is_changeable() => Ok(true),
        Some(decbkm) if decbkm.sends_bs() => Err(NegotiateError::BackspaceLockedToBs),
        _ => Ok(false),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Negotiation {
    pub protocol: Protocol,
    pub kitty_flags: u8,
}

pub fn plan(
    config: &Config,
    queried: QueriedTermMode,
) -> Result<Option<Negotiation>, NegotiateError> {
    let requested = config.protocol.protocol;

    if let Some(picked) = Protocol::ALL
        .iter()
        .copied()
        .filter(|p| *p >= requested && supports(*p, queried))
        .min()
    {
        Ok(match picked {
            Protocol::Legacy => None,
            Protocol::Kitty => Some(Negotiation {
                protocol: Protocol::Kitty,
                kitty_flags: kitty_flags_for(&config.mappings),
            }),
        })
    } else {
        Err(if queried.complete {
            NegotiateError::Unsupported(requested.name())
        } else {
            NegotiateError::QueryIncomplete(requested.name())
        })
    }
}

fn supports(protocol: Protocol, queried: QueriedTermMode) -> bool {
    match protocol {
        Protocol::Legacy => true,
        Protocol::Kitty => queried.kitty_flags.is_some(),
    }
}

fn kitty_flags_for(mappings: &[Mapping]) -> u8 {
    let mut flags = BASE_KITTY_FLAGS;
    if mappings.iter().any(|m| source_needs_all_keys(&m.from)) {
        flags |= ALL_KEYS_KITTY_FLAGS;
    }
    flags
}

fn source_needs_all_keys(source: &Source) -> bool {
    match source {
        Source::Token(TokenPattern::Key {
            key: KeyPattern::Named(key),
            ..
        }) => !key.is_legacy_reportable(),
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Pass,
    Suppress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushOutcome {
    pub upstream_changed: bool,
    pub downstream_changed: bool,
}

#[derive(Debug)]
pub struct TermProxy {
    tracker: TerminalModeTracker,
    scanner: ControlScanner,
    negotiated_flags: Option<u8>,

    pending: Vec<u8>,
    streaming: bool,
    decision: Decision,
    events: Vec<ControlEvent>,

    last_sent_flags: Option<u8>,
    alt_screen: bool,
}

impl TermProxy {
    pub fn new(queried: QueriedTermMode, negotiation: Option<Negotiation>) -> Self {
        let tracker = TerminalModeTracker::from_queried(queried);
        let alt_screen = tracker.alt_screen();
        Self {
            tracker,
            scanner: ControlScanner::default(),
            negotiated_flags: negotiation.map(|n| n.kitty_flags),
            pending: Vec::new(),
            streaming: false,
            decision: Decision::Pass,
            events: Vec::new(),
            last_sent_flags: None,
            alt_screen,
        }
    }

    pub fn upstream_mode(&self) -> TermMode {
        let mut mode = self.tracker.mode();
        mode.kitty_flags |= self.negotiated_flags.unwrap_or(0);
        mode
    }

    pub fn downstream_mode(&self) -> TermMode {
        self.tracker.mode()
    }

    pub fn enable_sequence(&mut self) -> Option<Vec<u8>> {
        self.negotiated_flags?;
        let desired = self.desired_upstream_flags();
        self.last_sent_flags = Some(desired);
        Some(kitty::push_flags_sequence(desired))
    }

    pub fn restore_sequence(&self) -> Vec<u8> {
        match self.negotiated_flags {
            Some(_) => kitty::POP_FLAGS_SEQUENCE.to_vec(),
            None => Vec::new(),
        }
    }

    pub fn push(
        &mut self,
        bytes: &[u8],
        to_terminal: &mut Vec<u8>,
        to_child: &mut Vec<u8>,
    ) -> PushOutcome {
        let was_upstream = self.upstream_mode();
        let was_downstream = self.downstream_mode();

        let mut idx = 0;
        while idx < bytes.len() {
            // outside sequence, everything up to the next passes through
            if self.pending.is_empty() && !self.streaming && self.scanner.in_ground() {
                let rest = &bytes[idx..];
                let run = rest.iter().position(|&b| b == 0x1b).unwrap_or(rest.len());
                to_terminal.extend_from_slice(&rest[..run]);
                idx += run;
                if idx == bytes.len() {
                    break;
                }
            }

            // inside sequence; one that's already known to not be ours passes through, one that
            // might still be ours is briefly held, so the scanner is never handed more than it
            // takes to find the answer
            let rest = &bytes[idx..];
            let take = if self.streaming {
                rest.len()
            } else {
                // not zero so the loop always makes progress
                let room = (MAX_CANDIDATE_BYTES + 1)
                    .saturating_sub(self.pending.len())
                    .max(1);
                rest.len().min(room)
            };

            let mut events = std::mem::take(&mut self.events);
            events.clear();
            let consumed = self.scanner.resume(&rest[..take], &mut events);

            if self.streaming {
                to_terminal.extend_from_slice(&rest[..consumed]);
            } else {
                debug_assert!(!self.pending.is_empty() || rest[0] == 0x1b);
                self.pending.extend_from_slice(&rest[..consumed]);
            }
            idx += consumed;

            for event in &events {
                self.handle_event(event, to_child);
            }
            self.events = events;

            if self.scanner.in_ground() {
                self.finish_sequence(to_terminal);
            } else if !self.streaming && !self.is_candidate() {
                to_terminal.extend_from_slice(&self.pending);
                self.pending.clear();
                self.streaming = true;
            }
        }

        self.sync_terminal_flags(to_terminal);

        PushOutcome {
            upstream_changed: self.upstream_mode() != was_upstream,
            downstream_changed: self.downstream_mode() != was_downstream,
        }
    }

    fn handle_event(&mut self, event: &ControlEvent, to_child: &mut Vec<u8>) {
        self.tracker.apply_event(event);

        if self.negotiated_flags.is_none() {
            return;
        }
        let ControlEvent::Csi(csi) = event else {
            return;
        };
        match kitty::classify_control(csi.as_csi()) {
            Some(kitty::Control::Set | kitty::Control::Push | kitty::Control::Pop) => {
                self.decision = Decision::Suppress;
            }
            Some(kitty::Control::Query) => {
                to_child.extend_from_slice(&kitty::query_reply(self.tracker.mode().kitty_flags));
                self.decision = Decision::Suppress;
            }
            None => {}
        }
    }

    fn finish_sequence(&mut self, to_terminal: &mut Vec<u8>) {
        if !self.streaming && self.decision == Decision::Pass {
            to_terminal.extend_from_slice(&self.pending);
        }
        self.pending.clear();
        self.streaming = false;
        self.decision = Decision::Pass;
    }

    fn is_candidate(&self) -> bool {
        self.pending.len() <= MAX_CANDIDATE_BYTES && self.pending.get(1).is_none_or(|b| *b == b'[')
    }

    fn desired_upstream_flags(&self) -> u8 {
        self.negotiated_flags.unwrap_or(0) | self.tracker.mode().kitty_flags
    }

    fn sync_terminal_flags(&mut self, to_terminal: &mut Vec<u8>) {
        if self.negotiated_flags.is_none() {
            return;
        }

        let alt_screen = self.tracker.alt_screen();
        if alt_screen != self.alt_screen {
            self.alt_screen = alt_screen;
            self.last_sent_flags = None;
        }

        let desired = self.desired_upstream_flags();
        if self.last_sent_flags != Some(desired) {
            to_terminal.extend_from_slice(&kitty::set_flags_sequence(desired));
            self.last_sent_flags = Some(desired);
        }
    }
}

#[derive(Debug)]
pub struct RestoreGuard<'a> {
    fd: BorrowedFd<'a>,
    sequence: Vec<u8>,
}

impl<'a> RestoreGuard<'a> {
    pub fn new(fd: BorrowedFd<'a>, sequence: Vec<u8>) -> Self {
        Self { fd, sequence }
    }
}

impl Drop for RestoreGuard<'_> {
    fn drop(&mut self) {
        _ = write_all_until(self.fd, &self.sequence, Instant::now() + RESTORE_TIMEOUT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::loader::ConfigLoader;
    use crate::model::{ProtocolRequest, ProtocolVerb};
    use crate::term::decode::{Decoded, Decoder, DecoderConfig};
    use crate::term::encode::Encoder;
    use crate::term::query::Decbkm;

    fn cfg(src: &str) -> Config {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.conf");
        std::fs::write(&path, src).unwrap();
        ConfigLoader::new().parse_file(&path).unwrap()
    }

    fn queried(kitty_flags: Option<u8>) -> QueriedTermMode {
        QueriedTermMode {
            decckm: Some(false),
            deckpam: Some(false),
            alt_screen: Some(false),
            kitty_flags,
            decbkm: None,
            complete: true,
        }
    }

    fn proxy(negotiated: Option<u8>) -> TermProxy {
        let mut proxy = TermProxy::new(
            queried(negotiated.map(|_| 0)),
            negotiated.map(|kitty_flags| Negotiation {
                protocol: Protocol::Kitty,
                kitty_flags,
            }),
        );
        proxy.enable_sequence();
        proxy
    }

    fn push(proxy: &mut TermProxy, bytes: &[u8]) -> (Vec<u8>, Vec<u8>, PushOutcome) {
        let mut to_terminal = Vec::new();
        let mut to_child = Vec::new();
        let outcome = proxy.push(bytes, &mut to_terminal, &mut to_child);
        (to_terminal, to_child, outcome)
    }

    fn with_decbkm(decbkm: Option<Decbkm>) -> QueriedTermMode {
        QueriedTermMode {
            decbkm,
            ..queried(None)
        }
    }

    fn backspace_del_cfg() -> Config {
        cfg("@version 1\nforce_backspace_sends_del = true\n")
    }

    #[test]
    fn legacy_request_negotiates_nothing() {
        let cfg = cfg("@version 1\n");

        assert_eq!(plan(&cfg, queried(None)).unwrap(), None);
        assert_eq!(plan(&cfg, queried(Some(0))).unwrap(), None);
    }

    #[test]
    fn kitty_request_negotiates_kitty_when_supported() {
        let cfg = cfg("@version 1\n@protocol want kitty\n");

        assert_eq!(
            plan(&cfg, queried(Some(0))).unwrap(),
            Some(Negotiation {
                protocol: Protocol::Kitty,
                kitty_flags: BASE_KITTY_FLAGS,
            }),
        );
    }

    #[test]
    fn kitty_request_fails_on_a_terminal_without_it() {
        let cfg = cfg("@version 1\n@protocol want kitty\n");

        assert_eq!(
            plan(&cfg, queried(None)),
            Err(NegotiateError::Unsupported("kitty")),
        );
    }

    #[test]
    fn an_unfinished_query_is_reported_separately() {
        let cfg = cfg("@version 1\n@protocol want kitty\n");
        let queried = QueriedTermMode::default();

        assert_eq!(
            plan(&cfg, queried),
            Err(NegotiateError::QueryIncomplete("kitty")),
        );
    }

    #[test]
    fn all_keys_is_only_negotiated_when_a_mapping_needs_it() {
        let modifiers_only = cfg("\
@version 1
@protocol want kitty
key('r'~, super) => send_key('x')
");
        assert_eq!(
            plan(&modifiers_only, queried(Some(0)))
                .unwrap()
                .unwrap()
                .kitty_flags,
            BASE_KITTY_FLAGS,
        );

        let bare_mod_key = cfg("\
@version 1
@protocol want kitty
key(left_super) => send_key('x')
");
        assert_eq!(
            plan(&bare_mod_key, queried(Some(0)))
                .unwrap()
                .unwrap()
                .kitty_flags,
            BASE_KITTY_FLAGS | ALL_KEYS_KITTY_FLAGS,
        );
    }

    #[test]
    fn plan_uses_the_configs_request() {
        let mut cfg = cfg("@version 1\n");
        cfg.protocol = ProtocolRequest {
            verb: ProtocolVerb::Want,
            protocol: Protocol::Kitty,
        };

        assert!(plan(&cfg, queried(Some(0))).unwrap().is_some());
    }

    #[test]
    fn without_negotiation_everything_passes_through() {
        let mut p = proxy(None);

        let (to_terminal, to_child, _) =
            push(&mut p, b"hello\x1b[?1049h\x1b[=5u\x1b[?u\x1b]0;title\x07");

        assert_eq!(
            to_terminal,
            b"hello\x1b[?1049h\x1b[=5u\x1b[?u\x1b]0;title\x07"
        );
        assert!(to_child.is_empty());
    }

    #[test]
    fn plain_output_is_forwarded_as_is_while_negotiating() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));

        let (to_terminal, to_child, outcome) = push(
            &mut p,
            b"hello \xf0\x9f\x91\x8b\x1b[1;31mred\x1b[0m\x1b]0;t\x07",
        );

        assert_eq!(
            to_terminal,
            b"hello \xf0\x9f\x91\x8b\x1b[1;31mred\x1b[0m\x1b]0;t\x07"
        );
        assert!(to_child.is_empty());
        assert!(!outcome.upstream_changed);
        assert!(!outcome.downstream_changed);
    }

    #[test]
    fn output_split_across_pushes_is_reassembled() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));

        let (to_terminal, ..) = push(&mut p, b"a\x1b[");
        assert_eq!(to_terminal, b"a");

        let (to_terminal, ..) = push(&mut p, b"1;31m");
        assert_eq!(to_terminal, b"\x1b[1;31m");

        let (to_terminal, ..) = push(&mut p, b"b");
        assert_eq!(to_terminal, b"b");
    }

    #[test]
    fn a_long_string_control_is_not_held_up() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));
        let long = "x".repeat(MAX_CANDIDATE_BYTES * 4);

        let (to_terminal, ..) = push(&mut p, format!("\x1b]0;{long}").as_bytes());

        assert_eq!(to_terminal, format!("\x1b]0;{long}").as_bytes());
    }

    #[test]
    fn a_long_csi_is_not_held_up() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));
        let params = "1;".repeat(MAX_CANDIDATE_BYTES);

        let (to_terminal, ..) = push(&mut p, format!("\x1b[{params}0m").as_bytes());

        assert_eq!(to_terminal, format!("\x1b[{params}0m").as_bytes());
    }

    #[test]
    fn malformed_sequences_pass_through() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));

        let (to_terminal, ..) = push(&mut p, b"\x1b[1\x80x");

        assert_eq!(to_terminal, b"\x1b[1\x80x");
    }

    #[test]
    fn the_childs_flag_changes_are_suppressed_and_re_derived() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));

        let (to_terminal, to_child, outcome) = push(&mut p, b"\x1b[=5u");

        // the child's sequence never reaches the terminal, what does is our union of the two
        assert_eq!(to_terminal, kitty::set_flags_sequence(BASE_KITTY_FLAGS | 5));
        assert!(to_child.is_empty());
        assert!(outcome.upstream_changed);
        assert!(outcome.downstream_changed);

        assert_eq!(p.downstream_mode().kitty_flags, 5);
        assert_eq!(p.upstream_mode().kitty_flags, BASE_KITTY_FLAGS | 5);
    }

    #[test]
    fn the_childs_push_and_pop_are_suppressed_and_re_derived() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));

        let (to_terminal, ..) = push(&mut p, b"\x1b[>5u");
        assert_eq!(to_terminal, kitty::set_flags_sequence(BASE_KITTY_FLAGS | 5));

        let (to_terminal, ..) = push(&mut p, b"\x1b[<u");
        assert_eq!(to_terminal, kitty::set_flags_sequence(BASE_KITTY_FLAGS));
        assert_eq!(p.downstream_mode().kitty_flags, 0);
    }

    #[test]
    fn a_flag_change_that_wouldnt_change_terminal_state_sends_nothing() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));

        // the child asks for flags we already turned on
        let (to_terminal, ..) = push(&mut p, format!("\x1b[={BASE_KITTY_FLAGS}u").as_bytes());

        assert!(to_terminal.is_empty());
        assert_eq!(p.downstream_mode().kitty_flags, BASE_KITTY_FLAGS);
    }

    #[test]
    fn a_query_is_answered_with_the_childs_own_flags() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));

        let (to_terminal, to_child, _) = push(&mut p, b"\x1b[?u");

        assert!(to_terminal.is_empty());
        assert_eq!(to_child, kitty::query_reply(0));

        push(&mut p, b"\x1b[=5u");
        let (_, to_child, _) = push(&mut p, b"\x1b[?u");
        assert_eq!(to_child, kitty::query_reply(5));
    }

    #[test]
    fn a_flags_reply_from_the_child_is_not_mistaken_for_a_query() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));

        let (to_terminal, to_child, _) = push(&mut p, b"\x1b[?5u");

        assert_eq!(to_terminal, b"\x1b[?5u");
        assert!(to_child.is_empty());
    }

    #[test]
    fn kitty_sequences_with_intermediates_are_left_alone() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));

        // not something the tracker acts on, so not something we may swallow
        let (to_terminal, ..) = push(&mut p, b"\x1b[=5 u");

        assert_eq!(to_terminal, b"\x1b[=5 u");
    }

    #[test]
    fn suppressed_sequences_split_across_pushes_leave_nothing_behind() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));

        let (to_terminal, ..) = push(&mut p, b"a\x1b[=");
        assert_eq!(to_terminal, b"a");

        let (to_terminal, ..) = push(&mut p, b"5u");
        assert_eq!(to_terminal, kitty::set_flags_sequence(BASE_KITTY_FLAGS | 5));
    }

    #[test]
    fn switching_screens_resends_the_flags() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));

        // the alternate screen has its own kitty state, which our push did not reach
        let (to_terminal, ..) = push(&mut p, b"\x1b[?1049h");
        assert_eq!(
            to_terminal,
            [
                b"\x1b[?1049h".as_slice(),
                &kitty::set_flags_sequence(BASE_KITTY_FLAGS)
            ]
            .concat(),
        );

        let (to_terminal, ..) = push(&mut p, b"\x1b[?1049l");
        assert_eq!(
            to_terminal,
            [
                b"\x1b[?1049l".as_slice(),
                &kitty::set_flags_sequence(BASE_KITTY_FLAGS)
            ]
            .concat(),
        );
    }

    #[test]
    fn the_enable_sequence_keeps_what_the_terminal_already_had() {
        let mut p = TermProxy::new(
            queried(Some(kitty::FLAG_REPORT_ALL_KEYS)),
            Some(Negotiation {
                protocol: Protocol::Kitty,
                kitty_flags: BASE_KITTY_FLAGS,
            }),
        );

        assert_eq!(
            p.enable_sequence(),
            Some(kitty::push_flags_sequence(
                BASE_KITTY_FLAGS | kitty::FLAG_REPORT_ALL_KEYS
            )),
        );
    }

    #[test]
    fn nothing_is_enabled_when_not_negotiating() {
        let mut p = proxy(None);

        assert_eq!(p.enable_sequence(), None);
        assert_eq!(p.upstream_mode(), p.downstream_mode());
    }

    #[test]
    fn long_string_control_split_across_pushes_is_streamed_whole() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));
        let long = "y".repeat(MAX_CANDIDATE_BYTES * 4);

        let (to_terminal, ..) = push(&mut p, format!("\x1b_G{long}").as_bytes());
        assert_eq!(to_terminal, format!("\x1b_G{long}").as_bytes());

        let (to_terminal, ..) = push(&mut p, format!("{long}\x1b\\after").as_bytes());
        assert_eq!(to_terminal, format!("{long}\x1b\\after").as_bytes());
    }

    #[test]
    fn kitty_input_is_translated_for_a_legacy_child() {
        let p = proxy(Some(BASE_KITTY_FLAGS | ALL_KEYS_KITTY_FLAGS));
        let mut decoder = Decoder::new(DecoderConfig {
            mode: p.upstream_mode(),
            esc_byte_is_partial_esc: false,
            partial_utf8_timeout: Duration::from_millis(10),
            partial_esc_timeout: Duration::from_millis(10),
            partial_st_timeout: Duration::from_millis(10),
            max_pending_bytes: 4096,
        });
        let encoder = Encoder::new(p.downstream_mode());

        assert_eq!(p.downstream_mode().kitty_flags, 0);

        let cases: &[(&[u8], &[u8])] = &[
            // plain key, reported as an escape code because of report-all-keys
            (b"\x1b[97;;97u", b"a"),
            // shifted character, only spelled out by the associated text
            (b"\x1b[50;2;64u", b"@"),
            (b"\x1b[97;5u", b"\x01"),
            (b"\x1b[9;2u", b"\x1b[Z"),
            (b"\x1b[27u", b"\x1b"),
            (b"\x1b[57399u", b"\x1b[D"),
            // not representable in legacy, child doesn't get anything
            (b"\x1b[57444u", b""),
            (b"\x1b[97;1:3u", b""),
        ];

        for (input, expected) in cases {
            let mut decoded = Vec::new();
            decoder.push(Instant::now(), input, &mut decoded);

            let mut encoded = Vec::new();
            for item in &decoded {
                let Decoded::Token(token) = item else {
                    panic!("expected a token for {input:?}, got {item:?}");
                };
                if let Some(bytes) = encoder.encode_token(token) {
                    encoded.extend_from_slice(&bytes);
                }
            }

            assert_eq!(
                encoded,
                *expected,
                "{:?} -> {:?}, wanted {:?}",
                String::from_utf8_lossy(input),
                String::from_utf8_lossy(&encoded),
                String::from_utf8_lossy(expected),
            );
        }
    }

    #[test]
    fn other_mode_changes_still_reach_both_ends() {
        let mut p = proxy(Some(BASE_KITTY_FLAGS));

        let (to_terminal, _, outcome) = push(&mut p, b"\x1b[?1h\x1b=");

        assert_eq!(to_terminal, b"\x1b[?1h\x1b=");
        assert!(outcome.upstream_changed);
        assert!(outcome.downstream_changed);
        assert!(p.upstream_mode().decckm);
        assert!(p.downstream_mode().decckm);
        assert!(p.upstream_mode().deckpam);
    }

    #[test]
    fn backspace_is_left_alone_unless_the_option_asks_for_it() {
        // no mapping is consulted: the option is the whole of the decision
        let plain = cfg("@version 1\nkey('h'~, any) => send_key('y')\n");

        for decbkm in [
            None,
            Some(Decbkm::Set),
            Some(Decbkm::PermSet),
            Some(Decbkm::Reset),
            Some(Decbkm::PermReset),
        ] {
            assert_eq!(
                plan_backspace_del(&plain, with_decbkm(decbkm)),
                Ok(false),
                "{decbkm:?} should have been left alone",
            );
        }
    }

    #[test]
    fn the_terminal_is_only_asked_to_switch_if_it_has_to() {
        // xterm: sends BS but lets that be changed, so ask it to
        assert_eq!(
            plan_backspace_del(&backspace_del_cfg(), with_decbkm(Some(Decbkm::Set))),
            Ok(true),
        );

        // vte: already sends DEL, so there is nothing to ask for and nothing to put back
        assert_eq!(
            plan_backspace_del(&backspace_del_cfg(), with_decbkm(Some(Decbkm::Reset))),
            Ok(false),
        );
        assert_eq!(
            plan_backspace_del(&backspace_del_cfg(), with_decbkm(Some(Decbkm::PermReset))),
            Ok(false),
        );
    }

    #[test]
    fn a_terminal_that_says_nothing_falls_back_to_the_configs_word() {
        // not implementing DECBKM is not the same as encoding backspace as BS
        assert_eq!(
            plan_backspace_del(&backspace_del_cfg(), with_decbkm(None)),
            Ok(false)
        );
    }

    #[test]
    fn the_option_is_refused_if_the_terminal_contradicts_it() {
        assert_eq!(
            plan_backspace_del(&backspace_del_cfg(), with_decbkm(Some(Decbkm::PermSet))),
            Err(NegotiateError::BackspaceLockedToBs),
        );
    }
}
