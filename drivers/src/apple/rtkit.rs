//! RTKit (D543), the firmware Apple's coprocessors run, spoken over their mailbox: the
//! management endpoint's handshake (the protocol version, the map of endpoints, the system
//! endpoints started, the coprocessor's and then the application processor's power state set to
//! on), the shared memory the crash log, system log, I/O report and OS log endpoints ask for,
//! the acknowledgments they wait on, and a crash. Messages to application endpoints (from
//! [`endpoint::FIRST_APPLICATION`] on) go to the driver of the coprocessor.

use super::mailbox::{Full, Mailbox, Message};
use crate::io::RegisterIo;

/// Endpoint numbers.
pub mod endpoint {
    pub const MANAGEMENT: u8 = 0;
    pub const CRASH_LOG: u8 = 1;
    pub const SYSTEM_LOG: u8 = 2;
    pub const DEBUG: u8 = 3;
    pub const IO_REPORT: u8 = 4;
    pub const OS_LOG: u8 = 8;
    /// The first endpoint of the coprocessor's own services (the display coprocessor's
    /// framebuffer endpoint, ...).
    pub const FIRST_APPLICATION: u8 = 0x20;
}

/// Power states, for the coprocessor (IOP) and as it sees the application processor (AP).
pub mod power_state {
    pub const OFF: u16 = 0x00;
    pub const SLEEP: u16 = 0x01;
    pub const QUIESCED: u16 = 0x10;
    pub const ON: u16 = 0x20;
    pub const INIT: u16 = 0x220;
}

const MINIMUM_VERSION: u64 = 11;
const MAXIMUM_VERSION: u64 = 12;
const TYPE_SHIFT: u32 = 52;
const TYPE_MASK: u64 = 0xff;
const OS_LOG_TYPE_SHIFT: u32 = 56;
const HELLO: u64 = 1;
const HELLO_REPLY: u64 = 2;
const START_ENDPOINT: u64 = 5;
const SET_IOP_POWER: u64 = 6;
const IOP_POWER_ACK: u64 = 7;
const ENDPOINT_MAP: u64 = 8;
const SET_AP_POWER: u64 = 0xb;
const VERSION_MASK: u64 = 0xffff;
const MAXIMUM_VERSION_SHIFT: u32 = 16;
const MAP_LAST: u64 = 1 << 51;
const MAP_BASE_SHIFT: u32 = 32;
const MAP_BASE_MASK: u64 = 0x7;
const MAP_BITMAP_MASK: u64 = 0xffff_ffff;
const MAP_REPLY_MORE: u64 = 1;
const START_ENDPOINT_SHIFT: u32 = 32;
const START_FLAG: u64 = 1 << 1;
const POWER_MASK: u64 = 0xffff;
const BUFFER_REQUEST: u64 = 1;
const BUFFER_SIZE_SHIFT: u32 = 44;
const BUFFER_SIZE_MASK: u64 = 0xff;
const BUFFER_ADDRESS_MASK: u64 = (1 << 44) - 1;
const OS_LOG_SIZE_SHIFT: u32 = 36;
const OS_LOG_SIZE_MASK: u64 = (1 << 20) - 1;
const OS_LOG_ADDRESS_MASK: u64 = (1 << 36) - 1;
const PAGE_SHIFT: u32 = 12;
const SYSTEM_LOG_ENTRY: u64 = 5;
const SYSTEM_LOG_INIT: u64 = 8;
const IO_REPORT_ACKED: [u64; 2] = [0x8, 0xc];
const CRASH: u64 = 1;
/// The most messages one [`Rtkit::poll`] takes, so a coprocessor that never stops talking
/// cannot hold its caller.
pub const MOST_MESSAGES_PER_POLL: usize = 256;

/// What the coprocessor's driver lends RTKit.
pub trait SharedMemory {
    /// Memory of `bytes` bytes for the system endpoint `endpoint`, at the address the
    /// coprocessor sees it at through its DART, `address` when the coprocessor named one
    /// (memory the firmware already set aside): that address, or `None` when there is none.
    fn shared_memory(&mut self, endpoint: u8, bytes: u64, address: u64) -> Option<u64>;
}

/// Why talking to the coprocessor failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtkitError {
    /// The firmware speaks only protocol versions outside 11 to 12.
    UnsupportedVersion { minimum: u16, maximum: u16 },
    /// The mailbox's send FIFO stayed full.
    MailboxFull,
    /// No shared memory for a system endpoint.
    NoMemory,
    /// The coprocessor reported a crash.
    Crashed,
    /// The endpoint is not one the coprocessor has, or not running yet.
    NoSuchEndpoint,
}

impl From<Full> for RtkitError {
    fn from(_: Full) -> Self {
        Self::MailboxFull
    }
}

/// One coprocessor's RTKit, through `mailbox`.
#[derive(Debug)]
pub struct Rtkit<R: RegisterIo> {
    mailbox: Mailbox<R>,
    version: Option<u16>,
    endpoints: [u64; 4],
    map_done: bool,
    iop_power: u16,
    ap_power: u16,
    crash_log_given: bool,
    crashed: bool,
}

impl<R: RegisterIo> Rtkit<R> {
    pub fn new(mailbox: Mailbox<R>) -> Self {
        Self {
            mailbox,
            version: None,
            endpoints: [0; 4],
            map_done: false,
            iop_power: power_state::OFF,
            ap_power: power_state::OFF,
            crash_log_given: false,
            crashed: false,
        }
    }

    /// The protocol version agreed, once the handshake has begun.
    pub fn version(&self) -> Option<u16> {
        self.version
    }

    /// Whether the coprocessor and the application processor are both on, as RTKit sees it:
    /// application endpoints work only then.
    pub fn is_running(&self) -> bool {
        self.iop_power & 0xff == power_state::ON && self.ap_power & 0xff == power_state::ON
    }

    pub fn has_crashed(&self) -> bool {
        self.crashed
    }

    /// Whether the coprocessor's endpoint map named `endpoint`.
    pub fn has_endpoint(&self, endpoint: u8) -> bool {
        self.endpoints[usize::from(endpoint / 64)] & 1 << (endpoint % 64) != 0
    }

    /// Asks a coprocessor that is stopped or asleep to start: its power state to `INIT`. The
    /// handshake then runs through [`Self::poll`].
    pub fn wake(&mut self) -> Result<(), RtkitError> {
        self.management(SET_IOP_POWER, u64::from(power_state::INIT))
    }

    /// Starts the application endpoint `endpoint`, once the coprocessor runs.
    pub fn start_endpoint(&mut self, endpoint: u8) -> Result<(), RtkitError> {
        let usable = self.has_endpoint(endpoint)
            && (endpoint < endpoint::FIRST_APPLICATION || self.is_running());
        if !usable {
            return Err(RtkitError::NoSuchEndpoint);
        }
        self.management(
            START_ENDPOINT,
            u64::from(endpoint) << START_ENDPOINT_SHIFT | START_FLAG,
        )
    }

    /// Sends `payload` to `endpoint`.
    pub fn send(&mut self, endpoint: u8, payload: u64) -> Result<(), RtkitError> {
        if self.crashed {
            return Err(RtkitError::Crashed);
        }
        self.mailbox.send(Message { payload, endpoint })?;
        Ok(())
    }

    fn management(&mut self, kind: u64, body: u64) -> Result<(), RtkitError> {
        let payload = body & !(TYPE_MASK << TYPE_SHIFT) | kind << TYPE_SHIFT;
        self.send(endpoint::MANAGEMENT, payload)
    }

    /// Handles the messages waiting, up to [`MOST_MESSAGES_PER_POLL`]: RTKit's own, with
    /// `memory` lending what the system endpoints ask for, and each one for an application
    /// endpoint passed to `application`.
    pub fn poll(
        &mut self,
        memory: &mut dyn SharedMemory,
        application: &mut dyn FnMut(u8, u64),
    ) -> Result<(), RtkitError> {
        for _ in 0..MOST_MESSAGES_PER_POLL {
            let Some(message) = self.mailbox.receive() else {
                break;
            };
            match message.endpoint {
                endpoint::MANAGEMENT => self.on_management(message.payload)?,
                endpoint::CRASH_LOG => self.on_crash_log(memory, message.payload)?,
                endpoint::SYSTEM_LOG => self.on_system_log(memory, message.payload)?,
                endpoint::IO_REPORT => self.on_io_report(memory, message.payload)?,
                endpoint::OS_LOG => self.on_os_log(memory, message.payload)?,
                endpoint if endpoint >= endpoint::FIRST_APPLICATION => {
                    application(endpoint, message.payload);
                }
                _ => {}
            }
        }
        match self.crashed {
            true => Err(RtkitError::Crashed),
            false => Ok(()),
        }
    }

    fn on_management(&mut self, payload: u64) -> Result<(), RtkitError> {
        match payload >> TYPE_SHIFT & TYPE_MASK {
            HELLO => {
                let minimum = payload & VERSION_MASK;
                let maximum = payload >> MAXIMUM_VERSION_SHIFT & VERSION_MASK;
                if minimum > MAXIMUM_VERSION || maximum < MINIMUM_VERSION {
                    return Err(RtkitError::UnsupportedVersion {
                        minimum: minimum as u16,
                        maximum: maximum as u16,
                    });
                }
                let wanted = maximum.min(MAXIMUM_VERSION);
                self.version = Some(wanted as u16);
                self.management(HELLO_REPLY, wanted << MAXIMUM_VERSION_SHIFT | wanted)
            }
            ENDPOINT_MAP => self.on_endpoint_map(payload),
            IOP_POWER_ACK => {
                self.iop_power = (payload & POWER_MASK) as u16;
                if self.iop_power & 0xff == power_state::ON && self.ap_power != power_state::ON {
                    self.management(SET_AP_POWER, u64::from(power_state::ON))?;
                }
                Ok(())
            }
            SET_AP_POWER => {
                self.ap_power = (payload & POWER_MASK) as u16;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn on_endpoint_map(&mut self, payload: u64) -> Result<(), RtkitError> {
        let base = payload >> MAP_BASE_SHIFT & MAP_BASE_MASK;
        let bitmap = payload & MAP_BITMAP_MASK;
        for bit in 0..32 {
            if bitmap & 1 << bit != 0 {
                let endpoint = base * 32 + bit;
                self.endpoints[(endpoint / 64) as usize] |= 1 << (endpoint % 64);
            }
        }
        let last = payload & MAP_LAST != 0;
        let reply = base << MAP_BASE_SHIFT
            | match last {
                true => MAP_LAST,
                false => MAP_REPLY_MORE,
            };
        self.management(ENDPOINT_MAP, reply)?;
        if !last {
            return Ok(());
        }
        self.map_done = true;
        for system in [
            endpoint::CRASH_LOG,
            endpoint::SYSTEM_LOG,
            endpoint::DEBUG,
            endpoint::IO_REPORT,
            endpoint::OS_LOG,
        ] {
            if self.has_endpoint(system) {
                self.start_endpoint(system)?;
            }
        }
        Ok(())
    }

    /// Lends memory for a system endpoint's buffer request and answers it.
    fn give_buffer(
        &mut self,
        memory: &mut dyn SharedMemory,
        endpoint: u8,
        payload: u64,
    ) -> Result<(), RtkitError> {
        let (bytes, asked) = match endpoint {
            endpoint::OS_LOG => (
                payload >> OS_LOG_SIZE_SHIFT & OS_LOG_SIZE_MASK,
                (payload & OS_LOG_ADDRESS_MASK) << PAGE_SHIFT,
            ),
            _ => (
                (payload >> BUFFER_SIZE_SHIFT & BUFFER_SIZE_MASK) << PAGE_SHIFT,
                payload & BUFFER_ADDRESS_MASK,
            ),
        };
        let address = memory
            .shared_memory(endpoint, bytes, asked)
            .ok_or(RtkitError::NoMemory)?;
        let reply = match endpoint {
            endpoint::OS_LOG => {
                BUFFER_REQUEST << OS_LOG_TYPE_SHIFT
                    | (bytes & OS_LOG_SIZE_MASK) << OS_LOG_SIZE_SHIFT
                    | (address >> PAGE_SHIFT) & OS_LOG_ADDRESS_MASK
            }
            _ => {
                BUFFER_REQUEST << TYPE_SHIFT
                    | ((bytes >> PAGE_SHIFT) & BUFFER_SIZE_MASK) << BUFFER_SIZE_SHIFT
                    | address & BUFFER_ADDRESS_MASK
            }
        };
        self.send(endpoint, reply)
    }

    fn on_crash_log(
        &mut self,
        memory: &mut dyn SharedMemory,
        payload: u64,
    ) -> Result<(), RtkitError> {
        if payload >> TYPE_SHIFT & TYPE_MASK != CRASH {
            return Ok(());
        }
        if !self.crash_log_given {
            self.crash_log_given = true;
            return self.give_buffer(memory, endpoint::CRASH_LOG, payload);
        }
        self.crashed = true;
        Ok(())
    }

    fn on_system_log(
        &mut self,
        memory: &mut dyn SharedMemory,
        payload: u64,
    ) -> Result<(), RtkitError> {
        match payload >> TYPE_SHIFT & TYPE_MASK {
            BUFFER_REQUEST => self.give_buffer(memory, endpoint::SYSTEM_LOG, payload),
            SYSTEM_LOG_ENTRY => self.send(endpoint::SYSTEM_LOG, payload),
            SYSTEM_LOG_INIT => Ok(()),
            _ => Ok(()),
        }
    }

    fn on_io_report(
        &mut self,
        memory: &mut dyn SharedMemory,
        payload: u64,
    ) -> Result<(), RtkitError> {
        match payload >> TYPE_SHIFT & TYPE_MASK {
            BUFFER_REQUEST => self.give_buffer(memory, endpoint::IO_REPORT, payload),
            kind if IO_REPORT_ACKED.contains(&kind) => self.send(endpoint::IO_REPORT, payload),
            _ => Ok(()),
        }
    }

    fn on_os_log(&mut self, memory: &mut dyn SharedMemory, payload: u64) -> Result<(), RtkitError> {
        match payload >> OS_LOG_TYPE_SHIFT {
            BUFFER_REQUEST => self.give_buffer(memory, endpoint::OS_LOG, payload),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::mailbox::Layout;
    use super::*;
    use crate::io::fake::FakeRegs;
    use alloc::vec::Vec;

    const SEND: usize = 0x800;
    const RECEIVE_CONTROL: usize = 0x114;
    const RECEIVE: usize = 0x830;

    struct Lender(Vec<(u8, u64, u64)>);

    impl SharedMemory for Lender {
        fn shared_memory(&mut self, endpoint: u8, bytes: u64, address: u64) -> Option<u64> {
            self.0.push((endpoint, bytes, address));
            Some(0x1_0000_0000 + u64::from(endpoint) * 0x10_0000)
        }
    }

    fn coprocessor_says(regs: &FakeRegs, messages: &[(u64, u8)]) {
        let mut controls: Vec<u32> = messages.iter().map(|_| 0).collect();
        controls.push(1 << 17);
        regs.script_reads(RECEIVE_CONTROL, &controls);
        let low: Vec<u32> = messages.iter().map(|&(payload, _)| payload as u32).collect();
        let high: Vec<u32> = messages.iter().map(|&(payload, _)| (payload >> 32) as u32).collect();
        let endpoints: Vec<u32> =
            messages.iter().map(|&(_, endpoint)| u32::from(endpoint)).collect();
        regs.script_reads(RECEIVE, &low);
        regs.script_reads(RECEIVE + 4, &high);
        regs.script_reads(RECEIVE + 8, &endpoints);
        regs.script_reads(RECEIVE + 12, &vec![0; messages.len()]);
    }

    fn sent(rtkit: &Rtkit<FakeRegs>) -> Vec<(u64, u8)> {
        let writes = rtkit.mailbox_registers().writes();
        writes
            .chunks(4)
            .filter(|chunk| chunk.len() == 4 && chunk[0].0 == SEND)
            .map(|chunk| {
                let payload = u64::from(chunk[0].1) | u64::from(chunk[1].1) << 32;
                (payload, chunk[2].1 as u8)
            })
            .collect()
    }

    impl<R: RegisterIo> Rtkit<R> {
        fn mailbox_registers(&self) -> &R {
            self.mailbox.registers()
        }
    }

    #[test]
    fn the_handshake_agrees_a_version_maps_endpoints_and_powers_on() {
        let regs = FakeRegs::default();
        coprocessor_says(
            &regs,
            &[
                (1 << 52 | 12 << 16 | 11, 0),
                (8 << 52 | 0b1_0000_0111, 0),
                (8 << 52 | 1 << 51 | 1 << 32 | 1 << (0x37 - 32), 0),
                (7 << 52 | 0x20, 0),
                (0xb << 52 | 0x20, 0),
            ],
        );
        let mut rtkit = Rtkit::new(Mailbox::new(regs, Layout::Asc));
        let mut lender = Lender(Vec::new());
        rtkit.poll(&mut lender, &mut |_, _| {}).unwrap();
        assert_eq!(rtkit.version(), Some(12));
        assert!(rtkit.has_endpoint(endpoint::CRASH_LOG));
        assert!(rtkit.has_endpoint(endpoint::SYSTEM_LOG));
        assert!(rtkit.has_endpoint(endpoint::OS_LOG));
        assert!(rtkit.has_endpoint(0x37));
        assert!(rtkit.is_running());
        let messages = sent(&rtkit);
        assert_eq!(messages[0], (2 << 52 | 12 << 16 | 12, 0));
        assert_eq!(messages[1], (8 << 52 | 1, 0));
        assert_eq!(messages[2], (8 << 52 | 1 << 51 | 1 << 32, 0));
        assert!(messages.contains(&(5 << 52 | 1 << 32 | 2, 0)));
        assert!(messages.contains(&(5 << 52 | 8 << 32 | 2, 0)));
        assert!(messages.contains(&(0xb << 52 | 0x20, 0)));
    }

    #[test]
    fn a_system_log_buffer_request_is_lent_memory_and_answered() {
        let regs = FakeRegs::default();
        coprocessor_says(&regs, &[(1 << 52 | 2 << 44, endpoint::SYSTEM_LOG)]);
        let mut rtkit = Rtkit::new(Mailbox::new(regs, Layout::Asc));
        let mut lender = Lender(Vec::new());
        rtkit.poll(&mut lender, &mut |_, _| {}).unwrap();
        assert_eq!(lender.0, vec![(endpoint::SYSTEM_LOG, 0x2000, 0)]);
        let reply = 1 << 52 | 2 << 44 | 0x1_0020_0000;
        assert_eq!(sent(&rtkit), vec![(reply, endpoint::SYSTEM_LOG)]);
    }

    #[test]
    fn an_old_firmware_is_refused_and_application_messages_reach_the_driver() {
        let regs = FakeRegs::default();
        coprocessor_says(&regs, &[(0x1234, 0x37), (1 << 52 | 10 << 16 | 9, 0)]);
        let mut rtkit = Rtkit::new(Mailbox::new(regs, Layout::Asc));
        let mut heard = Vec::new();
        let result = rtkit.poll(&mut Lender(Vec::new()), &mut |endpoint, payload| {
            heard.push((endpoint, payload));
        });
        assert_eq!(heard, vec![(0x37, 0x1234)]);
        assert_eq!(
            result,
            Err(RtkitError::UnsupportedVersion {
                minimum: 9,
                maximum: 10,
            })
        );
    }
}
