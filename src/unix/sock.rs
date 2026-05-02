// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixDatagram;

// unix(7):
// "When coding portable applications, keep in mind that some
// implementations have sun_path as short as 92 bytes."
// that includes the null terminator, so limit to 91 bytes
const MAX_UNIX_SOCKET_PATH_BYTES: usize = 91;

#[derive(Debug, thiserror::Error)]
pub enum ControlSockError {
    #[error("XDG_RUNTIME_DIR is not set; set `sock` explicitly")]
    NoRuntimeDir,

    #[error("command {0:?} has no basename")]
    CommandHasNoBasename(std::ffi::OsString),

    #[error("socket path '{0}' is too long (max {MAX_UNIX_SOCKET_PATH_BYTES} bytes)")]
    PathTooLong(PathBuf),

    #[error("failed to create socket directory '{path}': {source}")]
    CreateDir { path: PathBuf, #[source] source: std::io::Error },

    #[error("failed to remove stale socket '{path}': {source}")]
    RemoveStale { path: PathBuf, #[source] source: std::io::Error },

    #[error("failed to bind socket '{path}': {source}")]
    Bind { path: PathBuf, #[source] source: std::io::Error },

    #[error("failed to configure socket: {0}")]
    Configure(#[source] std::io::Error),

    #[error("failed to receive socket datagram: {0}")]
    Recv(#[source] std::io::Error),

    #[error("socket datagram is not valid UTF-8: {0}")]
    BadUtf8(#[source] std::str::Utf8Error),
}

pub struct ControlSock {
    path: PathBuf,
    socket: UnixDatagram,
    max_datagram_size: usize,
}

impl ControlSock {
    pub fn bind(path: &Path, max_datagram_size: usize) -> Result<Self, ControlSockError> {
        let len = path.as_os_str().as_bytes().len();
        if len > MAX_UNIX_SOCKET_PATH_BYTES {
            return Err(ControlSockError::PathTooLong(path.to_owned()));
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                ControlSockError::CreateDir { path: parent.to_owned(), source }
            })?;
        }

        if let Err(e) = std::fs::remove_file(&path) && e.kind() != std::io::ErrorKind::NotFound {
            return Err(ControlSockError::RemoveStale { path: path.to_owned(), source: e });
        }

        let socket = UnixDatagram::bind(&path).map_err(|source| {
            ControlSockError::Bind { path: path.to_owned(), source }
        })?;
        socket.set_nonblocking(true).map_err(ControlSockError::Configure)?;

        Ok(Self { path: path.to_owned(), socket, max_datagram_size })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }

    pub fn raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }

    pub fn recv_utf8_datagram(&self) -> Result<Option<String>, ControlSockError> {
        let mut buf = vec![0u8; self.max_datagram_size];
        match self.socket.recv(&mut buf) {
            Ok(n) => Ok(Some(std::str::from_utf8(&buf[..n]).map_err(ControlSockError::BadUtf8)?.to_owned())),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => Ok(None),
            Err(e) => Err(ControlSockError::Recv(e)),
        }
    }

    pub fn drain_utf8_datagrams(&self) -> Result<Vec<String>, ControlSockError> {
        let mut out = Vec::new();
        while let Some(data) = self.recv_utf8_datagram()? {
            out.push(data);
        }
        Ok(out)
    }
}

impl Drop for ControlSock {
    fn drop(&mut self) {
        _ = std::fs::remove_file(&self.path);
    }
}

pub fn default_sock_path(prog_name: &str) -> Result<PathBuf, ControlSockError> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or(ControlSockError::NoRuntimeDir)?;
    Ok(PathBuf::from(dir).join(prog_name).join(format!("{}.sock", std::process::id())))
}
