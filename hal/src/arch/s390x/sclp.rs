//! The service-call logical processor (SCLP, D321): the console of an s390x machine without a
//! terminal of its own, and what says how much memory there is. A command is a `servc` with a
//! service-call control block (SCCB) below 2 GiB; QEMU completes it before `servc` returns and
//! raises a service signal, which the HAL takes and ignores, so the HAL polls the response
//! code.

use core::sync::atomic::{AtomicBool, Ordering};

const WRITE_EVENT_DATA: u32 = 0x0076_0005;
const READ_EVENT_DATA: u32 = 0x0077_0005;
const READ_SCP_INFO: u32 = 0x0002_0001;
const NORMAL_COMPLETION: u16 = 0x0020;
const ASCII_CONSOLE: u8 = 0x1a;
const HEADER: usize = 8;
const EVENT_HEADER: usize = 6;
const SCCB_BYTES: usize = 4096;
const CHUNK: usize = SCCB_BYTES - HEADER - EVENT_HEADER;

#[repr(C, align(4096))]
struct Sccb([u8; SCCB_BYTES]);

static mut SCCB: Sccb = Sccb([0; SCCB_BYTES]);
static BUSY: AtomicBool = AtomicBool::new(false);

fn hold() {
    while BUSY.swap(true, Ordering::Acquire) {
        core::hint::spin_loop();
    }
}

fn release() {
    BUSY.store(false, Ordering::Release);
}

fn real_address() -> u64 {
    (&raw const SCCB) as u64 - super::KERNEL_VIRTUAL_OFFSET
}

fn sccb() -> &'static mut [u8; SCCB_BYTES] {
    // SAFETY: only a caller holding `BUSY` reaches the block, one at a time.
    unsafe { &mut (*(&raw mut SCCB)).0 }
}

fn service_call(command: u32, length: usize) -> u16 {
    let block = sccb();
    block[..2].copy_from_slice(&(length as u16).to_be_bytes());
    block[6..8].copy_from_slice(&0u16.to_be_bytes());
    let condition: u32;
    // SAFETY: `servc` hands the SCLP the SCCB at its real address, which this module owns
    // while `BUSY` is held; QEMU completes the command before the instruction ends.
    unsafe {
        core::arch::asm!(
            ".insn rre,0xb2200000,{command},{block}",
            "ipm {cc}",
            "srl {cc},28",
            command = in(reg) u64::from(command),
            block = in(reg) real_address(),
            cc = out(reg) condition,
        );
    }
    if condition != 0 {
        return 0;
    }
    u16::from_be_bytes([block[6], block[7]])
}

/// Writes `bytes` to the ASCII console event, in chunks.
pub(super) fn write(bytes: &[u8]) {
    hold();
    for chunk in bytes.chunks(CHUNK) {
        let block = sccb();
        let length = HEADER + EVENT_HEADER + chunk.len();
        block[HEADER..HEADER + 2]
            .copy_from_slice(&((EVENT_HEADER + chunk.len()) as u16).to_be_bytes());
        block[HEADER + 2] = ASCII_CONSOLE;
        block[HEADER + 3] = 0;
        block[HEADER + 4..HEADER + 6].fill(0);
        block[HEADER + EVENT_HEADER..length].copy_from_slice(chunk);
        service_call(WRITE_EVENT_DATA, length);
    }
    release();
}

/// Reads what the console typed into `buffer`; how many bytes.
pub(super) fn read(buffer: &mut [u8]) -> usize {
    hold();
    let block = sccb();
    block[HEADER..].fill(0);
    let mut taken = 0;
    if service_call(READ_EVENT_DATA, SCCB_BYTES) == NORMAL_COMPLETION {
        let event_length = usize::from(u16::from_be_bytes([block[HEADER], block[HEADER + 1]]));
        if block[HEADER + 2] == ASCII_CONSOLE && event_length > EVENT_HEADER {
            let data =
                &block[HEADER + EVENT_HEADER..HEADER + event_length.min(SCCB_BYTES - HEADER)];
            taken = data.len().min(buffer.len());
            buffer[..taken].copy_from_slice(&data[..taken]);
        }
    }
    release();
    taken
}

/// The machine's memory in bytes, from `READ SCP INFO`: the increments times their size;
/// `None` when the command fails.
pub(super) fn memory_bytes() -> Option<u64> {
    hold();
    let block = sccb();
    block[HEADER..].fill(0);
    let size = match service_call(READ_SCP_INFO, SCCB_BYTES) {
        NORMAL_COMPLETION => {
            let increments = u64::from(u16::from_be_bytes([block[8], block[9]]));
            let increment_mib = u64::from(block[10]);
            let (increments, increment_mib) = match (increments, increment_mib) {
                (0, _) | (_, 0) => (
                    u64::from_be_bytes(block[0x20..0x28].try_into().unwrap_or([0; 8])),
                    u64::from(u32::from_be_bytes(
                        block[0x18..0x1c].try_into().unwrap_or([0; 4]),
                    )),
                ),
                pair => pair,
            };
            Some(increments * increment_mib << 20).filter(|&bytes| bytes != 0)
        }
        _ => None,
    };
    release();
    size
}
