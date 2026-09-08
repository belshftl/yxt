// SPDX-FileCopyrightText: 2026 belshftl
// SPDX-License-Identifier: MIT

use std::collections::HashMap;

use crate::config::{ast::Span, options::Options};

// ================================================================================================
// keys/mods
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharPair {
    pub unshifted: char,
    pub shifted: char,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Esc,
    Enter,
    Tab,
    Backspace,

    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,

    Arrow(Direction),

    Function(u8), // f1..f35

    Keypad(KeypadKey),

    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    Menu,

    Media(MediaKey),

    ModifierKey(ModifierKey),
    IsoLevel3Shift,
    IsoLevel5Shift,
}

impl Key {
    pub fn is_legacy_reportable(self) -> bool {
        match self {
            Self::Esc
            | Self::Enter
            | Self::Tab
            | Self::Backspace
            | Self::Insert
            | Self::Delete
            | Self::Home
            | Self::End
            | Self::PageUp
            | Self::PageDown
            | Self::Arrow(_) => true,

            Self::Function(n) => (1..=12).contains(&n),

            // the numeric keypad is reportable with DECKPAM but the nav ones (numlock off)
            // only have kitty codepoints
            Self::Keypad(kp) => match kp {
                KeypadKey::Digit(_)
                | KeypadKey::Decimal
                | KeypadKey::Divide
                | KeypadKey::Multiply
                | KeypadKey::Subtract
                | KeypadKey::Add
                | KeypadKey::Enter
                | KeypadKey::Equal
                | KeypadKey::Separator
                | KeypadKey::Begin => true,

                KeypadKey::Left
                | KeypadKey::Right
                | KeypadKey::Up
                | KeypadKey::Down
                | KeypadKey::PageUp
                | KeypadKey::PageDown
                | KeypadKey::Home
                | KeypadKey::End
                | KeypadKey::Insert
                | KeypadKey::Delete => false,
            },

            Self::CapsLock
            | Self::ScrollLock
            | Self::NumLock
            | Self::PrintScreen
            | Self::Pause
            | Self::Menu
            | Self::Media(_)
            | Self::ModifierKey(_)
            | Self::IsoLevel3Shift
            | Self::IsoLevel5Shift => false,
        }
    }

    pub fn required_protocol(self) -> Protocol {
        if self.is_legacy_reportable() {
            Protocol::Legacy
        } else {
            Protocol::Kitty
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeypadKey {
    Digit(u8), // 0..9
    Decimal,
    Divide,
    Multiply,
    Subtract,
    Add,
    Enter,
    Equal,
    Separator,
    Begin,

    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Insert,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKey {
    Play,
    Pause,
    PlayPause,
    Reverse,
    Stop,
    FastForward,
    Rewind,
    TrackNext,
    TrackPrevious,
    Record,
    LowerVolume,
    RaiseVolume,
    MuteVolume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierKey {
    LeftShift,
    LeftCtrl,
    LeftAlt,
    LeftSuper,
    LeftHyper,
    LeftMeta,

    RightShift,
    RightCtrl,
    RightAlt,
    RightSuper,
    RightHyper,
    RightMeta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mods(u16);

impl Mods {
    pub const EMPTY: Self = Self(0);

    pub const SHIFT: Self = Self(1 << 0);
    pub const ALT: Self = Self(1 << 1);
    pub const CTRL: Self = Self(1 << 2);
    pub const SUPER: Self = Self(1 << 3);
    pub const HYPER: Self = Self(1 << 4);
    pub const META: Self = Self(1 << 5);

    pub const KITTY_IGNORED_LOCK_BITS: u16 = (1 << 6) | (1 << 7); // caps_lock, num_lock

    pub fn raw(self) -> u16 {
        self.0
    }

    pub fn required_protocol(self) -> Protocol {
        if (self & (Self::SUPER | Self::HYPER)) == Self::EMPTY {
            Protocol::Legacy
        } else {
            Protocol::Kitty
        }
    }
}

impl std::ops::Not for Mods {
    type Output = Self;
    fn not(self) -> Self::Output {
        Self(!self.0)
    }
}

impl std::ops::BitOr for Mods {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Mods {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = *self | rhs;
    }
}

impl std::ops::BitAnd for Mods {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl std::ops::BitAndAssign for Mods {
    fn bitand_assign(&mut self, rhs: Self) {
        *self = *self & rhs;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventKind {
    Press,
    Repeat,
    Release,
}

// ================================================================================================
// protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Protocol {
    Legacy,
    Kitty,
}

impl Protocol {
    pub const ALL: &'static [Self] = &[Self::Legacy, Self::Kitty];

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "legacy" => Some(Self::Legacy),
            "kitty" => Some(Self::Kitty),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Kitty => "kitty",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolVerb {
    Want,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolRequest {
    pub verb: ProtocolVerb,
    pub protocol: Protocol,
}

impl Default for ProtocolRequest {
    fn default() -> Self {
        Self {
            verb: ProtocolVerb::Want,
            protocol: Protocol::Legacy,
        }
    }
}

// ================================================================================================
// groups
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupId(pub(crate) usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefineGroupError {
    Duplicate(String),
}

#[derive(Debug, Clone, Default)]
pub struct GroupTable {
    id_to_name: Vec<String>,
    name_to_id: HashMap<String, GroupId>,
}

impl GroupTable {
    pub fn define(&mut self, name: String) -> Result<GroupId, DefineGroupError> {
        if self.name_to_id.contains_key(&name) {
            return Err(DefineGroupError::Duplicate(name));
        }

        let id = GroupId(self.id_to_name.len());
        self.id_to_name.push(name.clone());
        self.name_to_id.insert(name, id);
        Ok(id)
    }

    pub fn lookup(&self, name: &str) -> Option<GroupId> {
        self.name_to_id.get(name).copied()
    }

    #[cfg(test)]
    pub fn name(&self, id: GroupId) -> &str {
        &self.id_to_name[id.0]
    }

    pub fn len(&self) -> usize {
        self.id_to_name.len()
    }
}

// ================================================================================================
// signal/command
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signal(pub libc::c_int);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSpec {
    Exec { argv: Vec<String> },
    Shell { command: String },
}

// ================================================================================================
// toggle mappings
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleOp {
    On,
    Off,
    Toggle,
}

impl ToggleOp {
    pub fn apply(self, enabled: bool) -> bool {
        match self {
            Self::On => true,
            Self::Off => false,
            Self::Toggle => !enabled,
        }
    }
}

// ================================================================================================
// concrete sources / payloads
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Utf8 {
        ch: char,
        mods: Mods,
        kind: KeyEventKind,
    },
    Key {
        key: Key,
        mods: Mods,
        kind: KeyEventKind,
    },
}

impl Token {
    pub fn press_utf8(ch: char, mods: Mods) -> Self {
        Self::Utf8 {
            ch,
            mods,
            kind: KeyEventKind::Press,
        }
    }

    pub fn press_key(key: Key, mods: Mods) -> Self {
        Self::Key {
            key,
            mods,
            kind: KeyEventKind::Press,
        }
    }

    pub fn kind(&self) -> KeyEventKind {
        match self {
            Self::Utf8 { kind, .. } | Self::Key { kind, .. } => *kind,
        }
    }

    pub fn with_kind(self, kind: KeyEventKind) -> Self {
        match self {
            Self::Utf8 { ch, mods, .. } => Self::Utf8 { ch, mods, kind },
            Self::Key { key, mods, .. } => Self::Key { key, mods, kind },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenPayload {
    pub actual_mods: Mods,
    pub logical_mods: Mods,
    pub kind: KeyEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payload {
    Token(TokenPayload),
}

impl Payload {
    #[allow(clippy::unnecessary_wraps)]
    pub fn token(self) -> Option<TokenPayload> {
        match self {
            Self::Token(payload) => Some(payload),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadKind {
    Token,
}

// ================================================================================================
// source/target patterns
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyPattern {
    Named(Key),
    CharPair(CharPair),
}

impl KeyPattern {
    pub fn required_protocol(&self, mods: Mods) -> Protocol {
        let legacy = match self {
            Self::Named(key) => legacy_reports_key(*key, mods),
            Self::CharPair(pair) => legacy_reports_char_pair(*pair, mods),
        };
        if legacy {
            Protocol::Legacy
        } else {
            Protocol::Kitty
        }
    }
}

fn legacy_reports_key(key: Key, mods: Mods) -> bool {
    if !key.is_legacy_reportable() {
        return false;
    }

    // the mod bitfield on csi/vt sequences
    let csi_mods = Mods::SHIFT | Mods::ALT | Mods::CTRL | Mods::META;

    let carries = match key {
        // begin is the one keypad key reported as csi rather than a bare ss3
        Key::Keypad(KeypadKey::Begin) => csi_mods,
        // the rest of the keypad has no modifier form at all, and neither does esc, which can't
        // even carry an alt prefix since ESC ESC is indistinguishable from a lone esc
        Key::Esc | Key::Keypad(_) => Mods::EMPTY,
        // the other bare c0 bytes carry no modifier of their own but do take an alt prefix
        Key::Enter | Key::Tab | Key::Backspace => Mods::ALT,
        _ => csi_mods,
    };

    (mods & !carries) == Mods::EMPTY
}

fn legacy_reports_char_pair(pair: CharPair, mods: Mods) -> bool {
    if (mods & Mods::SHIFT) == Mods::EMPTY {
        legacy_reports_char(pair.unshifted, mods)
    } else {
        // shift isn't even a modifier under legacy, all that changes is the shifted side of the
        // pair standing in for the unshifted one, so the two must differ
        pair.unshifted != pair.shifted && legacy_reports_char(pair.shifted, mods & !Mods::SHIFT)
    }
}

fn legacy_reports_char(ch: char, mods: Mods) -> bool {
    // alt is only an esc prefix, so it doesn't change what the rest of the sequence can express
    let alt = (mods & Mods::ALT) != Mods::EMPTY;

    if (mods & !Mods::ALT) == Mods::CTRL {
        // ctrl turns the character into a c0 byte, and no such byte introduces a sequence, so the
        // esc prefix stays unambiguous here
        legacy_reports_ctrl_char(ch)
    } else if (mods & !Mods::ALT) == Mods::EMPTY {
        // c0 or del always decodes as the corresponding named key, not as text
        ch >= ' ' && ch != '\u{7f}' && !(alt && esc_ch_starts_sequence(ch))
    } else {
        // super/hyper/meta can't be encoded for text
        false
    }
}

fn esc_ch_starts_sequence(ch: char) -> bool {
    // mirrors `term::control::classify_esc_byte`
    // SS2, SS3, DCS, SOS, CSI, OSC, PM, APC
    matches!(ch, 'N' | 'O' | 'P' | 'X' | '[' | ']' | '^' | '_')
}

fn legacy_reports_ctrl_char(ch: char) -> bool {
    // 0x08 backspace (ctrl+h), 0x09 tab (ctrl+i), 0x0a and 0x0d enter (ctrl+j, ctrl+m),
    // 0x1b esc (ctrl+[)
    // ctrl+space makes it as nul
    matches!(ch, ' ' | 'a'..='g' | 'k' | 'l' | 'n'..='z' | '\\' | ']' | '^' | '_')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModsPattern {
    Any,
    AnyOf(Vec<Mods>),
}

impl ModsPattern {
    pub fn matches(&self, mods: Mods) -> bool {
        match self {
            Self::Any => true,
            Self::AnyOf(v) => v.contains(&mods),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenPattern {
    Key { key: KeyPattern, mods: ModsPattern },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InheritToken {
    Key { key: KeyPattern },
}

impl InheritToken {
    pub fn to_token(&self, payload: TokenPayload) -> Token {
        match self {
            Self::Key { key } => match *key {
                KeyPattern::Named(key) => Token::Key {
                    key,
                    mods: payload.actual_mods,
                    kind: payload.kind,
                },
                KeyPattern::CharPair(pair) => {
                    let ch = if (payload.logical_mods & Mods::SHIFT) == Mods::EMPTY {
                        pair.unshifted
                    } else {
                        pair.shifted
                    };
                    Token::Utf8 {
                        ch,
                        mods: payload.actual_mods,
                        kind: payload.kind,
                    }
                }
            },
        }
    }
}

// ================================================================================================
// entity types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Signal(Signal),
    Sockdata(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Command(CommandSpec),
    ToggleMappings(ToggleOp),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Event(Event),
    Token(TokenPattern),
    Group(GroupId),
}

impl Source {
    pub fn provides_payload(&self) -> Option<PayloadKind> {
        match self {
            Self::Token(_) => Some(PayloadKind::Token),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Token(Token),
    InheritToken(InheritToken),
    Group(GroupId),
    Action(Action),
}

impl Target {
    pub fn requires_payload(&self) -> Option<PayloadKind> {
        match self {
            Target::InheritToken(_) => Some(PayloadKind::Token),
            _ => None,
        }
    }
}

// ================================================================================================
// other primary config concepts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MappingAttrs {
    pub passthrough: bool,
    pub always: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub from: Source,
    pub to: Target,
    pub attrs: MappingAttrs,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub command: CommandSpec,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub options: Options,
    pub protocol: ProtocolRequest,
    pub groups: GroupTable,
    pub mappings: Vec<Mapping>,
    pub services: Vec<Service>,
}
