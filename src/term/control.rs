// SPDX-License-Identifier: MIT

pub const DEFAULT_MAX_CSI_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlScannerConfig {
    pub accept_c1: bool,
    pub max_csi_bytes: usize,
}

impl Default for ControlScannerConfig {
    fn default() -> Self {
        Self {
            accept_c1: false,
            max_csi_bytes: DEFAULT_MAX_CSI_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StringControlKind {
    Dcs,
    Sos,
    Osc,
    Pm,
    Apc,
}

impl StringControlKind {
    pub fn allows_bel_termination(self) -> bool {
        matches!(self, Self::Osc)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlPrefix {
    Ss2,
    Ss3,
    Csi,
    String(StringControlKind),
    Esc(u8),
}

pub fn classify_esc_byte(b: u8) -> ControlPrefix {
    match b {
        b'N' => ControlPrefix::Ss2,
        b'O' => ControlPrefix::Ss3,
        b'P' => ControlPrefix::String(StringControlKind::Dcs),
        b'X' => ControlPrefix::String(StringControlKind::Sos),
        b'[' => ControlPrefix::Csi,
        b']' => ControlPrefix::String(StringControlKind::Osc),
        b'^' => ControlPrefix::String(StringControlKind::Pm),
        b'_' => ControlPrefix::String(StringControlKind::Apc),
        _ => ControlPrefix::Esc(b),
    }
}

pub fn classify_esc_prefixed(buf: &[u8]) -> Option<ControlPrefix> {
    match *buf {
        [0x1b, b, ..] => Some(classify_esc_byte(b)),
        _ => None,
    }
}

pub fn classify_c1(byte: u8) -> Option<ControlPrefix> {
    match byte {
        0x8e => Some(ControlPrefix::Ss2),
        0x8f => Some(ControlPrefix::Ss3),
        0x90 => Some(ControlPrefix::String(StringControlKind::Dcs)),
        0x98 => Some(ControlPrefix::String(StringControlKind::Sos)),
        0x9b => Some(ControlPrefix::Csi),
        0x9d => Some(ControlPrefix::String(StringControlKind::Osc)),
        0x9e => Some(ControlPrefix::String(StringControlKind::Pm)),
        0x9f => Some(ControlPrefix::String(StringControlKind::Apc)),
        _ => None,
    }
}

pub fn classify_control_prefix(buf: &[u8], accept_c1: bool) -> Option<ControlPrefix> {
    if let Some(prefix) = classify_esc_prefixed(buf) {
        Some(prefix)
    } else if accept_c1 {
        buf.first().and_then(|b| classify_c1(*b))
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsiSeq<'a> {
    pub raw: &'a [u8],
    pub params: &'a [u8],
    pub intermediates: &'a [u8],
    pub final_byte: u8,
}

impl<'a> CsiSeq<'a> {
    pub fn private_marker(self) -> Option<u8> {
        match self.params.first().copied() {
            Some(b'?' | b'>' | b'=' | b'<') => Some(self.params[0]),
            _ => None,
        }
    }

    pub fn params_without_private_marker(self) -> &'a [u8] {
        match self.private_marker() {
            Some(_) => &self.params[1..],
            None => self.params,
        }
    }

    pub fn to_owned(self) -> OwnedCsiSeq {
        OwnedCsiSeq {
            raw: self.raw.to_vec(),
            params: self.params.to_vec(),
            intermediates: self.intermediates.to_vec(),
            final_byte: self.final_byte,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedCsiSeq {
    pub raw: Vec<u8>,
    pub params: Vec<u8>,
    pub intermediates: Vec<u8>,
    pub final_byte: u8,
}

impl OwnedCsiSeq {
    pub fn as_csi(&self) -> CsiSeq<'_> {
        CsiSeq {
            raw: &self.raw,
            params: &self.params,
            intermediates: &self.intermediates,
            final_byte: self.final_byte,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsiScan<'a> {
    NeedMore,
    Complete {
        csi: CsiSeq<'a>,
        consumed: usize,
    },
    Malformed {
        consumed: usize,
    },
}

pub fn scan_csi(buf: &[u8], accept_c1: bool) -> CsiScan<'_> {
    let prefix_len = if buf.starts_with(b"\x1b[") {
        2
    } else if accept_c1 && buf.first() == Some(&0x9b) {
        1
    } else {
        return CsiScan::Malformed {
            consumed: usize::from(!buf.is_empty()),
        };
    };

    let mut idx = prefix_len;
    while idx < buf.len() && (0x30..=0x3f).contains(&buf[idx]) {
        idx += 1;
    }
    while idx < buf.len() && (0x20..=0x2f).contains(&buf[idx]) {
        idx += 1;
    }
    if idx >= buf.len() {
        return CsiScan::NeedMore;
    }

    if !(0x40..=0x7e).contains(&buf[idx]) {
        return CsiScan::Malformed {
            consumed: idx + 1,
        };
    }
    let raw = &buf[prefix_len..=idx];
    let Some(csi) = split_csi_body(raw) else {
        return CsiScan::Malformed {
            consumed: idx + 1,
        };
    };
    CsiScan::Complete {
        csi,
        consumed: idx + 1,
    }
}

pub fn split_csi_body(raw: &[u8]) -> Option<CsiSeq<'_>> {
    let (&final_byte, body) = raw.split_last()?;
    if !(0x40..=0x7e).contains(&final_byte) {
        return None;
    }

    let mut idx = 0;
    let params_start = idx;
    while idx < body.len() && (0x30..=0x3f).contains(&body[idx]) {
        idx += 1;
    }

    let params_end = idx;
    while idx < body.len() && (0x20..=0x2f).contains(&body[idx]) {
        idx += 1;
    }

    if idx != body.len() {
        return None;
    }

    Some(CsiSeq {
        raw,
        params: &raw[params_start..params_end],
        intermediates: &raw[params_end..body.len()],
        final_byte,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringScan {
    NeedMore,
    Complete {
        consumed: usize,
    },
    Malformed {
        consumed: usize,
    },
}

pub fn scan_string_control(buf: &[u8], kind: StringControlKind, accept_c1: bool) -> StringScan {
    let idx = if buf.first() == Some(&0x1b) {
        if buf.len() < 2 {
            return StringScan::NeedMore;
        }
        match classify_esc_prefixed(buf) {
            Some(ControlPrefix::String(k)) if k == kind => 2,
            _ => return StringScan::Malformed { consumed: 1 },
        }
    } else if accept_c1 && buf.first().and_then(|b| classify_c1(*b)) == Some(ControlPrefix::String(kind)) {
        1
    } else {
        return StringScan::Malformed {
            consumed: usize::from(!buf.is_empty()),
        };
    };

    let mut idx = idx;
    while idx < buf.len() {
        match buf[idx] {
            0x07 if kind.allows_bel_termination() => return StringScan::Complete { consumed: idx + 1 },
            0x9c if accept_c1 => return StringScan::Complete { consumed: idx + 1 },
            0x1b if buf.get(idx + 1) == Some(&b'\\') => return StringScan::Complete { consumed: idx + 2 },
            _ => idx += 1,
        }
    }

    StringScan::NeedMore
}

pub fn parse_simple_params(params: &[u8]) -> Option<Vec<u32>> {
    if params.is_empty() {
        return Some(Vec::new());
    }
    let mut idx = 0;
    let mut values = Vec::new();
    loop {
        let value = read_u32(params, &mut idx, u32::MAX)?;
        values.push(value);
        if idx == params.len() {
            break;
        }
        if params[idx] != b';' {
            return None;
        }
        idx += 1;
        if idx == params.len() {
            return None;
        }
    }
    Some(values)
}

pub fn parse_private_simple_params(csi: CsiSeq<'_>, marker: u8) -> Option<Vec<u32>> {
    if !csi.intermediates.is_empty() {
        return None;
    }
    if csi.private_marker()? != marker {
        return None;
    }
    parse_simple_params(csi.params_without_private_marker())
}

pub fn read_u32(buf: &[u8], idx: &mut usize, max: u32) -> Option<u32> {
    let start = *idx;
    let mut value = 0u32;
    while *idx < buf.len() && buf[*idx].is_ascii_digit() {
        let digit = (buf[*idx] - b'0') as u32;
        value = value.checked_mul(10)?.checked_add(digit)?;
        if value > max {
            return None;
        }
        *idx += 1;
    }
    if *idx == start {
        return None;
    }
    Some(value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlEvent {
    Esc(u8),
    Ss2,
    Ss3,
    Csi(OwnedCsiSeq),
    StringTerminated(StringControlKind),
    Unknown,
}

#[derive(Debug, Clone)]
pub struct ControlScanner {
    cfg: ControlScannerConfig,
    state: ScannerState,
    out: Vec<ControlEvent>,
}

impl ControlScanner {
    pub fn new(cfg: ControlScannerConfig) -> Self {
        Self {
            cfg,
            state: ScannerState::Ground,
            out: Vec::new(),
        }
    }

    pub fn config(&self) -> ControlScannerConfig {
        self.cfg
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<ControlEvent> {
        self.out.clear();
        for &b in bytes {
            self.push_byte(b);
        }
        std::mem::take(&mut self.out)
    }

    fn push_byte(&mut self, b: u8) {
        match &mut self.state {
            ScannerState::Ground => {
                if b == 0x1b {
                    self.state = ScannerState::Esc;
                    return;
                }
                if !self.cfg.accept_c1 {
                    return;
                }
                if let Some(prefix) = classify_c1(b) {
                    self.enter_prefix(prefix);
                }
            }
            ScannerState::Esc => {
                let prefix = classify_esc_byte(b);
                self.enter_prefix(prefix);
            }
            ScannerState::Csi { bytes } => match b {
                0x20..=0x3f => {
                    bytes.push(b);
                    if bytes.len() > self.cfg.max_csi_bytes {
                        self.out.push(ControlEvent::Unknown);
                        self.state = ScannerState::Ground;
                    }
                }
                0x40..=0x7e => {
                    bytes.push(b);
                    if let Some(csi) = split_csi_body(bytes) {
                        self.out.push(ControlEvent::Csi(csi.to_owned()));
                    } else {
                        self.out.push(ControlEvent::Unknown);
                    }
                    self.state = ScannerState::Ground;
                }
                _ => {
                    self.out.push(ControlEvent::Unknown);
                    self.state = ScannerState::Ground;
                }
            },
            ScannerState::String { kind, seen_esc } => {
                if *seen_esc {
                    if b == b'\\' {
                        self.out.push(ControlEvent::StringTerminated(*kind));
                        self.state = ScannerState::Ground;
                    } else {
                        *seen_esc = b == 0x1b;
                    }
                    return;
                }
                if b == 0x9c && self.cfg.accept_c1 {
                    self.out.push(ControlEvent::StringTerminated(*kind));
                    self.state = ScannerState::Ground;
                } else if b == 0x07 && kind.allows_bel_termination() {
                    self.out.push(ControlEvent::StringTerminated(*kind));
                    self.state = ScannerState::Ground;
                } else if b == 0x1b {
                    *seen_esc = true;
                }
            }
        }
    }

    fn enter_prefix(&mut self, prefix: ControlPrefix) {
        match prefix {
            ControlPrefix::Ss2 => {
                self.out.push(ControlEvent::Ss2);
                self.state = ScannerState::Ground;
            }
            ControlPrefix::Ss3 => {
                self.out.push(ControlEvent::Ss3);
                self.state = ScannerState::Ground;
            }
            ControlPrefix::Csi => {
                self.state = ScannerState::Csi {
                    bytes: Vec::new(),
                };
            }
            ControlPrefix::String(kind) => {
                self.state = ScannerState::String {
                    kind,
                    seen_esc: false,
                };
            }
            ControlPrefix::Esc(b) => {
                self.out.push(ControlEvent::Esc(b));
                self.state = ScannerState::Ground;
            }
        }
    }
}

impl Default for ControlScanner {
    fn default() -> Self {
        Self::new(ControlScannerConfig::default())
    }
}

#[derive(Debug, Clone)]
enum ScannerState {
    Ground,
    Esc,
    Csi {
        bytes: Vec<u8>,
    },
    String {
        kind: StringControlKind,
        seen_esc: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_csi(buf: &[u8], accept_c1: bool) -> (CsiSeq<'_>, usize) {
        match scan_csi(buf, accept_c1) {
            CsiScan::Complete { csi, consumed } => (csi, consumed),
            other => panic!("expected complete CSI, got {other:?}"),
        }
    }

    #[test]
    fn classify_esc_byte_recognizes_control_prefixes() {
        assert_eq!(classify_esc_byte(b'N'), ControlPrefix::Ss2);
        assert_eq!(classify_esc_byte(b'O'), ControlPrefix::Ss3);
        assert_eq!(classify_esc_byte(b'P'), ControlPrefix::String(StringControlKind::Dcs));
        assert_eq!(classify_esc_byte(b'X'), ControlPrefix::String(StringControlKind::Sos));
        assert_eq!(classify_esc_byte(b'['), ControlPrefix::Csi);
        assert_eq!(classify_esc_byte(b']'), ControlPrefix::String(StringControlKind::Osc));
        assert_eq!(classify_esc_byte(b'^'), ControlPrefix::String(StringControlKind::Pm));
        assert_eq!(classify_esc_byte(b'_'), ControlPrefix::String(StringControlKind::Apc));
        assert_eq!(classify_esc_byte(b'='), ControlPrefix::Esc(b'='));
        assert_eq!(classify_esc_byte(b'>'), ControlPrefix::Esc(b'>'));
        assert_eq!(classify_esc_byte(b'c'), ControlPrefix::Esc(b'c'));
    }

    #[test]
    fn classify_esc_prefixed_requires_esc_prefix() {
        assert_eq!(classify_esc_prefixed(b"\x1bN"), Some(ControlPrefix::Ss2));
        assert_eq!(classify_esc_prefixed(b"\x1bO"), Some(ControlPrefix::Ss3));
        assert_eq!(classify_esc_prefixed(b"\x1b["), Some(ControlPrefix::Csi));
        assert_eq!(classify_esc_prefixed(b"\x1b]"), Some(ControlPrefix::String(StringControlKind::Osc)));
        assert_eq!(classify_esc_prefixed(b"\x1b="), Some(ControlPrefix::Esc(b'=')));

        assert_eq!(classify_esc_prefixed(b""), None);
        assert_eq!(classify_esc_prefixed(b"x"), None);
        assert_eq!(classify_esc_prefixed(b"["), None);
    }

    #[test]
    fn classify_c1_recognizes_c1_controls() {
        assert_eq!(classify_c1(0x8e), Some(ControlPrefix::Ss2));
        assert_eq!(classify_c1(0x8f), Some(ControlPrefix::Ss3));
        assert_eq!(classify_c1(0x90), Some(ControlPrefix::String(StringControlKind::Dcs)));
        assert_eq!(classify_c1(0x98), Some(ControlPrefix::String(StringControlKind::Sos)));
        assert_eq!(classify_c1(0x9b), Some(ControlPrefix::Csi));
        assert_eq!(classify_c1(0x9d), Some(ControlPrefix::String(StringControlKind::Osc)));
        assert_eq!(classify_c1(0x9e), Some(ControlPrefix::String(StringControlKind::Pm)));
        assert_eq!(classify_c1(0x9f), Some(ControlPrefix::String(StringControlKind::Apc)));

        assert_eq!(classify_c1(0x80), None);
        assert_eq!(classify_c1(0x91), None);
        assert_eq!(classify_c1(b'['), None);
    }

    #[test]
    fn classify_control_prefix_is_c1_gated() {
        assert_eq!(classify_control_prefix(b"\x1b[?1h", false), Some(ControlPrefix::Csi));
        assert_eq!(classify_control_prefix(b"\x1b[?1h", true), Some(ControlPrefix::Csi));

        assert_eq!(classify_control_prefix(b"\x9b?1h", false), None);
        assert_eq!(classify_control_prefix(b"\x9b?1h", true), Some(ControlPrefix::Csi));

        assert_eq!(classify_control_prefix(b"\x9d0;title\x9c", false), None);
        assert_eq!(classify_control_prefix(b"\x9d0;title\x9c", true), Some(ControlPrefix::String(StringControlKind::Osc)));
    }

    #[test]
    fn scan_csi_accepts_esc_csi() {
        let (csi, consumed) = complete_csi(b"\x1b[1;5Axxx", false);

        assert_eq!(consumed, 6);
        assert_eq!(csi.raw, b"1;5A");
        assert_eq!(csi.params, b"1;5");
        assert_eq!(csi.intermediates, b"");
        assert_eq!(csi.final_byte, b'A');
        assert_eq!(csi.private_marker(), None);
        assert_eq!(csi.params_without_private_marker(), b"1;5");
    }

    #[test]
    fn scan_csi_accepts_private_marker() {
        let (csi, consumed) = complete_csi(b"\x1b[?1h", false);

        assert_eq!(consumed, 5);
        assert_eq!(csi.raw, b"?1h");
        assert_eq!(csi.params, b"?1");
        assert_eq!(csi.intermediates, b"");
        assert_eq!(csi.final_byte, b'h');
        assert_eq!(csi.private_marker(), Some(b'?'));
        assert_eq!(csi.params_without_private_marker(), b"1");
    }

    #[test]
    fn scan_csi_accepts_intermediate_bytes() {
        let (csi, consumed) = complete_csi(b"\x1b[1 q", false);

        assert_eq!(consumed, 5);
        assert_eq!(csi.raw, b"1 q");
        assert_eq!(csi.params, b"1");
        assert_eq!(csi.intermediates, b" ");
        assert_eq!(csi.final_byte, b'q');
    }

    #[test]
    fn scan_csi_reports_need_more() {
        assert_eq!(scan_csi(b"\x1b[", false), CsiScan::NeedMore);
        assert_eq!(scan_csi(b"\x1b[1", false), CsiScan::NeedMore);
        assert_eq!(scan_csi(b"\x1b[1;5", false), CsiScan::NeedMore);
        assert_eq!(scan_csi(b"\x1b[1 ", false), CsiScan::NeedMore);
    }

    #[test]
    fn scan_csi_reports_malformed_for_bad_bytes() {
        assert_eq!(scan_csi(b"\x1b[1\x80", false), CsiScan::Malformed { consumed: 4 });
        assert_eq!(scan_csi(b"x", false), CsiScan::Malformed { consumed: 1 });
        assert_eq!(scan_csi(b"", false), CsiScan::Malformed { consumed: 0 });
    }

    #[test]
    fn scan_csi_c1_is_config_gated() {
        assert_eq!(scan_csi(b"\x9b?1h", false), CsiScan::Malformed { consumed: 1 });

        let (csi, consumed) = complete_csi(b"\x9b?1h", true);
        assert_eq!(consumed, 4);
        assert_eq!(csi.raw, b"?1h");
        assert_eq!(csi.params, b"?1");
        assert_eq!(csi.final_byte, b'h');
    }

    #[test]
    fn split_csi_body_splits_valid_body() {
        let csi = split_csi_body(b"?1h").unwrap();
        assert_eq!(csi.raw, b"?1h");
        assert_eq!(csi.params, b"?1");
        assert_eq!(csi.intermediates, b"");
        assert_eq!(csi.final_byte, b'h');

        let csi = split_csi_body(b"1 q").unwrap();
        assert_eq!(csi.raw, b"1 q");
        assert_eq!(csi.params, b"1");
        assert_eq!(csi.intermediates, b" ");
        assert_eq!(csi.final_byte, b'q');
    }

    #[test]
    fn split_csi_body_rejects_incomplete_or_invalid_body() {
        assert_eq!(split_csi_body(b""), None);
        assert_eq!(split_csi_body(b"1;5"), None);
        assert_eq!(split_csi_body(b"1\x80"), None);
        assert_eq!(split_csi_body(b"1 A B"), None);
    }

    #[test]
    fn owned_csi_roundtrips_to_borrowed_view() {
        let csi = split_csi_body(b"?1h").unwrap();
        let owned = csi.to_owned();
        let borrowed = owned.as_csi();

        assert_eq!(borrowed.raw, b"?1h");
        assert_eq!(borrowed.params, b"?1");
        assert_eq!(borrowed.intermediates, b"");
        assert_eq!(borrowed.final_byte, b'h');
        assert_eq!(borrowed.private_marker(), Some(b'?'));
    }

    #[test]
    fn scan_string_control_accepts_osc_bel_and_st() {
        assert_eq!(
            scan_string_control(b"\x1b]0;title\x07rest", StringControlKind::Osc, false),
            StringScan::Complete { consumed: 10 },
        );
        assert_eq!(
            scan_string_control(b"\x1b]0;title\x1b\\rest", StringControlKind::Osc, false),
            StringScan::Complete { consumed: 11 },
        );
    }

    #[test]
    fn scan_string_control_accepts_st_for_non_osc_strings() {
        for kind in [
            StringControlKind::Dcs,
            StringControlKind::Sos,
            StringControlKind::Pm,
            StringControlKind::Apc,
        ] {
            assert_eq!(
                scan_string_control(b"\x1bPabc\x1b\\rest", kind, false),
                if kind == StringControlKind::Dcs {
                    StringScan::Complete { consumed: 7 }
                } else {
                    StringScan::Malformed { consumed: 1 }
                },
                "kind: {kind:?}",
            );
        }

        assert_eq!(
            scan_string_control(b"\x1bXabc\x1b\\rest", StringControlKind::Sos, false),
            StringScan::Complete { consumed: 7 },
        );
        assert_eq!(
            scan_string_control(b"\x1b^abc\x1b\\rest", StringControlKind::Pm, false),
            StringScan::Complete { consumed: 7 },
        );
        assert_eq!(
            scan_string_control(b"\x1b_abc\x1b\\rest", StringControlKind::Apc, false),
            StringScan::Complete { consumed: 7 },
        );
    }

    #[test]
    fn scan_string_control_only_osc_allows_bel_termination() {
        assert_eq!(
            scan_string_control(b"\x1b]abc\x07", StringControlKind::Osc, false),
            StringScan::Complete { consumed: 6 },
        );
        assert_eq!(
            scan_string_control(b"\x1bPabc\x07", StringControlKind::Dcs, false),
            StringScan::NeedMore,
        );
    }

    #[test]
    fn scan_string_control_reports_need_more() {
        assert_eq!(
            scan_string_control(b"\x1b]0;title", StringControlKind::Osc, false),
            StringScan::NeedMore,
        );
        assert_eq!(
            scan_string_control(b"\x1bPabc", StringControlKind::Dcs, false),
            StringScan::NeedMore,
        );
    }

    #[test]
    fn scan_string_control_reports_malformed_for_wrong_kind_or_prefix() {
        assert_eq!(
            scan_string_control(b"x", StringControlKind::Osc, false),
            StringScan::Malformed { consumed: 1 },
        );
        assert_eq!(
            scan_string_control(b"", StringControlKind::Osc, false),
            StringScan::Malformed { consumed: 0 },
        );
        assert_eq!(
            scan_string_control(b"\x1b]abc\x07", StringControlKind::Dcs, false),
            StringScan::Malformed { consumed: 1 },
        );
    }

    #[test]
    fn scan_string_control_c1_prefix_and_st_are_config_gated() {
        assert_eq!(
            scan_string_control(b"\x9d0;title\x9c", StringControlKind::Osc, false),
            StringScan::Malformed { consumed: 1 },
        );
        assert_eq!(
            scan_string_control(b"\x9d0;title\x9c", StringControlKind::Osc, true),
            StringScan::Complete { consumed: 9 },
        );
        assert_eq!(
            scan_string_control(b"\x1b]0;title\x9c", StringControlKind::Osc, false),
            StringScan::NeedMore,
        );
        assert_eq!(
            scan_string_control(b"\x1b]0;title\x9c", StringControlKind::Osc, true),
            StringScan::Complete { consumed: 10 },
        );
    }

    #[test]
    fn parse_simple_params_accepts_basic_semicolon_list() {
        assert_eq!(parse_simple_params(b""), Some(vec![]));
        assert_eq!(parse_simple_params(b"0"), Some(vec![0]));
        assert_eq!(parse_simple_params(b"1"), Some(vec![1]));
        assert_eq!(parse_simple_params(b"1;5"), Some(vec![1, 5]));
        assert_eq!(parse_simple_params(b"01;005"), Some(vec![1, 5]));
    }

    #[test]
    fn parse_simple_params_rejects_empty_or_non_numeric_params() {
        assert_eq!(parse_simple_params(b";1"), None);
        assert_eq!(parse_simple_params(b"1;"), None);
        assert_eq!(parse_simple_params(b"1;;2"), None);
        assert_eq!(parse_simple_params(b"1:2"), None);
        assert_eq!(parse_simple_params(b"?1"), None);
        assert_eq!(parse_simple_params(b"1;a"), None);
    }

    #[test]
    fn parse_private_simple_params_requires_marker_and_no_intermediates() {
        let csi = split_csi_body(b"?1;1049h").unwrap();
        assert_eq!(parse_private_simple_params(csi, b'?'), Some(vec![1, 1049]));
        assert_eq!(parse_private_simple_params(csi, b'>'), None);

        let csi = split_csi_body(b"?1 h").unwrap();
        assert_eq!(parse_private_simple_params(csi, b'?'), None);
    }

    #[test]
    fn read_u32_reads_digits_and_stops_at_first_non_digit() {
        let mut idx = 0;
        assert_eq!(read_u32(b"123x", &mut idx, u32::MAX), Some(123));
        assert_eq!(idx, 3);
    }

    #[test]
    fn read_u32_rejects_missing_digits_overflow_and_max_violation() {
        let mut idx = 0;
        assert_eq!(read_u32(b"x", &mut idx, u32::MAX), None);
        assert_eq!(idx, 0);

        let mut idx = 0;
        assert_eq!(read_u32(b"999999999999999999999", &mut idx, u32::MAX), None);

        let mut idx = 0;
        assert_eq!(read_u32(b"11", &mut idx, 10), None);
    }

    #[test]
    fn scanner_emits_simple_esc_events() {
        let mut scanner = ControlScanner::default();

        assert_eq!(scanner.push(b"\x1b="), vec![ControlEvent::Esc(b'=')]);
        assert_eq!(scanner.push(b"\x1b>"), vec![ControlEvent::Esc(b'>')]);
        assert_eq!(scanner.push(b"\x1bc"), vec![ControlEvent::Esc(b'c')]);
    }

    #[test]
    fn scanner_emits_ss2_and_ss3_events() {
        let mut scanner = ControlScanner::default();

        assert_eq!(scanner.push(b"\x1bN"), vec![ControlEvent::Ss2]);
        assert_eq!(scanner.push(b"\x1bO"), vec![ControlEvent::Ss3]);
    }

    #[test]
    fn scanner_handles_split_escape_prefix() {
        let mut scanner = ControlScanner::default();

        assert_eq!(scanner.push(b"\x1b"), vec![]);
        assert_eq!(scanner.push(b"="), vec![ControlEvent::Esc(b'=')]);
    }

    #[test]
    fn scanner_handles_split_csi() {
        let mut scanner = ControlScanner::default();

        assert_eq!(scanner.push(b"\x1b["), vec![]);

        assert_eq!(
            scanner.push(b"?1h"),
            vec![ControlEvent::Csi(OwnedCsiSeq {
                raw: b"?1h".to_vec(),
                params: b"?1".to_vec(),
                intermediates: vec![],
                final_byte: b'h',
            })],
        );
    }

    #[test]
    fn scanner_handles_csi_with_intermediates() {
        let mut scanner = ControlScanner::default();

        assert_eq!(
            scanner.push(b"\x1b[1 q"),
            vec![ControlEvent::Csi(OwnedCsiSeq {
                raw: b"1 q".to_vec(),
                params: b"1".to_vec(),
                intermediates: b" ".to_vec(),
                final_byte: b'q',
            })],
        );
    }

    #[test]
    fn scanner_reports_unknown_for_malformed_csi() {
        let mut scanner = ControlScanner::default();

        assert_eq!(scanner.push(b"\x1b[1\x80"), vec![ControlEvent::Unknown]);
    }

    #[test]
    fn scanner_limits_csi_size() {
        let mut scanner = ControlScanner::new(ControlScannerConfig {
            accept_c1: false,
            max_csi_bytes: 3,
        });

        assert_eq!(scanner.push(b"\x1b[1234"), vec![ControlEvent::Unknown]);
        assert_eq!(
            scanner.push(b"A"),
            vec![],
            "after overflow reset, later byte should be ground text",
        );
    }

    #[test]
    fn scanner_tracks_string_control_until_bel_or_st() {
        let mut scanner = ControlScanner::default();

        assert_eq!(scanner.push(b"\x1b]0;title"), vec![]);
        assert_eq!(scanner.push(b"\x07"), vec![ControlEvent::StringTerminated(StringControlKind::Osc)]);
        assert_eq!(scanner.push(b"\x1b]0;title"), vec![]);
        assert_eq!(scanner.push(b"\x1b\\"), vec![ControlEvent::StringTerminated(StringControlKind::Osc)]);
    }

    #[test]
    fn scanner_tracks_non_osc_strings_until_st_only() {
        let mut scanner = ControlScanner::default();

        assert_eq!(scanner.push(b"\x1bPabc\x07"), vec![]);
        assert_eq!(scanner.push(b"\x1b\\"), vec![ControlEvent::StringTerminated(StringControlKind::Dcs)]);
    }

    #[test]
    fn scanner_handles_sos_pm_apc_strings() {
        for (prefix, kind) in [
            (b"\x1bX".as_slice(), StringControlKind::Sos),
            (b"\x1b^".as_slice(), StringControlKind::Pm),
            (b"\x1b_".as_slice(), StringControlKind::Apc),
        ] {
            let mut scanner = ControlScanner::default();

            assert_eq!(scanner.push(prefix), vec![]);
            assert_eq!(scanner.push(b"payload"), vec![]);
            assert_eq!(scanner.push(b"\x1b\\"), vec![ControlEvent::StringTerminated(kind)], "kind: {kind:?}");
        }
    }

    #[test]
    fn scanner_c1_is_disabled_by_default() {
        let mut scanner = ControlScanner::default();

        assert_eq!(scanner.push(b"\x9b?1h"), vec![]);
        assert_eq!(scanner.push(b"\x8e"), vec![]);
        assert_eq!(scanner.push(b"\x8f"), vec![]);
        assert_eq!(scanner.push(b"\x9d0;title\x9c"), vec![]);
    }

    #[test]
    fn scanner_c1_can_be_enabled() {
        let mut scanner = ControlScanner::new(ControlScannerConfig {
            accept_c1: true,
            max_csi_bytes: DEFAULT_MAX_CSI_BYTES,
        });

        assert_eq!(
            scanner.push(b"\x9b?1h"),
            vec![ControlEvent::Csi(OwnedCsiSeq {
                raw: b"?1h".to_vec(),
                params: b"?1".to_vec(),
                intermediates: vec![],
                final_byte: b'h',
            })],
        );

        assert_eq!(scanner.push(b"\x8e"), vec![ControlEvent::Ss2]);
        assert_eq!(scanner.push(b"\x8f"), vec![ControlEvent::Ss3]);

        assert_eq!(scanner.push(b"\x9d0;title"), vec![]);
        assert_eq!(scanner.push(b"\x9c"), vec![ControlEvent::StringTerminated(StringControlKind::Osc)]);
    }

    #[test]
    fn scanner_c1_st_inside_esc_string_is_config_gated() {
        let mut scanner = ControlScanner::default();

        assert_eq!(scanner.push(b"\x1b]title\x9c"), vec![]);
        assert_eq!(scanner.push(b"\x1b\\"), vec![ControlEvent::StringTerminated(StringControlKind::Osc)]);

        let mut scanner = ControlScanner::new(ControlScannerConfig {
            accept_c1: true,
            max_csi_bytes: DEFAULT_MAX_CSI_BYTES,
        });

        assert_eq!(scanner.push(b"\x1b]title\x9c"), vec![ControlEvent::StringTerminated(StringControlKind::Osc)]);
    }
}
