use super::*;
use crate::dma::DmaAddr;
use crate::dma::fake::HostDma;
use std::cell::RefCell;
use std::sync::Mutex;

pub(crate) const LOW_BASE: u64 = 0x10_0000;

#[derive(Debug)]
pub(crate) struct LowDma(pub HostDma);

// SAFETY: `HostDma`'s memory, which the fake channel subsystem reaches by subtracting
// `LOW_BASE` from the channel address and adding the CPU address.
unsafe impl DmaMemory for LowDma {
    fn as_mut_ptr(&self) -> *mut u8 {
        self.0.as_mut_ptr()
    }
    fn dma_addr(&self) -> DmaAddr {
        DmaAddr::new(LOW_BASE)
    }
    fn len(&self) -> usize {
        self.0.len()
    }
}

pub(crate) struct FakeDevice {
    pub cpu_base: usize,
    pub enabled: bool,
    pub pending: Option<Irb>,
    pub commands: Vec<(u8, u16)>,
    pub answer: Box<dyn FnMut(u8, &mut [u8]) -> u8 + Send>,
}

pub(crate) struct FakeChannels(pub Mutex<RefCell<FakeDevice>>);

impl core::fmt::Debug for FakeChannels {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("FakeChannels")
    }
}

// SAFETY: the mutex serializes the fake device.
unsafe impl Sync for FakeChannels {}

pub(crate) const THE_SUBCHANNEL: SubchannelId = SubchannelId::new(0, 0);

impl ChannelSubsystem for FakeChannels {
    fn store_subchannel(&self, subchannel: SubchannelId, schib: &mut Schib) -> ConditionCode {
        if subchannel != THE_SUBCHANNEL {
            return ConditionCode::NOT_OPERATIONAL;
        }
        let guard = self.0.lock().unwrap();
        let device = guard.borrow();
        *schib = Schib::default();
        schib.0[PMCW_FLAGS] = DEVICE_NUMBER_VALID | if device.enabled { ENABLED } else { 0 };
        schib.0[PMCW_DEVICE_NUMBER..PMCW_DEVICE_NUMBER + 2].copy_from_slice(&0x0002u16.to_be_bytes());
        ConditionCode::DONE
    }

    fn modify_subchannel(&self, _subchannel: SubchannelId, schib: &Schib) -> ConditionCode {
        let guard = self.0.lock().unwrap();
        guard.borrow_mut().enabled = schib.is_enabled();
        ConditionCode::DONE
    }

    unsafe fn start_subchannel(&self, _subchannel: SubchannelId, orb: &Orb) -> ConditionCode {
        let guard = self.0.lock().unwrap();
        let mut device = guard.borrow_mut();
        if !device.enabled {
            return ConditionCode::NOT_OPERATIONAL;
        }
        let program = u32::from_be_bytes([orb.0[8], orb.0[9], orb.0[10], orb.0[11]]) as u64;
        let cpu = |address: u64| device.cpu_base + (address - LOW_BASE) as usize;
        let word_at = cpu(program) as *const u8;
        let mut word = [0u8; CCW_BYTES];
        // SAFETY: the test's buffer holds the word at that address.
        unsafe { core::ptr::copy_nonoverlapping(word_at, word.as_mut_ptr(), CCW_BYTES) };
        let command = word[0];
        let count = u16::from_be_bytes([word[2], word[3]]);
        let data = u32::from_be_bytes([word[4], word[5], word[6], word[7]]) as u64;
        // SAFETY: the test's buffer holds `count` data bytes there.
        let bytes = unsafe { core::slice::from_raw_parts_mut(cpu(data) as *mut u8, usize::from(count)) };
        device.commands.push((command, count));
        let status = (device.answer)(command, bytes);
        let mut irb = Irb::default();
        irb.0[3] = status_control::PRIMARY | status_control::SECONDARY | status_control::PENDING;
        irb.0[8] = status;
        device.pending = Some(irb);
        ConditionCode::DONE
    }

    fn test_subchannel(&self, _subchannel: SubchannelId, irb: &mut Irb) -> ConditionCode {
        let guard = self.0.lock().unwrap();
        match guard.borrow_mut().pending.take() {
            Some(pending) => {
                *irb = pending;
                ConditionCode::DONE
            }
            None => ConditionCode::STATUS_PENDING,
        }
    }

    fn notify_virtio_queue(&self, _subchannel: SubchannelId, queue: u16, _cookie: u64) -> i64 {
        let guard = self.0.lock().unwrap();
        guard.borrow_mut().commands.push((0, queue));
        0
    }
}

pub(crate) fn fake(answer: Box<dyn FnMut(u8, &mut [u8]) -> u8 + Send>) -> (FakeChannels, LowDma) {
    let memory = LowDma(HostDma::zeroed(1));
    let channels = FakeChannels(Mutex::new(RefCell::new(FakeDevice {
        cpu_base: memory.as_mut_ptr() as usize,
        enabled: false,
        pending: None,
        commands: Vec::new(),
        answer,
    })));
    (channels, memory)
}

const GOOD: u8 = device_status::CHANNEL_END | device_status::DEVICE_END;

#[test]
fn subchannel_word_has_its_one_bit_and_set() {
    let id = SubchannelId::new(2, 0x1234);
    assert_eq!(id.word(), 0x0005_1234);
    assert_eq!((id.set(), id.number()), (2, 0x1234));
    assert_eq!(SubchannelId::from_word(0x1234), None);
    assert_eq!(id.to_string(), "0.2.1234");
}

#[test]
fn format1_word_and_request_are_big_endian() {
    assert_eq!(ccw1(0xe4, 0x20, 7, 0x8000_1000), [0xe4, 0x20, 0, 7, 0, 0, 0x10, 0]);
    let orb = Orb::format1(0x0001_0000, 0x1000, 0xff);
    assert_eq!(&orb.0[..12], &[0, 1, 0, 0, 0, 0x80, 0xff, 0, 0, 0, 0x10, 0]);
}

#[test]
fn enable_sets_the_enable_bit_and_subclass() {
    let mut schib = Schib::default();
    schib.enable(3, 7);
    assert!(schib.is_enabled());
    assert_eq!(schib.0[PMCW_ISC_ENABLE] >> ISC_SHIFT, 3);
    assert_eq!(&schib.0[..4], &[0, 0, 0, 7]);
    let (channels, _memory) = fake(Box::new(|_, _| GOOD));
    let enabled = enable(&channels, THE_SUBCHANNEL).unwrap();
    assert!(enabled.is_enabled());
    assert_eq!(enabled.device_number(), 2);
    assert_eq!(enable(&channels, SubchannelId::new(0, 1)), Err(ChannelError::NotOperational));
}

#[test]
fn sense_id_reads_the_control_unit() {
    let (channels, memory) = fake(Box::new(|command, data| {
        assert_eq!(command, SENSE_ID);
        data.copy_from_slice(&[0xff, 0x38, 0x32, 0x02, 0, 0, 0]);
        GOOD
    }));
    enable(&channels, THE_SUBCHANNEL).unwrap();
    let mut buffer = ChannelBuffer::new(memory).unwrap();
    let id = sense_id(&channels, THE_SUBCHANNEL, &mut buffer).unwrap();
    assert_eq!((id.control_unit_type, id.control_unit_model), (0x3832, 2));
}

#[test]
fn unit_check_is_a_refusal() {
    let (channels, memory) = fake(Box::new(|_, _| GOOD | device_status::UNIT_CHECK));
    enable(&channels, THE_SUBCHANNEL).unwrap();
    let mut buffer = ChannelBuffer::new(memory).unwrap();
    assert_eq!(buffer.run(&channels, THE_SUBCHANNEL, 0x83, 4), Err(ChannelError::UnitCheck));
}

#[test]
fn memory_above_two_gibibytes_is_refused() {
    #[derive(Debug)]
    struct High(HostDma);
    // SAFETY: as `LowDma`, at an address no channel reaches.
    unsafe impl DmaMemory for High {
        fn as_mut_ptr(&self) -> *mut u8 {
            self.0.as_mut_ptr()
        }
        fn dma_addr(&self) -> DmaAddr {
            DmaAddr::new(1 << 31)
        }
        fn len(&self) -> usize {
            self.0.len()
        }
    }
    assert_eq!(
        ChannelBuffer::new(High(HostDma::zeroed(1))).err().map(|(error, _)| error),
        Some(ChannelError::OutOfReach)
    );
}
