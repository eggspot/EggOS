//! The ASC mailbox (D543): two FIFOs between the application processor and a coprocessor, one
//! each way. A message is a 64-bit payload and the number of the endpoint it is for, in the low
//! byte of a second word. A full send FIFO and an empty receive FIFO show in each direction's
//! control register.

use crate::io::RegisterIo;

/// The register layouts the mailboxes come in, by their device tree `compatible`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `apple,asc-mailbox-v4`: the M1 and M2 coprocessors'.
    Asc,
    /// `apple,t8015-asc-mailbox`: the A11's, its control registers 8 bytes lower.
    T8015,
    /// `apple,m3-mailbox-v2`: the M3 generation's.
    M3,
}

impl Layout {
    /// The layout of a mailbox node whose `compatible` strings `compatible` lists.
    pub fn of_compatible<'a>(compatible: impl IntoIterator<Item = &'a [u8]>) -> Option<Self> {
        compatible.into_iter().find_map(|name| match name {
            b"apple,asc-mailbox-v4" => Some(Self::Asc),
            b"apple,t8015-asc-mailbox" => Some(Self::T8015),
            b"apple,m3-mailbox-v2" => Some(Self::M3),
            _ => None,
        })
    }

    fn places(self) -> Places {
        match self {
            Self::Asc => Places {
                send_control: 0x110,
                send: 0x800,
                receive_control: 0x114,
                receive: 0x830,
            },
            Self::T8015 => Places {
                send_control: 0x108,
                send: 0x800,
                receive_control: 0x10c,
                receive: 0x830,
            },
            Self::M3 => Places {
                send_control: 0x50,
                send: 0x60,
                receive_control: 0x80,
                receive: 0xa0,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Places {
    send_control: usize,
    send: usize,
    receive_control: usize,
    receive: usize,
}

const CONTROL_FULL: u32 = 1 << 16;
const CONTROL_EMPTY: u32 = 1 << 17;
const SECOND_WORD: usize = 8;
const ENDPOINT_MASK: u64 = 0xff;

/// One message: its payload and its endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message {
    pub payload: u64,
    pub endpoint: u8,
}

/// The send FIFO is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

/// A mailbox, through its registers in `regs`.
#[derive(Debug)]
pub struct Mailbox<R: RegisterIo> {
    regs: R,
    places: Places,
}

impl<R: RegisterIo> Mailbox<R> {
    pub fn new(regs: R, layout: Layout) -> Self {
        Self {
            regs,
            places: layout.places(),
        }
    }

    /// Puts `message` in the send FIFO: [`Full`] when it has no room, which the coprocessor
    /// makes by taking messages.
    pub fn send(&self, message: Message) -> Result<(), Full> {
        if self.regs.read32(self.places.send_control) & CONTROL_FULL != 0 {
            return Err(Full);
        }
        self.regs.write64(self.places.send, message.payload);
        self.regs
            .write64(self.places.send + SECOND_WORD, u64::from(message.endpoint));
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn registers(&self) -> &R {
        &self.regs
    }

    /// The next message from the coprocessor; `None` while its FIFO is empty.
    pub fn receive(&self) -> Option<Message> {
        if self.regs.read32(self.places.receive_control) & CONTROL_EMPTY != 0 {
            return None;
        }
        let payload = self.regs.read64(self.places.receive);
        let endpoint = self.regs.read64(self.places.receive + SECOND_WORD) & ENDPOINT_MASK;
        Some(Message {
            payload,
            endpoint: endpoint as u8,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn a_message_goes_out_as_payload_then_endpoint() {
        let mailbox = Mailbox::new(FakeRegs::default(), Layout::Asc);
        mailbox
            .send(Message {
                payload: 0x0010_0000_0000_000b,
                endpoint: 0x37,
            })
            .unwrap();
        let writes = mailbox.regs.writes();
        assert_eq!(
            writes,
            vec![
                (0x800, 0xb),
                (0x804, 0x0010_0000),
                (0x808, 0x37),
                (0x80c, 0)
            ]
        );
    }

    #[test]
    fn a_full_fifo_refuses_and_an_empty_one_gives_nothing() {
        let regs = FakeRegs::default();
        regs.script_reads(0x110, &[CONTROL_FULL]);
        regs.script_reads(0x114, &[CONTROL_EMPTY]);
        let mailbox = Mailbox::new(regs, Layout::Asc);
        let message = Message {
            payload: 1,
            endpoint: 0,
        };
        assert_eq!(mailbox.send(message), Err(Full));
        assert_eq!(mailbox.receive(), None);
    }

    #[test]
    fn layouts_come_from_the_compatible_strings() {
        let names: [&[u8]; 2] = [b"apple,t6000-asc-mailbox", b"apple,asc-mailbox-v4"];
        assert_eq!(Layout::of_compatible(names), Some(Layout::Asc));
        assert_eq!(
            Layout::of_compatible([&b"apple,m3-mailbox-v2"[..]]),
            Some(Layout::M3)
        );
        assert_eq!(Layout::of_compatible([&b"arm,pl011"[..]]), None);
    }
}
