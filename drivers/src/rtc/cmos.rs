//! The PC's real-time clock (D150): the Motorola MC146818 and the compatible clock every PC
//! chipset and QEMU keep, behind two I/O ports, an index (0x70) and a data port (0x71), over a
//! small battery-backed memory whose first registers hold the time of day. The firmware chooses
//! BCD or binary and a 12- or 24-hour clock (status register B); the century, if kept at all, is
//! in a register the ACPI FADT names. A read waits while the clock updates (status register A's
//! update-in-progress flag) and takes two snapshots in a row until they agree, so it never mixes
//! two seconds; a write holds the clock (status B's `SET`) while it stores the fields.

use super::{NANOSECONDS_PER_SECOND, RealTimeClock, RtcError};
use crate::io::RegisterIo;
use eggos_calendar::DateTime;

/// The register block: the index port, then the data port.
pub const REGISTER_BLOCK_SIZE: usize = 2;

/// The first register a century may be kept in: those below are the clock's own.
pub const FIRST_CENTURY_REGISTER: u8 = 0x0e;

/// The last register a century may be kept in: the index port takes seven bits.
pub const LAST_CENTURY_REGISTER: u8 = 0x7f;

const INDEX_PORT: usize = 0;
const DATA_PORT: usize = 1;

const SECONDS: u8 = 0x00;
const MINUTES: u8 = 0x02;
const HOURS: u8 = 0x04;
const WEEKDAY: u8 = 0x06;
const DAY: u8 = 0x07;
const MONTH: u8 = 0x08;
const YEAR: u8 = 0x09;
const STATUS_A: u8 = 0x0a;
const STATUS_B: u8 = 0x0b;

const UPDATE_IN_PROGRESS: u8 = 0x80;
const SET: u8 = 0x80;
const BINARY: u8 = 0x04;
const HOURS_24: u8 = 0x02;
const PM: u8 = 0x80;

const UPDATE_POLLS: u32 = 100_000;
const READ_ATTEMPTS: u32 = 8;
const YEAR_WITHOUT_CENTURY_FROM: u8 = 70;
const FIRST_YEAR_WITHOUT_CENTURY: i64 = 1970;
const LAST_YEAR_WITHOUT_CENTURY: i64 = 2069;
const LAST_YEAR_WITH_CENTURY: i64 = 9999;

/// The binary value of the BCD byte `bcd` (two decimal digits); `None` if a digit is above 9.
pub const fn bcd_to_binary(bcd: u8) -> Option<u8> {
    let (tens, ones) = (bcd >> 4, bcd & 0x0f);
    if tens > 9 || ones > 9 {
        return None;
    }
    Some(tens * 10 + ones)
}

/// `value` (0 to 99) as a BCD byte.
pub const fn binary_to_bcd(value: u8) -> u8 {
    ((value / 10) << 4) | (value % 10)
}

/// The hour of the day (0 to 23) a 12-hour clock shows as `hour` (1 to 12) with `pm`; `None`
/// for an hour that is not 1 to 12.
pub const fn hour_from_12(hour: u8, pm: bool) -> Option<u8> {
    if hour == 0 || hour > 12 {
        return None;
    }
    Some(hour % 12 + if pm { 12 } else { 0 })
}

/// What a 12-hour clock shows for the hour of the day `hour` (0 to 23): the hour (1 to 12)
/// and whether it is PM.
pub const fn hour_to_12(hour: u8) -> (u8, bool) {
    let shown = match hour % 12 {
        0 => 12,
        other => other,
    };
    (shown, hour >= 12)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Snapshot {
    second: u8,
    minute: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8,
    century: u8,
    status_b: u8,
}

/// An MC146818-compatible clock.
#[derive(Debug)]
pub struct Cmos<R: RegisterIo> {
    regs: R,
    century: Option<u8>,
}

impl<R: RegisterIo> Cmos<R> {
    /// The clock behind `regs` ([`REGISTER_BLOCK_SIZE`] ports), keeping its century in register
    /// `century` if the firmware names one (the FADT's `CENTURY`, 0x32 on QEMU and most PCs)
    /// from [`FIRST_CENTURY_REGISTER`] to [`LAST_CENTURY_REGISTER`]. Without one, years 70 to 99
    /// are 1970 to 1999 and 00 to 69 are 2000 to 2069, as Linux reads them.
    pub fn new(regs: R, century: Option<u8>) -> Self {
        let century = century
            .filter(|register| (FIRST_CENTURY_REGISTER..=LAST_CENTURY_REGISTER).contains(register));
        Self { regs, century }
    }

    fn register(&self, index: u8) -> u8 {
        self.regs.write8(INDEX_PORT, index);
        self.regs.read8(DATA_PORT)
    }

    fn set_register(&self, index: u8, value: u8) {
        self.regs.write8(INDEX_PORT, index);
        self.regs.write8(DATA_PORT, value);
    }

    fn snapshot(&self) -> Result<Snapshot, RtcError> {
        let mut polls = 0;
        while self.register(STATUS_A) & UPDATE_IN_PROGRESS != 0 {
            polls += 1;
            if polls == UPDATE_POLLS {
                return Err(RtcError::Busy);
            }
        }
        Ok(Snapshot {
            second: self.register(SECONDS),
            minute: self.register(MINUTES),
            hour: self.register(HOURS),
            day: self.register(DAY),
            month: self.register(MONTH),
            year: self.register(YEAR),
            century: self.century.map_or(0, |register| self.register(register)),
            status_b: self.register(STATUS_B),
        })
    }

    fn decode(&self, snapshot: Snapshot) -> Option<u64> {
        let binary = snapshot.status_b & BINARY != 0;
        let field = |value: u8| match binary {
            true => Some(value),
            false => bcd_to_binary(value),
        };
        let hour = match snapshot.status_b & HOURS_24 != 0 {
            true => field(snapshot.hour)?,
            false => hour_from_12(field(snapshot.hour & !PM)?, snapshot.hour & PM != 0)?,
        };
        let year_of_century = field(snapshot.year).filter(|&year| year < 100)?;
        let year = match self.century {
            Some(_) => {
                let century = field(snapshot.century).filter(|&century| century < 100)?;
                i64::from(century) * 100 + i64::from(year_of_century)
            }
            None if year_of_century >= YEAR_WITHOUT_CENTURY_FROM => {
                1900 + i64::from(year_of_century)
            }
            None => 2000 + i64::from(year_of_century),
        };
        let moment = DateTime {
            year,
            month: u32::from(field(snapshot.month)?),
            day: u32::from(field(snapshot.day)?),
            hour: u32::from(hour),
            minute: u32::from(field(snapshot.minute)?),
            second: u32::from(field(snapshot.second)?),
        };
        u64::try_from(moment.to_unix()?).ok()
    }
}

impl<R: RegisterIo> RealTimeClock for Cmos<R> {
    fn name(&self) -> &'static str {
        "rtc-cmos"
    }

    fn read(&self) -> Result<u64, RtcError> {
        for _ in 0..READ_ATTEMPTS {
            let first = self.snapshot()?;
            if self.snapshot()? == first {
                let seconds = self.decode(first).ok_or(RtcError::Invalid)?;
                return Ok(seconds * NANOSECONDS_PER_SECOND);
            }
        }
        Err(RtcError::Busy)
    }

    fn write(&self, nanoseconds: u64) -> Result<(), RtcError> {
        let seconds = i64::try_from(nanoseconds / NANOSECONDS_PER_SECOND)
            .map_err(|_| RtcError::OutOfRange)?;
        let moment = DateTime::from_unix(seconds);
        let years = match self.century {
            Some(_) => 0..=LAST_YEAR_WITH_CENTURY,
            None => FIRST_YEAR_WITHOUT_CENTURY..=LAST_YEAR_WITHOUT_CENTURY,
        };
        if !years.contains(&moment.year) {
            return Err(RtcError::OutOfRange);
        }
        let status_b = self.register(STATUS_B);
        let encode = |value: u32| match status_b & BINARY != 0 {
            true => value as u8,
            false => binary_to_bcd(value as u8),
        };
        let hour = match status_b & HOURS_24 != 0 {
            true => encode(moment.hour),
            false => {
                let (shown, pm) = hour_to_12(moment.hour as u8);
                encode(u32::from(shown)) | if pm { PM } else { 0 }
            }
        };
        self.set_register(STATUS_B, status_b | SET);
        for (register, value) in [
            (SECONDS, encode(moment.second)),
            (MINUTES, encode(moment.minute)),
            (HOURS, hour),
            (WEEKDAY, encode(moment.weekday() as u32 + 1)),
            (DAY, encode(moment.day)),
            (MONTH, encode(moment.month)),
            (YEAR, encode((moment.year % 100) as u32)),
        ] {
            self.set_register(register, value);
        }
        if let Some(register) = self.century {
            self.set_register(register, encode((moment.year / 100) as u32));
        }
        self.set_register(STATUS_B, status_b & !SET);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::{Cell, RefCell};

    const CENTURY: u8 = 0x32;
    const NOW: u64 = 1_793_282_709;

    #[derive(Debug)]
    struct FakeCmos {
        memory: RefCell<[u8; 128]>,
        index: Cell<usize>,
        busy_polls: Cell<u32>,
        data_reads: Cell<u32>,
        change_after_reads: Cell<Option<(u32, u8, u8)>>,
    }

    impl FakeCmos {
        fn with(status_b: u8, fields: [(u8, u8); 7]) -> Self {
            let mut memory = [0u8; 128];
            memory[usize::from(STATUS_A)] = 0x26;
            memory[usize::from(STATUS_B)] = status_b;
            for (register, value) in fields {
                memory[usize::from(register)] = value;
            }
            Self {
                memory: RefCell::new(memory),
                index: Cell::new(0),
                busy_polls: Cell::new(0),
                data_reads: Cell::new(0),
                change_after_reads: Cell::new(None),
            }
        }

        fn at(&self, register: u8) -> u8 {
            self.memory.borrow()[usize::from(register)]
        }
    }

    impl RegisterIo for &FakeCmos {
        fn read8(&self, offset: usize) -> u8 {
            assert_eq!(offset, DATA_PORT);
            let reads = self.data_reads.get() + 1;
            self.data_reads.set(reads);
            if let Some((after, register, value)) = self.change_after_reads.get()
                && reads > after
            {
                self.memory.borrow_mut()[usize::from(register)] = value;
                self.change_after_reads.set(None);
            }
            let index = self.index.get();
            if index == usize::from(STATUS_A) && self.busy_polls.get() > 0 {
                self.busy_polls.set(self.busy_polls.get() - 1);
                return self.memory.borrow()[index] | UPDATE_IN_PROGRESS;
            }
            self.memory.borrow()[index]
        }

        fn write8(&self, offset: usize, value: u8) {
            match offset {
                INDEX_PORT => self.index.set(usize::from(value & 0x7f)),
                DATA_PORT => self.memory.borrow_mut()[self.index.get()] = value,
                _ => panic!("no port {offset}"),
            }
        }

        fn read16(&self, _offset: usize) -> u16 {
            unreachable!("the clock's ports are bytes")
        }

        fn write16(&self, _offset: usize, _value: u16) {
            unreachable!("the clock's ports are bytes")
        }

        fn read32(&self, _offset: usize) -> u32 {
            unreachable!("the clock's ports are bytes")
        }

        fn write32(&self, _offset: usize, _value: u32) {
            unreachable!("the clock's ports are bytes")
        }
    }

    fn qemu_clock(year: u8, century: u8) -> FakeCmos {
        let fake = FakeCmos::with(
            HOURS_24,
            [
                (SECONDS, 0x09),
                (MINUTES, 0x05),
                (HOURS, 0x14),
                (DAY, 0x29),
                (MONTH, 0x10),
                (YEAR, year),
                (WEEKDAY, 0x05),
            ],
        );
        fake.memory.borrow_mut()[usize::from(CENTURY)] = century;
        fake
    }

    #[test]
    fn bcd_and_twelve_hour_conversions_are_exact() {
        assert_eq!(bcd_to_binary(0x59), Some(59));
        assert_eq!(bcd_to_binary(0x00), Some(0));
        assert_eq!(bcd_to_binary(0x1a), None);
        assert_eq!(bcd_to_binary(0xa1), None);
        for value in 0..100 {
            assert_eq!(bcd_to_binary(binary_to_bcd(value)), Some(value));
        }
        assert_eq!(hour_from_12(12, false), Some(0));
        assert_eq!(hour_from_12(12, true), Some(12));
        assert_eq!(hour_from_12(1, true), Some(13));
        assert_eq!(hour_from_12(11, false), Some(11));
        assert_eq!(hour_from_12(0, false), None);
        assert_eq!(hour_from_12(13, true), None);
        for hour in 0..24 {
            let (shown, pm) = hour_to_12(hour);
            assert_eq!(hour_from_12(shown, pm), Some(hour));
        }
    }

    #[test]
    fn a_bcd_24_hour_clock_with_its_century_reads_as_qemu_keeps_it() {
        let fake = qemu_clock(0x26, 0x20);
        let clock = Cmos::new(&fake, Some(CENTURY));
        assert_eq!(clock.read(), Ok(NOW * NANOSECONDS_PER_SECOND));
        assert_eq!(clock.name(), "rtc-cmos");
    }

    #[test]
    fn without_a_century_register_two_digit_years_split_at_seventy() {
        let fake = qemu_clock(0x26, 0x19);
        assert_eq!(
            Cmos::new(&fake, None).read(),
            Ok(NOW * NANOSECONDS_PER_SECOND)
        );
        assert_eq!(
            Cmos::new(&fake, Some(0x0d)).read(),
            Ok(NOW * NANOSECONDS_PER_SECOND)
        );
        let fake = qemu_clock(0x99, 0x19);
        let seconds = Cmos::new(&fake, None).read().unwrap() / NANOSECONDS_PER_SECOND;
        assert_eq!(DateTime::from_unix(seconds as i64).year, 1999);
    }

    #[test]
    fn a_binary_12_hour_clock_reads_its_pm_flag() {
        for (hour, expected) in [(2 | PM, 14), (12, 0), (12 | PM, 12)] {
            let fake = FakeCmos::with(
                BINARY,
                [
                    (SECONDS, 9),
                    (MINUTES, 5),
                    (HOURS, hour),
                    (DAY, 29),
                    (MONTH, 10),
                    (YEAR, 26),
                    (WEEKDAY, 5),
                ],
            );
            fake.memory.borrow_mut()[usize::from(CENTURY)] = 20;
            let read = Cmos::new(&fake, Some(CENTURY)).read().unwrap() / NANOSECONDS_PER_SECOND;
            assert_eq!(DateTime::from_unix(read as i64).hour, expected);
        }
    }

    #[test]
    fn a_read_waits_out_an_update_and_takes_two_equal_snapshots() {
        let fake = qemu_clock(0x26, 0x20);
        fake.busy_polls.set(3);
        fake.change_after_reads.set(Some((5, SECONDS, 0x10)));
        let clock = Cmos::new(&fake, Some(CENTURY));
        assert_eq!(clock.read(), Ok((NOW + 1) * NANOSECONDS_PER_SECOND));
        fake.busy_polls.set(u32::MAX);
        assert_eq!(clock.read(), Err(RtcError::Busy));
    }

    #[test]
    fn registers_that_hold_no_date_are_invalid() {
        for (register, value) in [(MONTH, 0x13), (DAY, 0x00), (SECONDS, 0x1a), (HOURS, 0x24)] {
            let fake = qemu_clock(0x26, 0x20);
            fake.memory.borrow_mut()[usize::from(register)] = value;
            assert_eq!(
                Cmos::new(&fake, Some(CENTURY)).read(),
                Err(RtcError::Invalid),
                "{register:#x}"
            );
        }
    }

    #[test]
    fn a_write_stores_every_field_in_the_clock_s_format_while_it_is_held() {
        let fake = qemu_clock(0x00, 0x00);
        let clock = Cmos::new(&fake, Some(CENTURY));
        clock.write(NOW * NANOSECONDS_PER_SECOND + 999).unwrap();
        assert_eq!(fake.at(STATUS_B), HOURS_24);
        for (register, value) in [
            (SECONDS, 0x09),
            (MINUTES, 0x05),
            (HOURS, 0x14),
            (WEEKDAY, 0x05),
            (DAY, 0x29),
            (MONTH, 0x10),
            (YEAR, 0x26),
            (CENTURY, 0x20),
        ] {
            assert_eq!(fake.at(register), value, "{register:#x}");
        }
        assert_eq!(clock.read(), Ok(NOW * NANOSECONDS_PER_SECOND));

        let fake = FakeCmos::with(BINARY, [(SECONDS, 0); 7]);
        let clock = Cmos::new(&fake, None);
        clock.write(NOW * NANOSECONDS_PER_SECOND).unwrap();
        assert_eq!((fake.at(HOURS), fake.at(YEAR)), (2 | PM, 26));
        assert_eq!(clock.read(), Ok(NOW * NANOSECONDS_PER_SECOND));
        let year_2070 = 3_155_760_000 * NANOSECONDS_PER_SECOND;
        assert_eq!(clock.write(year_2070), Err(RtcError::OutOfRange));
    }
}
