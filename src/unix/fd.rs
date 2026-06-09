// SPDX-License-Identifier: MIT

use std::os::fd::{AsRawFd, BorrowedFd, RawFd};
use std::time::Duration;

pub struct NonblockingFd<'a> {
    fd: BorrowedFd<'a>,
    old_flags: libc::c_int,
    restored: bool,
}

impl<'a> NonblockingFd<'a> {
    pub fn new(fd: BorrowedFd<'a>) -> std::io::Result<Self> {
        let raw = fd.as_raw_fd();

        // SAFETY: `raw` comes from a valid `BorrowedFd`, so it is a valid open fd for the duration
        // of this call; `F_GETFL` takes no third argument
        let old_flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
        if old_flags < 0 {
            return Err(std::io::Error::last_os_error());
        }

        // SAFETY: `raw` comes from a valid `BorrowedFd`, so it is a valid open fd for the duration
        // of this call; the third argument is created from a valid bitmask obtained from `F_GETFL`
        if unsafe { libc::fcntl(raw, libc::F_SETFL, old_flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            fd,
            old_flags,
            restored: false,
        })
    }

    pub fn restore(&mut self) -> std::io::Result<()> {
        if self.restored {
            return Ok(());
        }

        let raw = self.fd.as_raw_fd();

        // SAFETY: `raw` comes from a valid `BorrowedFd`, so it is a valid open fd for the duration
        // of this call; `F_GETFL` takes no third argument
        let curr = unsafe { libc::fcntl(raw, libc::F_GETFL) };
        if curr < 0 {
            return Err(std::io::Error::last_os_error());
        }

        // restore only O_NONBLOCK bit
        let flags = (curr & !libc::O_NONBLOCK) | (self.old_flags & libc::O_NONBLOCK);

        // SAFETY: `raw` comes from a valid `BorrowedFd`, so it is a valid open fd for the duration
        // of this call; `flags` is based on the current status-flag bitmask
        if unsafe { libc::fcntl(raw, libc::F_SETFL, flags) } < 0 {
            return Err(std::io::Error::last_os_error());
        }

        self.restored = true;
        Ok(())
    }
}

impl Drop for NonblockingFd<'_> {
    fn drop(&mut self) {
        _ = self.restore();
    }
}

#[derive(Debug, Clone)]
pub struct SelectFds<'a, K: Copy + Eq> {
    pub read: Vec<(K, BorrowedFd<'a>)>,
    pub write: Vec<(K, BorrowedFd<'a>)>,
}

#[derive(Debug, Clone)]
pub struct ReadyFds<K: Copy + Eq> {
    pub read: Vec<K>,
    pub write: Vec<K>,
}

impl<K: Copy + Eq> ReadyFds<K> {
    pub fn empty() -> Self {
        Self {
            read: Vec::new(),
            write: Vec::new(),
        }
    }

    pub fn readable(&self, key: K) -> bool {
        self.read.contains(&key)
    }

    pub fn writable(&self, key: K) -> bool {
        self.write.contains(&key)
    }
}

pub fn select<K: Copy + Eq>(
    fds: &SelectFds<'_, K>,
    timeout: Option<Duration>,
) -> std::io::Result<ReadyFds<K>> {
    let mut rfds = fd_zero();
    let mut wfds = fd_zero();
    let maxfd = fd_set(&mut rfds, fds.read.iter().map(|(_, fd)| fd))?
        .max(fd_set(&mut wfds, fds.write.iter().map(|(_, fd)| fd))?);

    let mut tv = timeout.map(duration_to_timeval).transpose()?;
    let tv_ptr = match &mut tv {
        Some(tv) => std::ptr::from_mut::<libc::timeval>(tv),
        None => std::ptr::null_mut(),
    };

    // SAFETY: `rfds` and `wfds` are initialized `fd_set`s; `tv_ptr` is either null or points to
    // `tv`, which lives until `select` returns; `fd_set` rejects fds >= `FD_SETSIZE`, and `maxfd`
    // is `-1` only when no fds are supplied, yielding `nfds == 0`
    if unsafe {
        libc::select(
            maxfd + 1,
            &raw mut rfds,
            &raw mut wfds,
            std::ptr::null_mut(),
            tv_ptr,
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let mut ready_r = Vec::new();
    let mut ready_w = Vec::new();
    for &fd in &fds.read {
        let raw = fd.1.as_raw_fd();
        // SAFETY: `raw` comes from a valid `BorrowedFd`, so it is a valid open fd for the duration
        // of this call; it was previously checked by `fd_set()` to fit into `fd_set`, and `rfds` is
        // an initialized `fd_set` that was passed to a successful `select` call
        if unsafe { libc::FD_ISSET(raw, &raw const rfds) } {
            ready_r.push(fd.0);
        }
    }
    for &fd in &fds.write {
        let raw = fd.1.as_raw_fd();
        // SAFETY: `raw` comes from a valid `BorrowedFd`, so it is a valid open fd for the duration
        // of this call; it was previously checked by `fd_set()` to fit into `fd_set`, and `wfds` is
        // an initialized `fd_set` that was passed to a successful `select` call
        if unsafe { libc::FD_ISSET(raw, &raw const wfds) } {
            ready_w.push(fd.0);
        }
    }
    Ok(ReadyFds {
        read: ready_r,
        write: ready_w,
    })
}

fn fd_zero() -> libc::fd_set {
    // SAFETY: `FD_ZERO` fully initializes the `fd_set` pointed to by its argument; after that call,
    // assuming initialization is valid
    unsafe {
        let mut set = std::mem::MaybeUninit::<libc::fd_set>::uninit();
        libc::FD_ZERO(set.as_mut_ptr());
        set.assume_init()
    }
}

fn fd_set<'a, 'b>(
    set: &mut libc::fd_set,
    fds: impl IntoIterator<Item = &'b BorrowedFd<'a>>,
) -> std::io::Result<RawFd>
where
    'a: 'b,
{
    let mut maxfd = -1;
    for fd in fds {
        let raw = fd.as_raw_fd();
        if raw < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("fd {raw} is invalid (negative)"),
            ));
        }
        if raw
            >= libc::c_int::try_from(libc::FD_SETSIZE)
                .expect("libc::FD_SETSIZE should fit into a libc::c_int")
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("fd {raw} is too large for select(2)"),
            ));
        }
        // SAFETY: `set` points to an initialized `fd_set`; `raw` comes from a valid `BorrowedFd`
        // and has been checked to be less than `FD_SETSIZE`
        unsafe { libc::FD_SET(raw, set) };
        maxfd = maxfd.max(raw);
    }
    Ok(maxfd)
}

fn duration_to_timeval(d: Duration) -> std::io::Result<libc::timeval> {
    let mut sec = d.as_secs();
    let mut usec = d.subsec_nanos().div_ceil(1000);

    if usec == 1000000 {
        usec = 0;
        sec = sec.checked_add(1).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "duration is too large to represent as timeval(3type)",
            )
        })?;
    }

    let sec = libc::time_t::try_from(sec).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "duration is too large to represent as timeval(3type)",
        )
    })?;
    Ok(libc::timeval {
        tv_sec: sec,
        tv_usec: libc::suseconds_t::from(usec),
    })
}
