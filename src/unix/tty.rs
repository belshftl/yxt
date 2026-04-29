// SPDX-License-Identifier: MIT

use std::ffi::{CStr};
use std::io::Error;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use libc::{termios, winsize};

#[derive(Debug)]
pub struct RawTerminal {
    fd: OwnedFd,
    old_termios: termios,
    restored: bool,
}

impl RawTerminal {
    pub fn enter(fd: impl AsFd) -> std::io::Result<Self> {
        let fd = dup_fd(fd)?;
        let raw_fd = fd.as_raw_fd();

        let old = tcgetattr_raw(raw_fd)?;
        let mut new = old;
        new.c_iflag &= !(
            libc::IGNBRK | libc::BRKINT | libc::PARMRK | libc::ISTRIP |
            libc::INLCR | libc::IGNCR | libc::ICRNL | libc::IXON
        );
        new.c_oflag &= !libc::OPOST;
        new.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN);
        new.c_cflag &= !(libc::CSIZE | libc::PARENB);
        new.c_cflag |= libc::CS8;
        new.c_cc[libc::VMIN] = 1;
        new.c_cc[libc::VTIME] = 0;

        tcsetattr_raw(raw_fd, &new)?;
        Ok(Self { fd, old_termios: old, restored: false })
    }

    pub fn restore(&mut self) -> std::io::Result<()> {
        if !self.restored {
            tcsetattr_raw(self.fd.as_raw_fd(), &self.old_termios)?;
            self.restored = true;
        }
        Ok(())
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        // swallow error
        let _ = self.restore();
    }
}

fn tcgetattr_raw(fd: RawFd) -> std::io::Result<termios> {
    let mut termios = MaybeUninit::<termios>::uninit();
    if unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) } < 0 {
        Err(Error::last_os_error())
    } else {
        // SAFETY: `termios` must have been initialized by a successful `tcgetattr()`
        Ok(unsafe { termios.assume_init() })
    }
}

fn tcsetattr_raw(fd: RawFd, termios: &termios) -> std::io::Result<()> {
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, termios) } < 0 {
        Err(Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn dup_fd(fd: impl AsFd) -> std::io::Result<OwnedFd> {
    let raw = unsafe { libc::dup(fd.as_fd().as_raw_fd()) };
    if raw < 0 {
        Err(Error::last_os_error())
    } else {
        // SAFETY: `raw` is a valid fd, ownership transfers from `raw` to the returned `OwnedFd`
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }
}

pub fn get_winsize(fd: impl AsFd) -> std::io::Result<winsize> {
    let mut ws = MaybeUninit::<winsize>::uninit();
    if unsafe { libc::ioctl(fd.as_fd().as_raw_fd(), libc::TIOCGWINSZ, ws.as_mut_ptr()) } < 0 {
        Err(Error::last_os_error())
    } else {
        // SAFETY: `ws` must have been initialized by a successful `TIOCGWINSZ` ioctl
        Ok(unsafe { ws.assume_init() })
    }
}

pub fn set_winsize(fd: impl AsFd, ws: &winsize) -> std::io::Result<()> {
    if unsafe { libc::ioctl(fd.as_fd().as_raw_fd(), libc::TIOCSWINSZ, ws) } < 0 {
        Err(Error::last_os_error())
    } else {
        Ok(())
    }
}

// SAFETY: intended to be called in a pre_exec() closure, so this must be async-signal-safe; since
// the closure's signature expects `std::io::Result<()>`, it's only safe to assume that
// constructing one via `last_os_error` / `from_raw_os_error` is safe
pub unsafe fn switch_to_ctty(fd: RawFd) -> std::io::Result<()> {
    // setsid(2) is listed as async-signal-safe in signal-safety(7)
    if unsafe { libc::setsid() } < 0 {
        return Err(Error::last_os_error());
    }

    // ioctl(TIOCSCTTY) is not POSIX so there's no standard to refer to, but it's likely safe to
    // assume it's async-signal-safe since it's the standard pty/session setup and the libc
    // function should typically be a thin wrapper over a syscall
    if unsafe { libc::ioctl(fd, libc::TIOCSCTTY, 0) } < 0 {
        return Err(Error::last_os_error());
    }

    Ok(())
}

pub struct PtyPair {
    pub master: OwnedFd,
    pub slave: OwnedFd,
    pub slave_name: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum PtyOpenError {
    #[error("posix_openpt failed: {0}")]
    PosixOpenpt(std::io::Error),

    #[error("grantpt failed: {0}")]
    Grantpt(std::io::Error),

    #[error("unlockpt failed: {0}")]
    Unlockpt(std::io::Error),

    #[error("ptsname failed: {0}")]
    Ptsname(std::io::Error),

    #[error("opening slave PTY failed: {0}")]
    OpenSlave(std::io::Error),
}

pub fn open_pty_pair() -> Result<PtyPair, PtyOpenError> {
    let master_raw_fd = unsafe {
        libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC)
    };
    if master_raw_fd < 0 {
        return Err(PtyOpenError::PosixOpenpt(Error::last_os_error()));
    }
    // SAFETY: `master_raw_fd` is a valid fd, ownership transfers from `master_raw_fd` to `master`
    let master = unsafe { OwnedFd::from_raw_fd(master_raw_fd) };

    if unsafe { libc::grantpt(master_raw_fd) } < 0 {
        return Err(PtyOpenError::Grantpt(Error::last_os_error()));
    }
    if unsafe { libc::unlockpt(master_raw_fd) } < 0 {
        return Err(PtyOpenError::Unlockpt(Error::last_os_error()));
    }

    // SAFETY: ptsname() returns a pointer to static storage, must not have a possibility of racing
    // with another call (maybe use a process-wide mutex later?)
    let name_ptr = unsafe { libc::ptsname(master_raw_fd) };
    if name_ptr.is_null() {
        return Err(PtyOpenError::Ptsname(Error::last_os_error()));
    }
    let name = unsafe { CStr::from_ptr(name_ptr) }.to_owned();

    let slave_raw_fd = unsafe {
        libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC)
    };
    if slave_raw_fd < 0 {
        return Err(PtyOpenError::OpenSlave(Error::last_os_error()));
    }
    // SAFETY: `slave_raw_fd` is a valid fd, ownership transfers from `slave_raw_fd` to `slave`
    let slave = unsafe { OwnedFd::from_raw_fd(slave_raw_fd) };

    let slave_name = std::ffi::OsStr::from_bytes(name.to_bytes()).into();
    Ok(PtyPair { master, slave, slave_name })
}
