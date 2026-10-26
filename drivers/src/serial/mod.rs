//! Serial ports (UARTs): the early boot console, and from M3 on its input too.

pub mod ns16550;
pub mod pl011;

pub use ns16550::Ns16550;
pub use pl011::Pl011;

/// A UART that transmits and receives bytes.
pub trait Uart {
    /// Transmits one byte, waiting while the transmitter is full.
    fn put_byte(&self, byte: u8);

    /// Transmits `bytes`, turning each `\n` into `\r\n` as serial terminals expect.
    #[inline]
    fn write_bytes(&self, bytes: &[u8]) {
        for &byte in bytes {
            if byte == b'\n' {
                self.put_byte(b'\r');
            }
            self.put_byte(byte);
        }
    }

    /// Takes the oldest received byte, or `None` when nothing is waiting. Never waits.
    fn receive_byte(&self) -> Option<u8>;

    /// Turns the receive FIFO on and has the UART raise its interrupt while received bytes wait,
    /// until [`Self::receive_byte`] has taken them all. Bytes received before the call may be
    /// dropped: turning the FIFO on empties it.
    fn enable_receive_interrupt(&self);

    /// Takes received bytes into `buffer` until it is full or nothing is waiting; returns how
    /// many it took.
    #[inline]
    fn read_bytes(&self, buffer: &mut [u8]) -> usize {
        let mut taken = 0;
        while let Some(slot) = buffer.get_mut(taken) {
            let Some(byte) = self.receive_byte() else {
                break;
            };
            *slot = byte;
            taken += 1;
        }
        taken
    }
}
