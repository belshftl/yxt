// SPDX-License-Identifier: MIT

use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
struct SignalEntry {
    sig: libc::c_int,
    pending: Arc<AtomicBool>,
    id: signal_hook::SigId,
}

#[derive(Debug)]
pub struct SignalRegistry {
    read: UnixStream,
    write: UnixStream,
    entries: Vec<SignalEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum SignalError {
    #[error("signal {0} cannot be registered")]
    Forbidden(libc::c_int),

    #[error("signal {0} is already registered")]
    AlreadyRegistered(libc::c_int),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl SignalRegistry {
    pub fn new() -> io::Result<Self> {
        let (read, write) = UnixStream::pair()?;
        read.set_nonblocking(true)?;
        write.set_nonblocking(true)?;
        Ok(Self { read, write, entries: Vec::new() })
    }

    pub fn register(&mut self, sig: libc::c_int) -> Result<(), SignalError> {
        if signal_hook::consts::FORBIDDEN.contains(&sig) {
            return Err(SignalError::Forbidden(sig));
        }
        if self.entries.iter().any(|ent| ent.sig == sig) {
            return Err(SignalError::AlreadyRegistered(sig));
        }

        let pending = Arc::new(AtomicBool::new(false));
        let handler_pending = Arc::clone(&pending);
        let write_raw_fd = self.write.as_raw_fd();

        // SAFETY: register() closure must be async-signal-safe; `write_raw_fd` must be valid and
        // non-blocking and stay as such until the handler is unregistered
        let id = unsafe {
            signal_hook::low_level::register(sig, move || {
                handler_pending.store(true, Ordering::Relaxed);
                let byte = 0u8;
                _ = libc::write(write_raw_fd, &byte as *const _ as *const _, core::mem::size_of_val(&byte));
            })
        }.map_err(SignalError::Io)?;

        self.entries.push(SignalEntry { sig, pending, id });
        Ok(())
    }

    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.read.as_fd()
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.read.as_raw_fd()
    }

    pub fn drain(&mut self) -> io::Result<Vec<libc::c_int>> {
        self.drain_pipe()?;
        let mut out = Vec::new();
        for entry in &self.entries {
            if entry.pending.swap(false, Ordering::Relaxed) {
                out.push(entry.sig);
            }
        }
        Ok(out)
    }

    fn drain_pipe(&mut self) -> io::Result<()> {
        let mut buf = [0u8; 256];
        loop {
            match self.read.read(&mut buf) {
                Ok(0) => return Ok(()),
                Ok(_) => continue,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
    }
}

impl Drop for SignalRegistry {
    fn drop(&mut self) {
        for entry in self.entries.drain(..) {
            signal_hook::low_level::unregister(entry.id);
        }
    }
}
