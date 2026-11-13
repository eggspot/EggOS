//! Moving bytes between descriptors inside the kernel (D557): `copy_file_range`, `sendfile`,
//! `splice` and `tee`. Each moves at most [`MAX_RW_COUNT`] bytes a call through a bounce buffer of
//! [`CHUNK`] bytes, between regular files (read from and written to their file system) and pipes
//! (taken from and pushed into their ring); what moves is the bytes, never a page. A pipe end
//! that is empty or full makes the call return what it moved, or `EAGAIN` if nothing.

use super::Errno;
use super::descriptors::{FileKind, OpenFile, open_flag};
use super::file_changes::now;
use super::pipe::PipeEnd;
use super::thread::LinuxThread;
use crate::process::Process;
use crate::vfs::{self, Node, NodeKind, WriteAt};
use alloc::sync::Arc;
use alloc::vec::Vec;
use eggos_hal::Platform;

const MAX_RW_COUNT: u64 = 0x7fff_f000;
const CHUNK: usize = 64 * 1024;

/// `splice`'s and `tee`'s flags: `SPLICE_F_NONBLOCK` is the only one that changes anything here.
const SPLICE_F_NONBLOCK: u64 = 2;

enum Source {
    Regular(Node),
    Pipe,
}

enum Sink {
    Regular(Node, bool),
    Pipe,
}

fn source_of(file: &OpenFile) -> Result<Source, Errno> {
    if !file.is_readable() {
        return Err(Errno::EBADF);
    }
    match file.kind() {
        FileKind::Pipe(_) => Ok(Source::Pipe),
        kind => match kind.node() {
            Some(node) if node.kind() == NodeKind::Regular => Ok(Source::Regular(node)),
            Some(node) if node.kind() == NodeKind::Directory => Err(Errno::EISDIR),
            _ => Err(Errno::EINVAL),
        },
    }
}

fn sink_of(file: &OpenFile) -> Result<Sink, Errno> {
    if !file.is_writable() {
        return Err(Errno::EBADF);
    }
    let append = file.status_flags() & open_flag::APPEND != 0;
    match file.kind() {
        FileKind::Pipe(_) => Ok(Sink::Pipe),
        kind => match kind.node() {
            Some(node) if node.kind() == NodeKind::Regular => Ok(Sink::Regular(node, append)),
            _ => Err(Errno::EINVAL),
        },
    }
}

fn pipe_end(file: &OpenFile) -> Option<&PipeEnd> {
    match file.kind() {
        FileKind::Pipe(end) => Some(end),
        _ => None,
    }
}

fn read_offset<P: Platform>(process: &Process<P>, pointer: u64) -> Result<Option<u64>, Errno> {
    if pointer == 0 {
        return Ok(None);
    }
    let mut bytes = [0u8; 8];
    process
        .read_user_memory(pointer, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    let offset = u64::from_ne_bytes(bytes);
    if (offset as i64) < 0 {
        return Err(Errno::EINVAL);
    }
    Ok(Some(offset))
}

fn write_offset<P: Platform>(process: &Process<P>, pointer: u64, offset: u64) -> Result<(), Errno> {
    process
        .write_user_memory(pointer, &offset.to_ne_bytes())
        .map_err(|_| Errno::EFAULT)
}

/// Moves up to `count` bytes from `from` (at `from_at`, or its file offset) to `to` (at `to_at`,
/// or its offset or end): how many moved, and the two positions after them.
fn transfer<P: Platform>(
    from: &OpenFile,
    from_at: Option<u64>,
    to: &OpenFile,
    to_at: Option<u64>,
    count: u64,
    keep_in_pipe: bool,
) -> Result<(u64, u64, u64), Errno> {
    let source = source_of(from)?;
    let sink = sink_of(to)?;
    let mut buffer: Vec<u8> = Vec::new();
    let chunk = (count.min(MAX_RW_COUNT) as usize).min(CHUNK);
    buffer
        .try_reserve_exact(chunk)
        .map_err(|_| Errno::ENOMEM)?;
    buffer.resize(chunk, 0);
    let mut read_at = from_at.unwrap_or_else(|| from.offset());
    let mut write_at = to_at.unwrap_or_else(|| to.offset());
    let mut moved = 0u64;
    let count = count.min(MAX_RW_COUNT);
    while moved < count {
        let wanted = ((count - moved) as usize).min(chunk);
        let got = match &source {
            Source::Regular(node) => node
                .file_system()
                .read_into(node.id(), read_at, &mut buffer[..wanted])
                .map_err(|_| Errno::EIO)?,
            Source::Pipe => {
                let end = pipe_end(from).ok_or(Errno::EINVAL)?;
                match keep_in_pipe {
                    true => end.peek_bytes(moved as usize, &mut buffer[..wanted]),
                    false => end.take_bytes(&mut buffer[..wanted]),
                }
            }
        };
        if got == 0 {
            break;
        }
        let put = match &sink {
            Sink::Regular(node, append) => {
                let at = match append {
                    true => WriteAt::End,
                    false => WriteAt::Offset(write_at),
                };
                let (written, after) = node
                    .file_system()
                    .write(node.id(), at, &buffer[..got], now::<P>())
                    .map_err(super::file_changes::refused)?;
                vfs::shared_pages::wrote(*node, after - written as u64, &buffer[..written]);
                write_at = after;
                written
            }
            Sink::Pipe => {
                let end = pipe_end(to).ok_or(Errno::EINVAL)?;
                end.push_bytes(&buffer[..got])?
            }
        };
        read_at += put as u64;
        moved += put as u64;
        if put < got {
            if let Source::Pipe = source
                && !keep_in_pipe
            {
                let end = pipe_end(from).ok_or(Errno::EINVAL)?;
                end.give_back(&buffer[put..got]);
            }
            break;
        }
    }
    Ok((moved, read_at, write_at))
}

fn finish_offsets(
    from: &OpenFile,
    from_pointer: bool,
    to: &OpenFile,
    to_pointer: bool,
    read_at: u64,
    write_at: u64,
) {
    if !from_pointer && !matches!(from.kind(), FileKind::Pipe(_)) {
        from.set_offset(read_at);
    }
    if !to_pointer && !matches!(to.kind(), FileKind::Pipe(_)) {
        to.set_offset(write_at);
    }
}

/// `copy_file_range(fd_in, off_in, fd_out, off_out, len, flags)`: copies up to `len` bytes
/// between two regular files, at the offsets `off_in` and `off_out` point to (moved past what
/// was copied) or the files' own. As Linux: `EINVAL` for flags, a non-regular file or the same
/// file's overlapping ranges, `EBADF` for a descriptor not open the right way, `EFAULT`.
pub fn copy_file_range<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    arguments: [u64; 6],
) -> Result<u64, Errno> {
    let [fd_in, off_in, fd_out, off_out, length, flags] = arguments;
    if flags != 0 {
        return Err(Errno::EINVAL);
    }
    let from = thread.files().file(fd_in)?;
    let to = thread.files().file(fd_out)?;
    if matches!(from.kind(), FileKind::Pipe(_)) || matches!(to.kind(), FileKind::Pipe(_)) {
        return Err(Errno::EINVAL);
    }
    let from_at = read_offset(process, off_in)?;
    let to_at = read_offset(process, off_out)?;
    if let (Some(a), Some(b), Some(x), Some(y)) =
        (from.kind().node(), to.kind().node(), from_at.or(Some(from.offset())), to_at.or(Some(to.offset())))
        && a == b
        && x < y.saturating_add(length)
        && y < x.saturating_add(length)
    {
        return Err(Errno::EINVAL);
    }
    let (moved, read_at, write_at) = transfer::<P>(&from, from_at, &to, to_at, length, false)?;
    finish(process, &from, off_in, &to, off_out, read_at, write_at)?;
    Ok(moved)
}

fn finish<P: Platform>(
    process: &Process<P>,
    from: &Arc<OpenFile>,
    off_in: u64,
    to: &Arc<OpenFile>,
    off_out: u64,
    read_at: u64,
    write_at: u64,
) -> Result<(), Errno> {
    if off_in != 0 {
        write_offset(process, off_in, read_at)?;
    }
    if off_out != 0 {
        write_offset(process, off_out, write_at)?;
    }
    finish_offsets(from, off_in != 0, to, off_out != 0, read_at, write_at);
    Ok(())
}

/// `sendfile(out_fd, in_fd, offset, count)`: copies up to `count` bytes from the regular file
/// `in_fd` (at `*offset`, which moves, or at its own offset) to `out_fd`, a regular file or a
/// pipe. As Linux: `EINVAL` for an input that is not a regular file, `EAGAIN` for a full pipe.
pub fn sendfile<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    out_fd: u64,
    in_fd: u64,
    offset: u64,
    count: u64,
) -> Result<u64, Errno> {
    let from = thread.files().file(in_fd)?;
    let to = thread.files().file(out_fd)?;
    if matches!(from.kind(), FileKind::Pipe(_)) {
        return Err(Errno::EINVAL);
    }
    let from_at = read_offset(process, offset)?;
    let (moved, read_at, write_at) = transfer::<P>(&from, from_at, &to, None, count, false)?;
    if moved == 0 && count > 0 && matches!(to.kind(), FileKind::Pipe(_)) && to.is_nonblocking() {
        return Err(Errno::EAGAIN);
    }
    finish(process, &from, offset, &to, 0, read_at, write_at)?;
    Ok(moved)
}

/// `splice(fd_in, off_in, fd_out, off_out, len, flags)`: moves up to `len` bytes between a pipe
/// and a regular file, or two pipes; a pipe end takes no offset (`ESPIPE`). As Linux: `EINVAL`
/// when neither end is a pipe, `EAGAIN` with `SPLICE_F_NONBLOCK` and nothing to move.
pub fn splice<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    arguments: [u64; 6],
) -> Result<u64, Errno> {
    let [fd_in, off_in, fd_out, off_out, length, flags] = arguments;
    let from = thread.files().file(fd_in)?;
    let to = thread.files().file(fd_out)?;
    let (from_pipe, to_pipe) = (
        matches!(from.kind(), FileKind::Pipe(_)),
        matches!(to.kind(), FileKind::Pipe(_)),
    );
    if !from_pipe && !to_pipe {
        return Err(Errno::EINVAL);
    }
    if (from_pipe && off_in != 0) || (to_pipe && off_out != 0) {
        return Err(Errno::ESPIPE);
    }
    let from_at = read_offset(process, off_in)?;
    let to_at = read_offset(process, off_out)?;
    let (moved, read_at, write_at) = transfer::<P>(&from, from_at, &to, to_at, length, false)?;
    if moved == 0 && length > 0 && flags & SPLICE_F_NONBLOCK != 0 && from_pipe {
        return Err(Errno::EAGAIN);
    }
    finish(process, &from, off_in, &to, off_out, read_at, write_at)?;
    Ok(moved)
}

/// `tee(fd_in, fd_out, len, flags)`: copies up to `len` bytes from one pipe to another, leaving
/// them in the first. As Linux: `EINVAL` unless both are pipes, and for the same pipe.
pub fn tee(thread: &LinuxThread, fd_in: u64, fd_out: u64, length: u64, flags: u64) -> Result<u64, Errno> {
    let from = thread.files().file(fd_in)?;
    let to = thread.files().file(fd_out)?;
    let (Some(source), Some(sink)) = (pipe_end(&from), pipe_end(&to)) else {
        return Err(Errno::EINVAL);
    };
    if source.same_pipe(sink) {
        return Err(Errno::EINVAL);
    }
    let mut buffer = [0u8; 4096];
    let mut moved = 0u64;
    while moved < length.min(MAX_RW_COUNT) {
        let wanted = ((length - moved) as usize).min(buffer.len());
        let got = source.peek_bytes(moved as usize, &mut buffer[..wanted]);
        if got == 0 {
            break;
        }
        let put = sink.push_bytes(&buffer[..got])?;
        moved += put as u64;
        if put < got {
            break;
        }
    }
    if moved == 0 && length > 0 && flags & SPLICE_F_NONBLOCK != 0 {
        return Err(Errno::EAGAIN);
    }
    Ok(moved)
}
