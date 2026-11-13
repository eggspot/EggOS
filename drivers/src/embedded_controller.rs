//! The ACPI embedded controller (ACPI 6.5, 12.2 and 12.3; D539): a byte-wide command and status
//! port and a data port, through which the operating system reads and writes the controller's
//! 256-byte address space and collects the events it raises (`SCI_EVT`) as query values, which
//! name the `_Qxx` methods to run.

/// The two ports of an embedded controller.
pub trait EcPorts {
    fn read_status(&mut self) -> u8;
    fn write_command(&mut self, command: u8);
    fn read_data(&mut self) -> u8;
    fn write_data(&mut self, value: u8);
}

/// The status register's bits.
pub mod status {
    /// The data port holds a byte for the host.
    pub const OUTPUT_FULL: u8 = 1 << 0;
    /// The controller has not taken the last byte the host wrote.
    pub const INPUT_FULL: u8 = 1 << 1;
    /// The controller has an event to report through `QR_EC`.
    pub const EVENT: u8 = 1 << 5;
}

const READ: u8 = 0x80;
const WRITE: u8 = 0x81;
const QUERY: u8 = 0x84;

/// The controller did not answer within the polls allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeout;

/// An embedded controller behind `ports`, each wait polling its status at most `polls` times.
#[derive(Debug)]
pub struct EmbeddedController<T: EcPorts> {
    ports: T,
    polls: u32,
}

impl<T: EcPorts> EmbeddedController<T> {
    pub const fn new(ports: T, polls: u32) -> Self {
        Self { ports, polls }
    }

    fn wait_for(&mut self, done: impl Fn(u8) -> bool) -> Result<(), Timeout> {
        for _ in 0..self.polls {
            if done(self.ports.read_status()) {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(Timeout)
    }

    fn send_command(&mut self, command: u8) -> Result<(), Timeout> {
        self.wait_for(|status| status & status::INPUT_FULL == 0)?;
        self.ports.write_command(command);
        Ok(())
    }

    fn send_data(&mut self, value: u8) -> Result<(), Timeout> {
        self.wait_for(|status| status & status::INPUT_FULL == 0)?;
        self.ports.write_data(value);
        Ok(())
    }

    fn receive_data(&mut self) -> Result<u8, Timeout> {
        self.wait_for(|status| status & status::OUTPUT_FULL != 0)?;
        Ok(self.ports.read_data())
    }

    /// `RD_EC`: the byte at `address` of the controller's space.
    pub fn read(&mut self, address: u8) -> Result<u8, Timeout> {
        self.send_command(READ)?;
        self.send_data(address)?;
        self.receive_data()
    }

    /// `WR_EC`: writes `value` at `address` of the controller's space.
    pub fn write(&mut self, address: u8, value: u8) -> Result<(), Timeout> {
        self.send_command(WRITE)?;
        self.send_data(address)?;
        self.send_data(value)?;
        self.wait_for(|status| status & status::INPUT_FULL == 0)
    }

    /// Whether the controller has an event waiting.
    pub fn has_event(&mut self) -> bool {
        self.ports.read_status() & status::EVENT != 0
    }

    /// `QR_EC`: the next event's query value, which names its `_Qxx` method; 0 when none is
    /// left.
    pub fn query(&mut self) -> Result<u8, Timeout> {
        self.send_command(QUERY)?;
        self.receive_data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::VecDeque;

    #[derive(Debug)]
    struct FakeController {
        space: [u8; 256],
        command: Option<u8>,
        arguments: VecDeque<u8>,
        output: Option<u8>,
        events: VecDeque<u8>,
        busy_polls: u32,
    }

    impl Default for FakeController {
        fn default() -> Self {
            Self {
                space: [0; 256],
                command: None,
                arguments: VecDeque::new(),
                output: None,
                events: VecDeque::new(),
                busy_polls: 0,
            }
        }
    }

    impl EcPorts for FakeController {
        fn read_status(&mut self) -> u8 {
            if self.busy_polls > 0 {
                self.busy_polls -= 1;
                return status::INPUT_FULL;
            }
            let mut value = 0;
            if self.output.is_some() {
                value |= status::OUTPUT_FULL;
            }
            if !self.events.is_empty() {
                value |= status::EVENT;
            }
            value
        }

        fn write_command(&mut self, command: u8) {
            self.command = Some(command);
            self.arguments.clear();
            if command == QUERY {
                self.output = Some(self.events.pop_front().unwrap_or(0));
            }
            self.busy_polls = 2;
        }

        fn read_data(&mut self) -> u8 {
            self.output.take().unwrap_or(0xff)
        }

        fn write_data(&mut self, value: u8) {
            self.arguments.push_back(value);
            match (self.command, self.arguments.len()) {
                (Some(READ), 1) => self.output = Some(self.space[usize::from(value)]),
                (Some(WRITE), 2) => {
                    let address = usize::from(self.arguments[0]);
                    self.space[address] = value;
                }
                _ => {}
            }
        }
    }

    #[test]
    fn bytes_are_written_then_read_back_through_the_commands() {
        let mut controller = EmbeddedController::new(FakeController::default(), 100);
        controller.write(0x40, 0x5a).unwrap();
        assert_eq!(controller.read(0x40), Ok(0x5a));
        assert_eq!(controller.read(0x41), Ok(0));
    }

    #[test]
    fn events_are_queried_until_none_is_left() {
        let mut fake = FakeController::default();
        fake.events.extend([0x1d, 0x42]);
        let mut controller = EmbeddedController::new(fake, 100);
        assert!(controller.has_event());
        assert_eq!(controller.query(), Ok(0x1d));
        assert_eq!(controller.query(), Ok(0x42));
        assert!(!controller.has_event());
        assert_eq!(controller.query(), Ok(0));
    }

    #[test]
    fn a_controller_that_never_takes_input_times_out() {
        let fake = FakeController {
            busy_polls: u32::MAX,
            ..FakeController::default()
        };
        let mut controller = EmbeddedController::new(fake, 10);
        assert_eq!(controller.read(0), Err(Timeout));
    }
}
