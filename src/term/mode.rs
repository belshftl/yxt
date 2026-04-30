// SPDX-License-Identifier: MIT

use crate::term::control::{self, ControlEvent, ControlScanner, CsiSeq};

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

const KITTY_STACK_LIMIT: usize = 64;

#[derive(Debug, Clone)]
struct KittyState {
    flags: u8,
    stack: Vec<u8>,
}

impl KittyState {
    fn new() -> Self {
        Self {
            flags: 0,
            stack: Vec::new(),
        }
    }

    fn push(&mut self, flags: u8) {
        if self.stack.len() == KITTY_STACK_LIMIT {
            self.stack.remove(0);
        }
        self.stack.push(self.flags);
        self.flags = flags;
    }

    fn pop(&mut self, count: u32) {
        for _ in 0..count {
            match self.stack.pop() {
                Some(flags) => self.flags = flags,
                None => {
                    self.flags = 0;
                    break;
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TerminalModeTracker {
    decckm: bool,
    deckpam: bool,
    alt_screen: bool,
    main_kitty: KittyState,
    alt_kitty: KittyState,
    scanner: ControlScanner,
}

impl TerminalModeTracker {
    pub fn new() -> Self {
        Self {
            decckm: false,
            deckpam: false,
            alt_screen: false,
            main_kitty: KittyState::new(),
            alt_kitty: KittyState::new(),
            scanner: ControlScanner::default(),
        }
    }

    pub fn mode(&self) -> TermMode {
        TermMode {
            decckm: self.decckm,
            deckpam: self.deckpam,
            kitty_flags: self.active_kitty().flags,
        }
    }

    pub fn observe_child_output(&mut self, bytes: &[u8]) -> bool {
        let old = self.mode();
        for event in self.scanner.push(bytes) {
            match event {
                ControlEvent::Esc(b'=') => self.deckpam = true,
                ControlEvent::Esc(b'>') => self.deckpam = false,
                ControlEvent::Csi(csi) => self.apply_csi(csi.as_csi()),
                _ => {}
            }
        }
        self.mode() != old
    }

    fn apply_csi(&mut self, csi: CsiSeq<'_>) {
        match (csi.private_marker(), csi.final_byte) {
            (Some(b'?'), b'h') => {
                if let Some(params) = control::parse_private_simple_params(csi, b'?') {
                    for param in params {
                        self.apply_dec_private_mode(param, true);
                    }
                }
            }
            (Some(b'?'), b'l') => {
                if let Some(params) = control::parse_private_simple_params(csi, b'?') {
                    for param in params {
                        self.apply_dec_private_mode(param, false);
                    }
                }
            }
            (Some(b'='), b'u') => self.apply_kitty_set(csi),
            (Some(b'>'), b'u') => self.apply_kitty_push(csi),
            (Some(b'<'), b'u') => self.apply_kitty_pop(csi),
            _ => {}
        }
    }

    fn apply_dec_private_mode(&mut self, param: u32, enabled: bool) {
        match param {
            1 => self.decckm = enabled,
            47 | 1047 | 1049 => self.alt_screen = enabled,
            _ => {}
        }
    }

    fn apply_kitty_set(&mut self, csi: CsiSeq<'_>) {
        if !csi.intermediates.is_empty() {
            return;
        }
        let Some(params) = control::parse_simple_params(csi.params_without_private_marker()) else {
            return;
        };
        let Some(&flags) = params.first() else {
            return;
        };
        let Ok(flags) = u8::try_from(flags) else {
            return;
        };
        let mode = params.get(1).copied().unwrap_or(1);
        match mode {
            1 => self.active_kitty_mut().flags = flags,
            2 => self.active_kitty_mut().flags |= flags,
            3 => self.active_kitty_mut().flags &= !flags,
            _ => {}
        }
    }

    fn apply_kitty_push(&mut self, csi: CsiSeq<'_>) {
        if !csi.intermediates.is_empty() {
            return;
        }
        let Some(params) = control::parse_simple_params(csi.params_without_private_marker()) else {
            return;
        };
        let flags = params.first().copied().unwrap_or(0);
        let Ok(flags) = u8::try_from(flags) else {
            return;
        };
        self.active_kitty_mut().push(flags);
    }

    fn apply_kitty_pop(&mut self, csi: CsiSeq<'_>) {
        if !csi.intermediates.is_empty() {
            return;
        }
        let Some(params) = control::parse_simple_params(csi.params_without_private_marker()) else {
            return;
        };
        let count = params.first().copied().unwrap_or(1);
        self.active_kitty_mut().pop(count);
    }

    fn active_kitty(&self) -> &KittyState {
        if self.alt_screen {
            &self.alt_kitty
        } else {
            &self.main_kitty
        }
    }

    fn active_kitty_mut(&mut self) -> &mut KittyState {
        if self.alt_screen {
            &mut self.alt_kitty
        } else {
            &mut self.main_kitty
        }
    }
}

impl Default for TerminalModeTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::term::kitty;

    #[test]
    fn initial_mode_is_plain_legacy() {
        let tracker = TerminalModeTracker::new();

        assert_eq!(
            tracker.mode(),
            TermMode {
                decckm: false,
                deckpam: false,
                kitty_flags: 0,
            },
        );
    }

    #[test]
    fn tracks_deckpam_and_deckpnm() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b="));
        assert_eq!(tracker.mode().deckpam, true);

        assert!(!tracker.observe_child_output(b"x"));
        assert_eq!(tracker.mode().deckpam, true);

        assert!(tracker.observe_child_output(b"\x1b>"));
        assert_eq!(tracker.mode().deckpam, false);
    }

    #[test]
    fn tracks_deckpam_split_across_pushes() {
        let mut tracker = TerminalModeTracker::new();

        assert!(!tracker.observe_child_output(b"\x1b"));
        assert_eq!(tracker.mode().deckpam, false);

        assert!(tracker.observe_child_output(b"="));
        assert_eq!(tracker.mode().deckpam, true);
    }

    #[test]
    fn tracks_decckm_private_mode_set_reset() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b[?1h"));
        assert_eq!(tracker.mode().decckm, true);

        assert!(tracker.observe_child_output(b"\x1b[?1l"));
        assert_eq!(tracker.mode().decckm, false);
    }

    #[test]
    fn tracks_decckm_split_across_pushes() {
        let mut tracker = TerminalModeTracker::new();

        assert!(!tracker.observe_child_output(b"\x1b[?"));
        assert_eq!(tracker.mode().decckm, false);

        assert!(tracker.observe_child_output(b"1h"));
        assert_eq!(tracker.mode().decckm, true);
    }

    #[test]
    fn ignores_non_decckm_private_modes_except_alt_screen() {
        let mut tracker = TerminalModeTracker::new();

        assert!(!tracker.observe_child_output(b"\x1b[?25l"));
        assert_eq!(
            tracker.mode(),
            TermMode {
                decckm: false,
                deckpam: false,
                kitty_flags: 0,
            },
        );
    }

    #[test]
    fn applies_multiple_dec_private_params() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b[?25;1h"));
        assert_eq!(tracker.mode().decckm, true);

        assert!(tracker.observe_child_output(b"\x1b[?25;1l"));
        assert_eq!(tracker.mode().decckm, false);
    }

    #[test]
    fn ignores_csi_with_intermediates_for_dec_private_modes() {
        let mut tracker = TerminalModeTracker::new();

        assert!(!tracker.observe_child_output(b"\x1b[?1 h"));
        assert_eq!(tracker.mode().decckm, false);
    }

    #[test]
    fn tracks_kitty_set_exact() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b[=5u"));
        assert_eq!(tracker.mode().kitty_flags, 5);

        assert!(tracker.observe_child_output(b"\x1b[=1u"));
        assert_eq!(tracker.mode().kitty_flags, 1);
    }

    #[test]
    fn tracks_kitty_set_with_explicit_mode_1() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b[=5;1u"));
        assert_eq!(tracker.mode().kitty_flags, 5);
    }

    #[test]
    fn tracks_kitty_set_bits_mode_2() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b[=1u"));
        assert_eq!(tracker.mode().kitty_flags, 1);

        assert!(tracker.observe_child_output(b"\x1b[=4;2u"));
        assert_eq!(tracker.mode().kitty_flags, 5);
    }

    #[test]
    fn tracks_kitty_reset_bits_mode_3() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b[=7u"));
        assert_eq!(tracker.mode().kitty_flags, 7);

        assert!(tracker.observe_child_output(b"\x1b[=2;3u"));
        assert_eq!(tracker.mode().kitty_flags, 5);
    }

    #[test]
    fn ignores_unknown_kitty_set_mode() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b[=5u"));
        assert_eq!(tracker.mode().kitty_flags, 5);

        assert!(!tracker.observe_child_output(b"\x1b[=1;99u"));
        assert_eq!(tracker.mode().kitty_flags, 5);
    }

    #[test]
    fn ignores_kitty_set_with_bad_flags() {
        let mut tracker = TerminalModeTracker::new();

        assert!(!tracker.observe_child_output(b"\x1b[=999u"));
        assert_eq!(tracker.mode().kitty_flags, 0);
    }

    #[test]
    fn tracks_kitty_push_and_pop() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b[=1u"));
        assert_eq!(tracker.mode().kitty_flags, 1);

        assert!(tracker.observe_child_output(b"\x1b[>5u"));
        assert_eq!(tracker.mode().kitty_flags, 5);

        assert!(tracker.observe_child_output(b"\x1b[<u"));
        assert_eq!(tracker.mode().kitty_flags, 1);
    }

    #[test]
    fn kitty_push_without_flags_pushes_zero() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b[=5u"));
        assert_eq!(tracker.mode().kitty_flags, 5);

        assert!(tracker.observe_child_output(b"\x1b[>u"));
        assert_eq!(tracker.mode().kitty_flags, 0);

        assert!(tracker.observe_child_output(b"\x1b[<u"));
        assert_eq!(tracker.mode().kitty_flags, 5);
    }

    #[test]
    fn kitty_pop_with_explicit_count() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b[=1u"));
        assert_eq!(tracker.mode().kitty_flags, 1);

        assert!(tracker.observe_child_output(b"\x1b[>2u"));
        assert_eq!(tracker.mode().kitty_flags, 2);

        assert!(tracker.observe_child_output(b"\x1b[>3u"));
        assert_eq!(tracker.mode().kitty_flags, 3);

        assert!(tracker.observe_child_output(b"\x1b[<2u"));
        assert_eq!(tracker.mode().kitty_flags, 1);
    }

    #[test]
    fn kitty_pop_past_empty_resets_to_zero() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b[=7u"));
        assert_eq!(tracker.mode().kitty_flags, 7);

        assert!(tracker.observe_child_output(b"\x1b[<u"));
        assert_eq!(tracker.mode().kitty_flags, 0);
    }

    #[test]
    fn tracks_kitty_sequences_split_across_pushes() {
        let mut tracker = TerminalModeTracker::new();

        assert!(!tracker.observe_child_output(b"\x1b[="));
        assert_eq!(tracker.mode().kitty_flags, 0);

        assert!(tracker.observe_child_output(b"5;1u"));
        assert_eq!(tracker.mode().kitty_flags, 5);

        assert!(!tracker.observe_child_output(b"\x1b[>"));
        assert_eq!(tracker.mode().kitty_flags, 5);

        assert!(tracker.observe_child_output(b"9u"));
        assert_eq!(tracker.mode().kitty_flags, 9);

        assert!(!tracker.observe_child_output(b"\x1b[<"));
        assert_eq!(tracker.mode().kitty_flags, 9);

        assert!(tracker.observe_child_output(b"u"));
        assert_eq!(tracker.mode().kitty_flags, 5);
    }

    #[test]
    fn tracks_main_and_alt_screen_kitty_stacks_separately() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b[=1u"));
        assert_eq!(tracker.mode().kitty_flags, 1);

        assert!(tracker.observe_child_output(b"\x1b[?1049h"));
        assert_eq!(tracker.mode().kitty_flags, 0);

        assert!(tracker.observe_child_output(b"\x1b[=5u"));
        assert_eq!(tracker.mode().kitty_flags, 5);

        assert!(tracker.observe_child_output(b"\x1b[?1049l"));
        assert_eq!(tracker.mode().kitty_flags, 1);

        assert!(tracker.observe_child_output(b"\x1b[?1049h"));
        assert_eq!(tracker.mode().kitty_flags, 5);
    }

    #[test]
    fn tracks_alt_screen_47_1047_1049() {
        for param in [47, 1047, 1049] {
            let mut tracker = TerminalModeTracker::new();

            assert!(tracker.observe_child_output(b"\x1b[=1u"));
            assert_eq!(tracker.mode().kitty_flags, 1);

            assert!(tracker.observe_child_output(format!("\x1b[?{param}h").as_bytes()));
            assert_eq!(tracker.mode().kitty_flags, 0);

            assert!(tracker.observe_child_output(b"\x1b[=5u"));
            assert_eq!(tracker.mode().kitty_flags, 5);

            assert!(tracker.observe_child_output(format!("\x1b[?{param}l").as_bytes()));
            assert_eq!(tracker.mode().kitty_flags, 1);
        }
    }

    #[test]
    fn alt_screen_switch_does_not_reset_deckpam_or_decckm() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(b"\x1b="));
        assert!(tracker.observe_child_output(b"\x1b[?1h"));
        assert_eq!(
            tracker.mode(),
            TermMode {
                decckm: true,
                deckpam: true,
                kitty_flags: 0,
            },
        );

        assert!(!tracker.observe_child_output(b"\x1b[?1049h"));
        assert_eq!(
            tracker.mode(),
            TermMode {
                decckm: true,
                deckpam: true,
                kitty_flags: 0,
            },
        );
    }

    #[test]
    fn ignores_dcs_sos_osc_pm_apc_payloads() {
        let mut tracker = TerminalModeTracker::new();

        assert!(!tracker.observe_child_output(b"\x1bPpayload\x1b\\"));
        assert!(!tracker.observe_child_output(b"\x1bXpayload\x1b\\"));
        assert!(!tracker.observe_child_output(b"\x1b]0;title\x07"));
        assert!(!tracker.observe_child_output(b"\x1b^payload\x1b\\"));
        assert!(!tracker.observe_child_output(b"\x1b_payload\x1b\\"));
        assert_eq!(
            tracker.mode(),
            TermMode {
                decckm: false,
                deckpam: false,
                kitty_flags: 0,
            },
        );
    }

    #[test]
    fn split_osc_does_not_mess_with_later_csi() {
        let mut tracker = TerminalModeTracker::new();

        assert!(!tracker.observe_child_output(b"\x1b]0;title"));
        assert!(!tracker.observe_child_output(b"\x07"));

        assert!(tracker.observe_child_output(b"\x1b[?1h"));
        assert_eq!(tracker.mode().decckm, true);
    }

    #[test]
    fn malformed_csi_is_ignored_and_scanner_recovers() {
        let mut tracker = TerminalModeTracker::new();

        assert!(!tracker.observe_child_output(b"\x1b[?1\x80"));

        assert!(tracker.observe_child_output(b"\x1b[?1h"));
        assert_eq!(tracker.mode().decckm, true);
    }

    #[test]
    fn c1_controls_are_not_tracked_by_default() {
        let mut tracker = TerminalModeTracker::new();

        assert!(!tracker.observe_child_output(b"\x9b?1h"));
        assert_eq!(tracker.mode().decckm, false);

        assert!(!tracker.observe_child_output(b"\x9b=5u"));
        assert_eq!(tracker.mode().kitty_flags, 0);
    }

    #[test]
    fn kitty_flag_constants_can_be_tracked() {
        let mut tracker = TerminalModeTracker::new();

        assert!(tracker.observe_child_output(format!("\x1b[={}u", kitty::FLAG_REPORT_ALL_KEYS).as_bytes()));
        assert_eq!(tracker.mode().kitty_flags, kitty::FLAG_REPORT_ALL_KEYS);

        assert!(tracker.observe_child_output(format!("\x1b[={};2u", kitty::FLAG_REPORT_EVENT_TYPES).as_bytes()));
        assert_eq!(
            tracker.mode().kitty_flags,
            kitty::FLAG_REPORT_ALL_KEYS | kitty::FLAG_REPORT_EVENT_TYPES,
        );

        assert!(tracker.observe_child_output(format!("\x1b[={};3u", kitty::FLAG_REPORT_ALL_KEYS).as_bytes()));
        assert_eq!(tracker.mode().kitty_flags, kitty::FLAG_REPORT_EVENT_TYPES);
    }
}
