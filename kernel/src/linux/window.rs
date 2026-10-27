//! `/dev/window` for Linux programs (D127, docs/windows.md): opening the device connects a
//! client of the window server ([`crate::window`]); `ioctl` makes and changes its windows,
//! `mmap` maps a window's pixels, `read` takes its event records (waiting for one unless
//! `O_NONBLOCK`), `poll` reports `POLLIN` while one waits, `fstat` reports the character
//! device, and closing the last descriptor destroys the client's windows. `write` is `EINVAL`
//! and `lseek` `ESPIPE`, as on a Linux character device without them.

use super::descriptors::{FileKind, OpenFile};
use super::poll::event;
use super::signals::{self, Interrupted};
use super::terminal::{Waited, wait_for_input};
use super::thread::LinuxThread;
use super::{Errno, Iovecs, iovec};
use crate::process::{LentFrames, Placement, Process};
use crate::regions::RegionError;
use crate::terminal::with_windows;
use crate::window::{self, Connection, PixelBuffer, ServerError};
use alloc::sync::Arc;
use core::ops::ControlFlow;
use eggos_drivers::desktop::protocol::event as record_kind;
use eggos_drivers::desktop::protocol::{
    EVENT_SIZE, EventRecord, EventSink, MAX_COMMIT_RECTS, Request, WindowError, create_reply,
    list_reply, put_double_word, put_word, request, set_size_reply, size,
};
use eggos_drivers::desktop::{Desktop, WindowId};
use eggos_drivers::display::Damage;
use eggos_hal::Platform;
use eggos_hal::paging::Attributes;
use eggos_hal::user::UserContext;

const READ_RECORDS: usize = 16;

const SCALE: u32 = 1;

fn errno(error: ServerError) -> Errno {
    match error {
        ServerError::Refused(WindowError::Invalid) => Errno::EINVAL,
        ServerError::Refused(WindowError::NotAllowed) => Errno::EPERM,
        ServerError::Refused(WindowError::TableFull) => Errno::ENOSPC,
        ServerError::Refused(WindowError::Unsupported) => Errno::EOPNOTSUPP,
        ServerError::Refused(WindowError::NoSuchWindow) => Errno::ENOENT,
        ServerError::NoServer => Errno::ENXIO,
        ServerError::NoClientSlot => Errno::ENFILE,
        ServerError::OutOfMemory => Errno::ENOMEM,
    }
}

fn refused(error: WindowError) -> Errno {
    errno(ServerError::Refused(error))
}

/// Opens the window server's device: a new client. `ENXIO` without a display, `ENFILE` when
/// the server serves as many clients as it may.
pub fn open<P: Platform>() -> Result<FileKind, Errno> {
    window::connect(window::disconnect::<P>)
        .map(FileKind::Window)
        .map_err(errno)
}

/// `ioctl(fd, request, argument)` on `/dev/window`: the requests of docs/windows.md, their
/// argument read from and written back to `argument`. `ENOTTY` for a request it does not
/// answer, `EFAULT` for an argument outside the program's memory.
pub fn ioctl<P: Platform>(
    process: &Process<P>,
    connection: &Connection,
    number: u64,
    argument: u64,
) -> Result<u64, Errno> {
    let number = number as u32;
    let length = Request::argument_size(number).ok_or(Errno::ENOTTY)?;
    let mut bytes = [0u8; size::LARGEST];
    let bytes = &mut bytes[..length];
    if number != request::GET_WORK_AREA {
        process
            .read_user_memory(argument, bytes)
            .map_err(|_| Errno::EFAULT)?;
    }
    let request = Request::parse(number, bytes).ok_or(Errno::ENOTTY)?;
    let client = connection.client().id();
    let manages = matches!(
        request,
        Request::ListWindow { .. } | Request::ManageWindow { .. }
    );
    if manages
        && !process
            .credentials_and_limits()
            .lock()
            .credentials
            .is_privileged()
    {
        return Err(Errno::EPERM);
    }
    match request {
        Request::Create {
            role,
            parent,
            width,
            height,
            flags,
        } => {
            let (window, offset, width, height) =
                create::<P>(connection, role, parent, width, height, flags)?;
            put_word(bytes, create_reply::WIDTH, width);
            put_word(bytes, create_reply::HEIGHT, height);
            put_word(bytes, create_reply::WINDOW, u32::from(window));
            put_double_word(bytes, create_reply::OFFSET, offset);
            if process.write_user_memory(argument, bytes).is_err() {
                on_windows::<P, _>(|desktop, buffers, damage, events| {
                    desktop.destroy(client, u32::from(window), damage, events)?;
                    buffers.release_unused(desktop);
                    Ok(())
                })?;
                return Err(Errno::EFAULT);
            }
        }
        Request::SetTitle {
            window,
            length,
            title,
        } => {
            let title = title.get(..length as usize).ok_or(Errno::EINVAL)?;
            on_windows::<P, _>(|desktop, _, damage, _| {
                desktop.set_title(client, window, title, damage)
            })?;
        }
        Request::SetSize {
            window,
            width,
            height,
        } => {
            let (offset, width, height) = set_size::<P>(connection, window, width, height)?;
            put_word(bytes, set_size_reply::WIDTH, width);
            put_word(bytes, set_size_reply::HEIGHT, height);
            put_double_word(bytes, set_size_reply::OFFSET, offset);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::SetMinimumSize {
            window,
            width,
            height,
        } => on_windows::<P, _>(|desktop, _, _, _| {
            desktop.set_minimum_size(client, window, width, height)
        })?,
        Request::SetPosition { window, x, y } => on_windows::<P, _>(|desktop, _, damage, _| {
            desktop.set_position(client, window, x, y, damage)
        })?,
        Request::SetState { window, state } => on_windows::<P, _>(|desktop, _, damage, events| {
            desktop.set_state(client, window, state, damage, events)
        })?,
        Request::SetPointer { window, shape } => on_windows::<P, _>(|desktop, _, damage, _| {
            desktop.set_pointer(client, window, shape, damage)
        })?,
        Request::Commit {
            window,
            count,
            rects,
        } => {
            let rects = rects
                .get(..count as usize)
                .filter(|_| count as usize <= MAX_COMMIT_RECTS)
                .ok_or(Errno::EINVAL)?;
            on_windows::<P, _>(|desktop, _, damage, events| {
                desktop.commit(client, window, rects, damage, events)
            })?;
        }
        Request::Destroy { window } => on_windows::<P, _>(|desktop, buffers, damage, events| {
            desktop.destroy(client, window, damage, events)?;
            buffers.release_unused(desktop);
            Ok(())
        })?,
        Request::GetWorkArea => {
            let mut area = None;
            on_windows::<P, _>(|desktop, _, _, _| {
                area = Some(desktop.work_area());
                Ok(())
            })?;
            let area = area.ok_or(Errno::ENXIO)?;
            for (index, value) in [area.x, area.y, area.width, area.height, SCALE]
                .into_iter()
                .enumerate()
            {
                put_word(bytes, index * 4, value);
            }
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::ListWindow { index } => {
            let info =
                on_windows::<P, _>(|desktop, _, _, _| desktop.listed_window(index as usize))?;
            bytes.fill(0);
            for (offset, value) in [
                (list_reply::WINDOW, u32::from(info.id)),
                (list_reply::STATE, info.state.value()),
                (list_reply::X, info.rect.x),
                (list_reply::Y, info.rect.y),
                (list_reply::WIDTH, info.rect.width),
                (list_reply::HEIGHT, info.rect.height),
                (list_reply::CLIENT, u32::from(info.client)),
                (list_reply::TITLE_LENGTH, info.title_length as u32),
            ] {
                put_word(bytes, offset, value);
            }
            bytes[list_reply::TITLE..list_reply::TITLE + info.title.len()]
                .copy_from_slice(&info.title);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::ManageWindow {
            window,
            action,
            a,
            b,
        } => on_windows::<P, _>(|desktop, _, damage, events| {
            desktop.manage(window, action, a, b, damage, events)
        })?,
    }
    Ok(0)
}

fn on_windows<P: Platform, R>(
    change: impl FnOnce(
        &mut Desktop,
        &mut window::Buffers,
        &mut Damage,
        &mut dyn EventSink,
    ) -> Result<R, WindowError>,
) -> Result<R, Errno> {
    with_windows::<P, _>(change)
        .ok_or(Errno::ENXIO)?
        .map_err(refused)
}

const SIZE_ATTEMPTS: usize = 4;

fn create<P: Platform>(
    connection: &Connection,
    role: u32,
    parent: u32,
    width: u32,
    height: u32,
    flags: u32,
) -> Result<(WindowId, u64, u32, u32), Errno> {
    let client = connection.client().id();
    let size = with_windows::<P, _>(|desktop, _, _, _| desktop.created_size(width, height))
        .ok_or(Errno::ENXIO)?;
    let buffer = PixelBuffer::new(size.0, size.1, connection.client()).ok_or(Errno::ENOMEM)?;
    with_windows::<P, _>(|desktop, buffers, _, _| {
        if desktop.created_size(width, height) != buffer.size() {
            return Err(WindowError::Invalid);
        }
        let created = desktop.create(client, role, parent, width, height, flags)?;
        let offset = buffers.attach(created.window, buffer);
        Ok((created.window, offset, created.width, created.height))
    })
    .ok_or(Errno::ENXIO)?
    .map_err(refused)
}

fn set_size<P: Platform>(
    connection: &Connection,
    window: u32,
    width: u32,
    height: u32,
) -> Result<(u64, u32, u32), Errno> {
    let client = connection.client().id();
    let id = WindowId::try_from(window).map_err(|_| Errno::EINVAL)?;
    for _ in 0..SIZE_ATTEMPTS {
        let size = with_windows::<P, _>(|desktop, _, _, _| {
            desktop.size_for(client, window, width, height)
        })
        .ok_or(Errno::ENXIO)?
        .map_err(refused)?;
        let buffer = PixelBuffer::new(size.0, size.1, connection.client()).ok_or(Errno::ENOMEM)?;
        let attached = with_windows::<P, _>(|desktop, buffers, damage, events| {
            if desktop.size_for(client, window, width, height)? != buffer.size() {
                return Ok(None);
            }
            let resized = desktop.set_size(client, window, width, height, damage, events)?;
            Ok(Some((buffers.attach(id, buffer), resized.0, resized.1)))
        })
        .ok_or(Errno::ENXIO)?
        .map_err(refused)?;
        if let Some(attached) = attached {
            return Ok(attached);
        }
    }
    Err(Errno::EAGAIN)
}

/// The `poll` events `connection` reports of `requested`: `POLLIN` while an event waits.
pub fn events(connection: &Connection, requested: u16) -> u16 {
    match connection.client().has_events() {
        true => requested & (event::IN | event::RDNORM),
        false => 0,
    }
}

/// `read` or `readv` of `buffers` from `/dev/window` through `file`: whole records, as many as
/// fit (`EINVAL` if not one does), waiting for one unless `O_NONBLOCK` (`EAGAIN`); sets the
/// call's result and delivers the signals due after it. A record whose copy to the program
/// fails is lost, as on Linux's input devices.
pub(super) fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    connection: &Connection,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    let interrupted = Interrupted::RestartsIfAsked {
        number: context.syscall_number(),
        first_argument: context.syscall_arguments()[0],
    };
    let result = loop {
        match read_or_wait(process, thread, file, connection, buffers)? {
            Some(result) => break result,
            None => {
                if signals::deliver_interrupting(process, thread, context, interrupted)? {
                    return ControlFlow::Continue(());
                }
            }
        }
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    signals::take_in_sent(process, thread);
    signals::deliver(process, thread, context)
}

fn read_or_wait<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    file: &OpenFile,
    connection: &Connection,
    buffers: &Iovecs,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let room = buffers.total() / EVENT_SIZE as u64;
    if room == 0 {
        return ControlFlow::Continue(Some(Err(Errno::EINVAL)));
    }
    let client = connection.client();
    loop {
        let mut records = [EventRecord::default(); READ_RECORDS];
        let wanted = room.min(READ_RECORDS as u64) as usize;
        let taken = client.take_events(&mut records[..wanted]);
        if taken > 0 {
            if records[..taken]
                .iter()
                .any(|record| record.kind == record_kind::CLOSE_REQUEST)
            {
                let id = client.id();
                with_windows::<P, _>(|desktop, _, damage, _| desktop.close_answered(id, damage));
            }
            return ControlFlow::Continue(Some(copy_out(process, buffers, &records[..taken])));
        }
        if file.is_nonblocking() {
            return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
        }
        if wait_for_input(process, thread, u64::MAX, || client.has_events())? == Waited::Signalled {
            return ControlFlow::Continue(None);
        }
    }
}

fn copy_out<P: Platform>(
    process: &Process<P>,
    buffers: &Iovecs,
    records: &[EventRecord],
) -> Result<u64, Errno> {
    let mut bytes = [0u8; READ_RECORDS * EVENT_SIZE];
    for (record, out) in records.iter().zip(bytes.as_chunks_mut::<EVENT_SIZE>().0) {
        out.copy_from_slice(&record.to_bytes());
    }
    let bytes = &bytes[..records.len() * EVENT_SIZE];
    let mut copied = 0;
    for entry in buffers.entries() {
        let (base, length) = iovec(entry);
        let part = (length as usize).min(bytes.len() - copied);
        if part > 0
            && process
                .write_user_memory(base, &bytes[copied..copied + part])
                .is_err()
        {
            return Err(Errno::EFAULT);
        }
        copied += part;
        if copied == bytes.len() {
            break;
        }
    }
    Ok(copied as u64)
}

/// `mmap` of `length` bytes of `/dev/window` at `offset` with `attributes` where `placement`
/// says: the pixels of the caller's window whose buffer `offset` names, shared with the
/// server. `EINVAL` for an offset that names none of the caller's buffers or a length past
/// the buffer.
pub fn map<P: Platform>(
    process: &Process<P>,
    connection: &Connection,
    placement: Placement,
    length: u64,
    attributes: Attributes,
    offset: u64,
) -> Result<u64, Errno> {
    let client = connection.client().id();
    let buffer = with_windows::<P, _>(|desktop, buffers, _, _| {
        let (window, buffer) = buffers.at_offset(offset)?;
        desktop.owns(client, window).then_some(buffer)
    })
    .ok_or(Errno::ENXIO)?
    .ok_or(Errno::EINVAL)?;
    if length > buffer.length() {
        return Err(Errno::EINVAL);
    }
    process
        .map_lent_frames(placement, length, attributes, buffer)
        .map_err(|error| match error {
            RegionError::Overlaps => Errno::EEXIST,
            RegionError::Invalid => Errno::EINVAL,
            RegionError::NotCovered | RegionError::NoRoom | RegionError::OutOfMemory => {
                Errno::ENOMEM
            }
        })
}
