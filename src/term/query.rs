// SPDX-FileCopyrightText: 2026 belshftl
// SPDX-License-Identifier: MIT

use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use crate::runtime::io::{ReadResult, read, write_all_until};
use crate::term::control::{ControlEvent, ControlScanner, CsiSeq, parse_simple_params};
use crate::unix::fd::{ReadyFds, SelectFds, select};

// `CSI ? u`       kitty keyboard flags, answered as `CSI ? flags u`
// `CSI ? Ps $ p`  DECRQM, answered as `CSI ? Ps ; Pv $ y`
// `CSI c`         DA1, answered as `CSI ? ... c`, which is used as an ending sentinel
const QUERY: &[u8] = b"\x1b[?u\x1b[?1$p\x1b[?66$p\x1b[?47$p\x1b[?1047$p\x1b[?1049$p\x1b[c";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueriedTermMode {
    pub decckm: Option<bool>,
    pub deckpam: Option<bool>,
    pub alt_screen: Option<bool>,
    pub kitty_flags: Option<u8>,
    pub complete: bool,
}

impl QueriedTermMode {
    fn apply(&mut self, csi: CsiSeq<'_>) {
        if csi.private_marker() != Some(b'?') {
            return;
        }
        match (csi.intermediates, csi.final_byte) {
            (b"", b'c') => self.complete = true,
            (b"", b'u') => self.apply_kitty(csi),
            (b"$", b'y') => self.apply_decrpm(csi),
            _ => {}
        }
    }

    fn apply_kitty(&mut self, csi: CsiSeq<'_>) {
        let Some(params) = parse_simple_params(csi.params_without_private_marker()) else {
            return;
        };
        let Some(flags) = params.first().copied().and_then(|f| u8::try_from(f).ok()) else {
            return;
        };
        self.kitty_flags = Some(flags);
    }

    fn apply_decrpm(&mut self, csi: CsiSeq<'_>) {
        let Some(params) = parse_simple_params(csi.params_without_private_marker()) else {
            return;
        };
        if params.len() != 2 {
            return;
        }
        // DECRPM: 1 set, 2 reset, 3 permanently set, 4 permanently reset, 0 unrecognized
        let state = match params[1] {
            1 | 3 => true,
            2 | 4 => false,
            _ => return,
        };
        match params[0] {
            1 => self.decckm = Some(state),
            66 => self.deckpam = Some(state),
            47 | 1047 | 1049 => self.alt_screen = Some(self.alt_screen.unwrap_or(false) || state),
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryFd {
    Term,
}

pub fn query_term_mode<F: AsFd>(term: &F, timeout: Duration) -> std::io::Result<QueriedTermMode> {
    let term = term.as_fd();
    let deadline = Instant::now() + timeout;
    let mut queried = QueriedTermMode::default();

    if !write_all_until(term, QUERY, deadline)? {
        return Ok(queried);
    }

    let mut scanner = ControlScanner::default();
    let mut buf = [0u8; 512];
    let fds = SelectFds {
        read: vec![(QueryFd::Term, term)],
        write: Vec::new(),
    };

    while !queried.complete {
        let Some(ready) = wait(&fds, deadline)? else {
            break;
        };
        if !ready.readable(QueryFd::Term) {
            break;
        }
        match read(&term, &mut buf)? {
            ReadResult::Success(n) => {
                for event in scanner.push(&buf[..n]) {
                    if let ControlEvent::Csi(csi) = event {
                        queried.apply(csi.as_csi());
                    }
                }
            }
            ReadResult::WouldBlock => {}
            ReadResult::Eof => break,
            ReadResult::EmptyInput => unreachable!(),
        }
    }

    Ok(queried)
}

fn wait(
    fds: &SelectFds<'_, QueryFd>,
    deadline: Instant,
) -> std::io::Result<Option<ReadyFds<QueryFd>>> {
    loop {
        let Some(timeout) = deadline
            .checked_duration_since(Instant::now())
            .filter(|rem| !rem.is_zero())
        else {
            return Ok(None);
        };
        match select(fds, Some(timeout)) {
            Ok(ready) if ready.read.is_empty() && ready.write.is_empty() => return Ok(None),
            Ok(ready) => return Ok(Some(ready)),
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
}
