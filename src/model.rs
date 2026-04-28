// SPDX-License-Identifier: MIT

use std::collections::HashMap;

use crate::config::{line::Span, options::Options};

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

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl std::ops::BitOr for Mods {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitAnd for Mods {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupId(pub(crate) usize);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Signal(pub libc::c_int);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Event {
    Signal(Signal),
    Sockdata(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Token {
    Utf8 {
        ch: char,
        mods: Mods,
    },
    Key {
        key: Key,
        mods: Mods,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    Shell(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    Event(Event),
    Token(Token),
    Group(GroupId),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    Token(Token),
    Group(GroupId),
    Action(Action),
}

#[derive(Debug, Clone)]
pub struct Mapping {
    pub from: Source,
    pub to: Target,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Service {
    pub name: Option<String>,
    // TODO: the rest of the struct
}

#[derive(Debug, Clone)]
pub struct Config {
    pub options: Options,
    pub groups: GroupTable,
    pub mappings: Vec<Mapping>,
    pub services: Vec<Service>,
}
