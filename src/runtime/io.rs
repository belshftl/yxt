// SPDX-License-Identifier: MIT

use std::os::fd::AsRawFd;

#[derive(Debug)]
pub struct ByteQueue {
    buf: Box<[u8]>,
    head: usize,
    tail: usize,
}

impl ByteQueue {
    pub fn new(cap: usize) -> Self {
        assert!(cap > 0);
        Self {
            buf: vec![0; cap].into_boxed_slice(),
            head: 0,
            tail: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    pub fn len(&self) -> usize {
        self.tail - self.head
    }

    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    pub fn remaining(&self) -> usize {
        self.capacity() - self.len()
    }

    pub fn pending(&self) -> &[u8] {
        &self.buf[self.head..self.tail]
    }

    pub fn consume(&mut self, n: usize) {
        assert!(n <= self.len());
        self.head += n;
        if self.head == self.tail {
            self.head = 0;
            self.tail = 0;
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<(), QueueFull> {
        if bytes.len() > self.remaining() {
            return Err(QueueFull);
        }

        self.make_tail_space(bytes.len());

        let end = self.tail + bytes.len();
        self.buf[self.tail..end].copy_from_slice(bytes);
        self.tail = end;

        Ok(())
    }

    pub fn writable_tail(&mut self) -> &mut [u8] {
        if self.remaining() == 0 {
            return &mut [];
        }
        if self.tail == self.capacity() {
            self.compact();
        }
        &mut self.buf[self.tail..]
    }

    pub fn commit(&mut self, n: usize) {
        assert!(n <= self.capacity() - self.tail);
        self.tail += n;
    }

    fn make_tail_space(&mut self, needed: usize) {
        if self.capacity() - self.tail >= needed {
            return;
        }
        self.compact();
        debug_assert!(self.capacity() - self.tail >= needed);
    }

    fn compact(&mut self) {
        let len = self.len();
        if len != 0 {
            self.buf.copy_within(self.head..self.tail, 0);
        }
        self.head = 0;
        self.tail = len;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("byte queue is full")]
pub struct QueueFull;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadResult {
    Success(usize),
    WouldBlock,
    Eof,
    EmptyInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadToQueueResult {
    Success {
        offset: usize,
        len: usize,
    },
    WouldBlock,
    Eof,
    NoSpace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteResult {
    Success(usize),
    WouldBlock,
    EmptyInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteToPtyResult {
    Success(usize),
    WouldBlock,
    Hangup,
    EmptyInput,
}

pub fn read<F: AsRawFd + ?Sized>(fd: &F, buf: &mut [u8]) -> std::io::Result<ReadResult> {
    if buf.is_empty() {
        return Ok(ReadResult::EmptyInput);
    }
    loop {
        let rv = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if rv < 0 {
            let e = std::io::Error::last_os_error();
            match e.kind() {
                std::io::ErrorKind::Interrupted => continue,
                std::io::ErrorKind::WouldBlock => return Ok(ReadResult::WouldBlock),
                _ => return Err(e),
            }
        } else if rv == 0 {
            return Ok(ReadResult::Eof);
        } else if rv > 0 {
            return Ok(ReadResult::Success(rv as usize));
        }
        unreachable!();
    }
}

pub fn read_to_queue<F: AsRawFd + ?Sized>(fd: &F, q: &mut ByteQueue) -> std::io::Result<ReadToQueueResult> {
    if q.remaining() == 0 {
        return Ok(ReadToQueueResult::NoSpace);
    }

    let offset = q.len();
    let dst = q.writable_tail();
    if dst.is_empty() {
        return Ok(ReadToQueueResult::NoSpace);
    }

    loop {
        let rv = unsafe { libc::read(fd.as_raw_fd(), dst.as_mut_ptr().cast(), dst.len()) };
        if rv < 0 {
            let e = std::io::Error::last_os_error();
            match e.kind() {
                std::io::ErrorKind::Interrupted => continue,
                std::io::ErrorKind::WouldBlock => return Ok(ReadToQueueResult::WouldBlock),
                _ => return Err(e),
            }
        } else if rv == 0 {
            return Ok(ReadToQueueResult::Eof);
        } else if rv > 0 {
            let len = rv as usize;
            q.commit(len);
            return Ok(ReadToQueueResult::Success { offset, len });
        }
        unreachable!();
    }
}

pub fn read_pty_to_queue<F: AsRawFd + ?Sized>(fd: &F, q: &mut ByteQueue) -> std::io::Result<ReadToQueueResult> {
    match read_to_queue(fd, q) {
        Err(e) if is_pty_hangup(&e) => Ok(ReadToQueueResult::Eof),
        other => other,
    }
}

pub fn drain_from_queue<F: AsRawFd + ?Sized>(fd: &F, q: &mut ByteQueue) -> std::io::Result<WriteResult> {
    if q.is_empty() {
        return Ok(WriteResult::EmptyInput);
    }

    loop {
        let pending = q.pending();
        let rv = unsafe { libc::write(fd.as_raw_fd(), pending.as_ptr().cast(), pending.len()) };
        if rv < 0 {
            let e = std::io::Error::last_os_error();
            match e.kind() {
                std::io::ErrorKind::Interrupted => continue,
                std::io::ErrorKind::WouldBlock => return Ok(WriteResult::WouldBlock),
                _ => return Err(e),
            }
        } else if rv == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::WriteZero));
        } else if rv > 0 {
            let n = rv as usize;
            q.consume(n);
            return Ok(WriteResult::Success(n));
        }
        unreachable!();
    }
}

pub fn drain_to_pty_from_queue<F: AsRawFd + ?Sized>(fd: &F, q: &mut ByteQueue) -> std::io::Result<WriteToPtyResult> {
    match drain_from_queue(fd, q) {
        Err(e) if is_pty_hangup(&e) => Ok(WriteToPtyResult::Hangup),
        other => other.map(|r| match r {
            WriteResult::Success(n) => WriteToPtyResult::Success(n),
            WriteResult::WouldBlock => WriteToPtyResult::WouldBlock,
            WriteResult::EmptyInput => WriteToPtyResult::EmptyInput,
        }),
    }
}

fn is_pty_hangup(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::EIO) | Some(libc::EPIPE)) || e.kind() == std::io::ErrorKind::WriteZero
}
