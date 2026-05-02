// SPDX-License-Identifier: MIT

use std::os::fd::{AsRawFd, BorrowedFd, RawFd};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct SelectFds<'a> {
    pub read: Vec<BorrowedFd<'a>>,
    pub write: Vec<BorrowedFd<'a>>,
}

#[derive(Debug, Clone)]
pub struct ReadyFds<'a> {
    pub read: Vec<BorrowedFd<'a>>,
    pub write: Vec<BorrowedFd<'a>>,
}

impl<'a> ReadyFds<'a> {
    pub fn readable(&self, fd: BorrowedFd<'a>) -> bool {
        let needle = fd.as_raw_fd();
        self.read.iter().any(|fd| fd.as_raw_fd() == needle)
    }

    pub fn writable(&self, fd: BorrowedFd<'a>) -> bool {
        let needle = fd.as_raw_fd();
        self.write.iter().any(|fd| fd.as_raw_fd() == needle)
    }
}

pub fn select<'a>(fds: SelectFds<'a>, timeout: Option<Duration>) -> std::io::Result<ReadyFds<'a>> {
    let mut rfds = fd_zero();
    let mut wfds = fd_zero();
    let maxfd = fd_set(&mut rfds, fds.read.iter())?.max(fd_set(&mut wfds, fds.write.iter())?);

    let mut tv = timeout.map(duration_to_timeval).transpose()?;
    let tv_ptr = match &mut tv {
        Some(tv) => tv as *mut libc::timeval,
        None => std::ptr::null_mut(),
    };

    if unsafe { libc::select(maxfd + 1, &mut rfds, &mut wfds, std::ptr::null_mut(), tv_ptr) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut ready_r = Vec::new();
    let mut ready_w = Vec::new();
    for &fd in fds.read.iter() {
        let raw = fd.as_raw_fd();
        if raw < 0 {
            panic!("BorrowedFd.as_raw_fd() returned bad fd");
        }
        if unsafe { libc::FD_ISSET(raw, &rfds) } {
            ready_r.push(fd);
        }
    }
    for &fd in fds.write.iter() {
        let raw = fd.as_raw_fd();
        if raw < 0 {
            panic!("BorrowedFd.as_raw_fd() returned bad fd");
        }
        if unsafe { libc::FD_ISSET(raw, &wfds) } {
            ready_w.push(fd);
        }
    }
    Ok(ReadyFds {
        read: ready_r,
        write: ready_w
    })
}

fn fd_zero() -> libc::fd_set {
    unsafe {
        let mut set = std::mem::MaybeUninit::<libc::fd_set>::uninit();
        libc::FD_ZERO(set.as_mut_ptr());
        set.assume_init()
    }
}

fn fd_set<'a, 'b>(set: &mut libc::fd_set, fds: impl IntoIterator<Item = &'b BorrowedFd<'a>>) -> std::io::Result<RawFd> where 'a: 'b {
    let mut maxfd = -1;
    for fd in fds {
        let raw = fd.as_raw_fd();
        if raw < 0 {
            panic!("BorrowedFd.as_raw_fd() returned bad fd");
        }
        if raw >= libc::FD_SETSIZE as libc::c_int {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("fd {raw} is too large for select(2)")
            ));
        }
        unsafe {
            libc::FD_SET(raw, set);
        }
        maxfd = maxfd.max(raw);
    }
    Ok(maxfd)
}

fn duration_to_timeval(d: Duration) -> std::io::Result<libc::timeval> {
    let mut sec = d.as_secs();
    let mut usec = (d.subsec_nanos() + 999) / 1000;

    if usec == 1000000 {
        usec = 0;
        sec = sec.checked_add(1).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "duration is too large to represent as timeval",
            )
        })?;
    }

    let max = libc::time_t::MAX as u64;
    if sec > max {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "duration is too large to represent as timeval",
        ));
    }

    Ok(libc::timeval {
        tv_sec: sec as libc::time_t,
        tv_usec: usec as libc::suseconds_t,
    })
}
