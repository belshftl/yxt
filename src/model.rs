// SPDX-License-Identifier: MIT

use std::{collections::HashMap};

use crate::config::{line::Span, options::Options};

// ================================================================================================
// protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Protocol {
    Legacy = 0,
    Kitty = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProtocolPolicy {
    pub exact: Option<Protocol>,
    pub minimum: Protocol,
}

impl Default for ProtocolPolicy {
    fn default() -> Self {
        Self {
            exact: None,
            minimum: Protocol::Legacy,
        }
    }
}

// ================================================================================================
// keys/mods
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CharPair {
    pub unshifted: char,
    pub shifted: char,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyEventKind {
    Press,
    Repeat,
    Release,
}

// ================================================================================================
// groups
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupId(pub(crate) usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefineGroupError {
    Duplicate(String),
}

#[derive(Debug, Default, Clone)]
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

    pub fn name(&self, id: GroupId) -> &str {
        &self.id_to_name[id.0]
    }

    pub fn len(&self) -> usize {
        self.id_to_name.len()
    }
}

// ================================================================================================
// signal/command
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Signal(pub libc::c_int);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CommandSpec {
    Exec {
        argv: Vec<String>,
    },
    Shell {
        command: String,
    },
}

// ================================================================================================
// concrete sources / payloads
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenPayload {
    pub actual_mods: Mods,
    pub logical_mods: Mods,
    pub kind: KeyEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Payload {
    Token(TokenPayload),
}

impl Payload {
    pub fn token(self) -> Option<TokenPayload> {
        match self {
            Self::Token(payload) => Some(payload),
        }
    }

    pub fn token_kind(self) -> Option<KeyEventKind> {
        self.token().map(|p| p.kind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadKind {
    Token,
}

// ================================================================================================
// source/target patterns
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyPattern {
    Named(Key),
    CharPair(CharPair),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TokenPattern {
    Key {
        key: KeyPattern,
        mods: ModsPattern,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InheritToken {
    Key {
        key: KeyPattern,
    },
}

impl InheritToken {
    pub fn to_token(&self, payload: TokenPayload) -> Token {
        match self {
            Self::Key { key } => match *key {
                KeyPattern::Named(key) => Token::Key {
                    key: key,
                    mods: payload.actual_mods,
                    kind: payload.kind,
                },
                KeyPattern::CharPair(pair) => {
                    let ch = if (payload.logical_mods & Mods::SHIFT) != Mods::EMPTY {
                        pair.shifted
                    } else {
                        pair.unshifted
                    };
                    Token::Utf8 {
                        ch,
                        mods: payload.actual_mods,
                        kind: payload.kind,
                    }
                }
            }
        }
    }
}

// ================================================================================================
// entity types
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Event {
    Signal(Signal),
    Sockdata(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    Command(CommandSpec),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub from: Source,
    pub to: Target,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub command: CommandSpec,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub protocol: ProtocolPolicy,
    pub options: Options,
    pub groups: GroupTable,
    pub mappings: Vec<Mapping>,
    pub services: Vec<Service>,
}
