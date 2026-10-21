//! Serial ports (UARTs), used first as the early boot console.

pub mod ns16550;
pub mod pl011;

pub use ns16550::Ns16550;
pub use pl011::Pl011;

/// A UART that can transmit bytes.
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
}
