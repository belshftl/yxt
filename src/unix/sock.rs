// SPDX-FileCopyrightText: 2026 belshftl
// SPDX-License-Identifier: MIT

use anyhow::{Context as _, bail};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};

// unix(7):
// "When coding portable applications, keep in mind that some
// implementations have sun_path as short as 92 bytes."
// that includes the null terminator, so limit to 91 bytes
const MAX_UNIX_SOCKET_PATH_BYTES: usize = 91;

/// glibc's `BUFSIZ`, same as the read buffers; control messages are far shorter than this.
pub const MAX_DATAGRAM_BYTES: usize = 8192;

pub struct ControlSock {
    path: PathBuf,
    socket: UnixDatagram,
    max_datagram_size: usize,
}

impl ControlSock {
    pub fn bind(path: &Path, max_datagram_size: usize) -> anyhow::Result<Self> {
        let len = path.as_os_str().as_bytes().len();
        if len > MAX_UNIX_SOCKET_PATH_BYTES {
            bail!(
                "socket path '{}' is {len} bytes, over the {MAX_UNIX_SOCKET_PATH_BYTES} byte limit",
                path.display(),
            );
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating socket directory '{}'", parent.display()))?;
        }

        if let Err(e) = std::fs::remove_file(path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(e).with_context(|| format!("removing stale socket '{}'", path.display()));
        }

        let socket = UnixDatagram::bind(path)
            .with_context(|| format!("binding socket '{}'", path.display()))?;
        socket
            .set_nonblocking(true)
            .context("making the control socket nonblocking")?;

        Ok(Self {
            path: path.to_owned(),
            socket,
            max_datagram_size,
        })
    }

    pub fn recv(&self) -> anyhow::Result<Option<Vec<u8>>> {
        let mut buf = vec![0u8; self.max_datagram_size];
        match self.socket.recv(&mut buf) {
            Ok(n) => {
                buf.truncate(n);
                Ok(Some(buf))
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e).context("receiving a control socket datagram"),
        }
    }
}

impl Drop for ControlSock {
    fn drop(&mut self) {
        _ = std::fs::remove_file(&self.path);
    }
}

impl AsFd for ControlSock {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }
}

impl AsRawFd for ControlSock {
    fn as_raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }
}

#[cfg(not(target_os = "macos"))]
pub fn default_sock_path(prog_name: &str) -> anyhow::Result<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .context("XDG_RUNTIME_DIR is not set; pass `--sock` explicitly")?;
    Ok(PathBuf::from(dir)
        .join(prog_name)
        .join(format!("{}.sock", std::process::id())))
}

#[cfg(target_os = "macos")]
pub fn default_sock_path(prog_name: &str) -> anyhow::Result<PathBuf> {
    let dir = std::env::var_os("TMPDIR")
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR"))
        .context("neither TMPDIR nor XDG_RUNTIME_DIR are set; pass `--sock` explicitly")?;
    Ok(PathBuf::from(dir)
        .join(prog_name)
        .join(format!("{}.sock", std::process::id())))
}
