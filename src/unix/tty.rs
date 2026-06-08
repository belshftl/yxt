// SPDX-License-Identifier: MIT

use libc::{termios, winsize};
use std::ffi::CStr;
use std::io::Error;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};

#[derive(Debug)]
pub struct RawTerminal {
    fd: OwnedFd,
    old_termios: termios,
    restored: bool,
}

impl RawTerminal {
    pub fn enter<F: AsFd + ?Sized>(fd: &F) -> std::io::Result<Self> {
        let fd = dup_fd(fd)?;
        let raw_fd = fd.as_raw_fd();

        let old = tcgetattr_raw(raw_fd)?;
        let mut new = old;
        new.c_iflag &= !(libc::IGNBRK
            | libc::BRKINT
            | libc::PARMRK
            | libc::ISTRIP
            | libc::INLCR
            | libc::IGNCR
            | libc::ICRNL
            | libc::IXON);
        new.c_oflag &= !libc::OPOST;
        new.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN);
        new.c_cflag &= !(libc::CSIZE | libc::PARENB);
        new.c_cflag |= libc::CS8;
        new.c_cc[libc::VMIN] = 1;
        new.c_cc[libc::VTIME] = 0;

        tcsetattr_raw(raw_fd, &new)?;
        Ok(Self {
            fd,
            old_termios: old,
            restored: false,
        })
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

    // SAFETY: `termios.as_mut_ptr()` is valid for writes of `termios` and gets initialized by
    // `tcgetattr` on success; invalid `fd` is reported as a syscall error
    if unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) } < 0 {
        Err(Error::last_os_error())
    } else {
        // SAFETY: `termios` has been initialized by a successful `tcgetattr`
        Ok(unsafe { termios.assume_init() })
    }
}

fn tcsetattr_raw(fd: RawFd, termios: &termios) -> std::io::Result<()> {
    // SAFETY: `termios` points to a valid initialized `termios` and does not get retained by
    // `tcsetattr`; invalid `fd` is reported as a syscall error
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, termios) } < 0 {
        Err(Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn dup_fd<F: AsFd + ?Sized>(fd: &F) -> std::io::Result<OwnedFd> {
    // SAFETY: `fd.as_fd().as_raw_fd()` is a valid borrowed fd for the duration of this call, `dup`
    // does not take ownership of it
    let raw = unsafe { libc::dup(fd.as_fd().as_raw_fd()) };
    if raw < 0 {
        Err(Error::last_os_error())
    } else {
        // SAFETY: `raw` is a valid fd returned by `dup` whose ownership now transfers to the
        // returned `OwnedFd`
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }
}

pub fn get_winsize<F: AsFd + ?Sized>(fd: &F) -> std::io::Result<winsize> {
    let mut ws = MaybeUninit::<winsize>::uninit();

    // SAFETY: `ws.as_mut_ptr()` is valid for writes of `winsize` and gets initialized by
    // `ioctl(TIOCGWINSZ)` on success; `fd.as_fd().as_raw_fd()` is a valid borrowed fd for the
    // duration of this call, the `ioctl` does not take ownership of it
    if unsafe { libc::ioctl(fd.as_fd().as_raw_fd(), libc::TIOCGWINSZ, ws.as_mut_ptr()) } < 0 {
        Err(Error::last_os_error())
    } else {
        // SAFETY: `ws` has been initialized by a successful `ioctl(TIOCGWINSZ)`
        Ok(unsafe { ws.assume_init() })
    }
}

pub fn set_winsize<F: AsFd + ?Sized>(fd: &F, ws: &winsize) -> std::io::Result<()> {
    // SAFETY: `fd.as_fd().as_raw_fd()` is a valid borrowed fd for the duration of this call; `ws`
    // points to a valid initialized `winsize` and does not get retained by the `ioctl` call
    if unsafe { libc::ioctl(fd.as_fd().as_raw_fd(), libc::TIOCSWINSZ, ws) } < 0 {
        Err(Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Calls `setsid(2)` and switches the controlling terminal to the provided `fd`.
/// Intended to be called in a `pre_exec()` closure after fork and before exec to attach
/// the child to the slave side of a PTY.
///
/// # Safety
///
/// The call may fail after `setsid(2)`, in which case the process will remain in the new
/// session / process group, so errors must be treated as having potentially mutated process state.
pub unsafe fn switch_to_ctty(fd: RawFd) -> std::io::Result<()> {
    // SAFETY remark: there is no formal guarantee or promise that `Error::last_os_error` is
    // async-signal-safe; however, since the closure's signature expects a return of
    // `std::io::Result<()>`, we have no choice but to make the assumption that constructing one via
    // `last_os_error` / `from_raw_os_error` is safe

    // SAFETY: `setsid(2)` is listed as async-signal-safe in `signal-safety(7)`; it takes no inputs
    // and has no rust-side safety requirements
    if unsafe { libc::setsid() } < 0 {
        return Err(Error::last_os_error());
    }

    // SAFETY: `ioctl(TIOCSCTTY)` is not POSIX so there's no standard to refer to; we assume here
    // that it is async-signal-safe since it's the standard pty/session setup and the libc function
    // is typically a small wrapper over a raw syscall. invalid `fd` is reported as a syscall error
    // additionally, if this call fails, the process will remain `setsid`'d, so callers must treat
    // errors with caution
    if unsafe { libc::ioctl(fd, libc::TIOCSCTTY, 0 as libc::c_int) } < 0 {
        return Err(Error::last_os_error());
    }

    Ok(())
}

pub struct PtyPair {
    pub master: OwnedFd,
    pub slave: OwnedFd,
}

#[derive(Debug, thiserror::Error)]
pub enum PtyOpenError {
    #[error("posix_openpt: {0}")]
    PosixOpenpt(std::io::Error),

    #[error("grantpt: {0}")]
    Grantpt(std::io::Error),

    #[error("unlockpt: {0}")]
    Unlockpt(std::io::Error),

    #[error("ptsname: {0}")]
    Ptsname(std::io::Error),

    #[error("open() slave PTY: {0}")]
    OpenSlave(std::io::Error),
}

pub fn open_pty_pair() -> Result<PtyPair, PtyOpenError> {
    // SAFETY: `posix_openpt` takes no pointer arguments, called with valid flag bits here
    let master_raw_fd =
        unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
    if master_raw_fd < 0 {
        return Err(PtyOpenError::PosixOpenpt(Error::last_os_error()));
    }

    // SAFETY: `master_raw_fd` was returned by successful `posix_openpt`, is open, and is not owned
    // elsewhere; ownership transfers from `master_raw_fd` to `master`
    let master = unsafe { OwnedFd::from_raw_fd(master_raw_fd) };

    // SAFETY: `master.as_raw_fd()` is a valid fd from a successful `posix_openpt`; `grantpt` does
    // not take ownership of it
    if unsafe { libc::grantpt(master.as_raw_fd()) } < 0 {
        return Err(PtyOpenError::Grantpt(Error::last_os_error()));
    }

    // SAFETY: `master.as_raw_fd()` is a valid fd from a successful `posix_openpt` + `grantpt`;
    // `unlockpt` does not take ownership of it
    if unsafe { libc::unlockpt(master.as_raw_fd()) } < 0 {
        return Err(PtyOpenError::Unlockpt(Error::last_os_error()));
    }

    // a bit ugly, but `std::cmp::min` is still marked only `const: unstable`
    // cap at 2048 to prevent large PATH_MAX from allocating a lot on the stack; technically, this
    // can disallow valid filepaths longer than 2047 characters but, like, come on
    // realistically it's going to be something like `/dev/pts/N`
    const BUFSIZE: usize = if (libc::PATH_MAX as usize) < 2048usize {
        libc::PATH_MAX as usize
    } else {
        2048usize
    };
    let mut buf = [MaybeUninit::<libc::c_char>::uninit(); BUFSIZE];

    // SAFETY: `buf` is valid for writes of up to `buf.len()` bytes and does not get retained by
    // `ptsname_r`, and `master.as_raw_fd()` is a valid fd from a successful `posix_openpt` +
    // `grantpt` + `unlockpt`
    let rv = unsafe {
        libc::ptsname_r(
            master.as_raw_fd(),
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len(),
        )
    };
    if rv != 0 {
        // the usual style here is `< 0` but the manpage says it returns "an error number", not -1
        return Err(PtyOpenError::Ptsname(Error::from_raw_os_error(rv)));
    }

    // SAFETY: earlier successful `ptsname_r` wrote a valid C string into `buf`
    let name = unsafe { CStr::from_ptr(buf.as_ptr().cast::<libc::c_char>()) };

    // SAFETY: `name.as_ptr()` is a valid NUL-terminated pathname returned by `ptsname_r`
    let slave_raw_fd = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if slave_raw_fd < 0 {
        return Err(PtyOpenError::OpenSlave(Error::last_os_error()));
    }

    // SAFETY: `slave_raw_fd` is a valid fd, ownership transfers from `slave_raw_fd` to `slave`
    let slave = unsafe { OwnedFd::from_raw_fd(slave_raw_fd) };

    Ok(PtyPair { master, slave })
}
