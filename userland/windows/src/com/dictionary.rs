//! `Scripting.Dictionary` (`{EE09B103-97E0-11CF-978F-00A02463E06F}`), the class the layer
//! provides in process (D178): keys of any kind but arrays, each with an item, kept in the order
//! they were added and found through an open-addressing table of entry numbers. Keys compare as
//! `CompareMode` says: binary (0, the default: strings match unit for unit, a number matches any
//! number of equal value, objects match by pointer) or text (1 and above: strings ignore case).
//! Programs reach it through `IDispatch` (`GetIDsOfNames` ignoring case, `Invoke` with
//! positional arguments in reverse order and the value of a property put as the named argument
//! `DISPID_PROPERTYPUT`) or through its dual `IDictionary` table; errors through `Invoke` come
//! back as `DISP_E_EXCEPTION` with an `EXCEPINFO`. `_NewEnum` gives an `IEnumVARIANT` over the
//! keys as they were then. Apartment model: one thread uses the object at a time; its reference
//! count is atomic.

use super::guid::{self, Guid};
use super::safe_array::{self, Bound, SafeArray};
use super::variant::{self, Variant, kind};
use super::{E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, HResult, S_OK, bstr, conversion, enumerator};
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

/// `CLSID_Dictionary`.
pub const CLASS: Guid = Guid::new(
    0xee09_b103,
    0x97e0,
    0x11cf,
    [0x97, 0x8f, 0x00, 0xa0, 0x24, 0x63, 0xe0, 0x6f],
);
/// `IID_IDictionary`.
pub const INTERFACE: Guid = Guid::new(
    0x42c6_42c1,
    0x97e1,
    0x11cf,
    [0x97, 0x8f, 0x00, 0xa0, 0x24, 0x63, 0xe0, 0x6f],
);
/// The class's ProgID.
pub const PROGRAM_ID: &[u8] = b"Scripting.Dictionary";

const DISP_E_UNKNOWNINTERFACE: HResult = 0x8002_0001_u32 as i32;
const DISP_E_MEMBERNOTFOUND: HResult = 0x8002_0003_u32 as i32;
const DISP_E_PARAMNOTFOUND: HResult = 0x8002_0004_u32 as i32;
const DISP_E_UNKNOWNNAME: HResult = 0x8002_0006_u32 as i32;
const DISP_E_NONAMEDARGS: HResult = 0x8002_0007_u32 as i32;
const DISP_E_EXCEPTION: HResult = 0x8002_0009_u32 as i32;
const DISP_E_BADPARAMCOUNT: HResult = 0x8002_000e_u32 as i32;
const INVALID_CALL: HResult = 0x800a_0005_u32 as i32;
const KEY_EXISTS: HResult = 0x800a_01c9_u32 as i32;
const ELEMENT_NOT_FOUND: HResult = 0x800a_802b_u32 as i32;
const FACILITY_CONTROL: u32 = 0x800a;

const DISPATCH_METHOD: u16 = 0x1;
const DISPATCH_PROPERTYGET: u16 = 0x2;
const DISPATCH_PROPERTYPUT: u16 = 0x4;
const DISPATCH_PROPERTYPUTREF: u16 = 0x8;
const DISPID_UNKNOWN: i32 = -1;
const DISPID_PROPERTYPUT: i32 = -3;

const ITEM: i32 = 0;
const ADD: i32 = 1;
const COUNT: i32 = 2;
const EXISTS: i32 = 3;
const ITEMS: i32 = 4;
const KEY: i32 = 5;
const KEYS: i32 = 6;
const REMOVE: i32 = 7;
const REMOVE_ALL: i32 = 8;
const COMPARE_MODE: i32 = 9;
const HASH_VALUE: i32 = 10;
const NEW_ENUM: i32 = -4;

const MEMBERS: [(&[u8], i32); 12] = [
    (b"Item", ITEM),
    (b"Add", ADD),
    (b"Count", COUNT),
    (b"Exists", EXISTS),
    (b"Items", ITEMS),
    (b"Key", KEY),
    (b"Keys", KEYS),
    (b"Remove", REMOVE),
    (b"RemoveAll", REMOVE_ALL),
    (b"CompareMode", COMPARE_MODE),
    (b"HashVal", HASH_VALUE),
    (b"_NewEnum", NEW_ENUM),
];

const FIRST_CAPACITY: u32 = 8;
const EMPTY_SLOT: u32 = 0;
const REMOVED_SLOT: u32 = u32::MAX;
const HASH_VALUE_BUCKETS: u32 = 1201;

/// The tables of methods a dictionary and its enumerators use.
#[derive(Debug, Clone, Copy)]
pub struct Tables {
    pub dictionary: usize,
    pub enumerator: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Entry {
    key: Variant,
    item: Variant,
    hash: u32,
    live: bool,
}

/// A dictionary: its table, its reference count, how keys compare, its entries in the order
/// they were added (removed ones stay as gaps until the table is rebuilt) and the slots that
/// find them.
#[repr(C)]
#[derive(Debug)]
pub struct Dictionary {
    #[allow(
        dead_code,
        reason = "the program reads it: an object's first word is its table"
    )]
    table: usize,
    references: AtomicU32,
    compare_mode: i32,
    enumerator_table: usize,
    entries: *mut Entry,
    capacity: u32,
    used: u32,
    live: u32,
    slots: *mut u32,
    slot_mask: u32,
}

/// `DISPPARAMS`: the arguments, last first, the named ones' identifiers and the counts.
#[repr(C)]
#[derive(Debug)]
pub struct DispParams {
    pub arguments: *mut Variant,
    pub named: *mut i32,
    pub count: u32,
    pub named_count: u32,
}

/// `EXCEPINFO`: what `Invoke` reports with `DISP_E_EXCEPTION`.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code, reason = "the program reads it")]
pub struct ExcepInfo {
    pub code: u16,
    pub reserved: u16,
    pub source: *mut u16,
    pub description: *mut u16,
    pub help_file: *mut u16,
    pub help_context: u32,
    pub reserved_pointer: usize,
    pub deferred_fill_in: usize,
    pub status: HResult,
}

const _: () = assert!(core::mem::size_of::<DispParams>() == 24);
const _: () = assert!(core::mem::size_of::<ExcepInfo>() == 64);

#[derive(Debug, Clone, Copy)]
enum Key<'a> {
    Empty,
    Null,
    Number { integer: Option<i128>, value: f64 },
    Date(f64),
    Boolean(bool),
    Text(&'a [u16]),
    Object(usize),
    Error(u32),
}

unsafe fn key_of<'a>(value: &Variant) -> Result<Key<'a>, HResult> {
    Ok(match value.kind {
        kind::EMPTY => Key::Empty,
        kind::NULL => Key::Null,
        // SAFETY: the caller's duty: the value holds a BSTR.
        kind::BSTR => Key::Text(unsafe { bstr::units(value.pointer() as *const u16) }),
        kind::DISPATCH | kind::UNKNOWN => Key::Object(value.pointer()),
        kind::BOOL => Key::Boolean(value.data[0] as u16 != 0),
        kind::DATE => Key::Date(f64::from_bits(value.data[0])),
        kind::ERROR => Key::Error(value.data[0] as u32),
        _ => match conversion::numeric_value(value).ok_or(INVALID_CALL)? {
            conversion::Number::Integer(integer) => Key::Number {
                integer: Some(integer),
                value: integer as f64,
            },
            conversion::Number::Real(value) => Key::Number {
                integer: None,
                value,
            },
            conversion::Number::Currency(currency) => Key::Number {
                integer: (currency % 10_000 == 0).then_some(i128::from(currency / 10_000)),
                value: currency as f64 / 10_000.0,
            },
        },
    })
}

fn fold(unit: u16, ignore_case: bool) -> u16 {
    match (ignore_case, unit) {
        (false, _) => unit,
        (true, ..0x80) => u16::from((unit as u8).to_ascii_uppercase()),
        (true, _) => crate::text::upper(unit),
    }
}

fn mix(word: u64) -> u32 {
    let mut mixed = word ^ (word >> 33);
    mixed = mixed.wrapping_mul(0xff51_afd7_ed55_8ccd);
    mixed ^= mixed >> 33;
    mixed = mixed.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    (mixed ^ (mixed >> 33)) as u32
}

fn number_bits(value: f64) -> u64 {
    if value == 0.0 { 0 } else { value.to_bits() }
}

fn hash(key: &Key<'_>, ignore_case: bool) -> u32 {
    let word = match *key {
        Key::Empty => 0x9e37_79b9,
        Key::Null => 0x7f4a_7c15,
        Key::Number { value, .. } => number_bits(value),
        Key::Date(value) => number_bits(value) ^ 0x5555_5555,
        Key::Boolean(value) => 0x3c6e_f372 + u64::from(value),
        Key::Text(units) => units.iter().fold(0xcbf2_9ce4_8422_2325, |state, &unit| {
            (state ^ u64::from(fold(unit, ignore_case))).wrapping_mul(0x0100_0000_01b3)
        }),
        Key::Object(pointer) => pointer as u64,
        Key::Error(code) => u64::from(code) ^ 0xa54f_f53a_0000_0000,
    };
    mix(word)
}

fn same(a: &Key<'_>, b: &Key<'_>, ignore_case: bool) -> bool {
    match (*a, *b) {
        (Key::Empty, Key::Empty) | (Key::Null, Key::Null) => true,
        (
            Key::Number {
                integer: Some(a), ..
            },
            Key::Number {
                integer: Some(b), ..
            },
        ) => a == b,
        (Key::Number { value: a, .. }, Key::Number { value: b, .. }) => a == b,
        (Key::Date(a), Key::Date(b)) => a == b,
        (Key::Boolean(a), Key::Boolean(b)) => a == b,
        (Key::Object(a), Key::Object(b)) => a == b,
        (Key::Error(a), Key::Error(b)) => a == b,
        (Key::Text(a), Key::Text(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(&x, &y)| fold(x, ignore_case) == fold(y, ignore_case))
        }
        _ => false,
    }
}

fn description(code: HResult) -> &'static [u8] {
    match code {
        KEY_EXISTS => b"This key is already associated with an element of this collection",
        ELEMENT_NOT_FOUND => b"Element not found",
        _ => b"Invalid procedure call or argument",
    }
}

/// A new, empty dictionary with one reference; null without memory.
pub fn create(tables: Tables) -> *mut Dictionary {
    let object = super::allocate(core::mem::size_of::<Dictionary>()).cast::<Dictionary>();
    if object.is_null() {
        return object;
    }
    let dictionary = Dictionary {
        table: tables.dictionary,
        references: AtomicU32::new(1),
        compare_mode: 0,
        enumerator_table: tables.enumerator,
        entries: ptr::null_mut(),
        capacity: 0,
        used: 0,
        live: 0,
        slots: ptr::null_mut(),
        slot_mask: 0,
    };
    // SAFETY: a fresh, aligned block for one dictionary.
    unsafe { object.write(dictionary) };
    object
}

impl Dictionary {
    fn ignores_case(&self) -> bool {
        self.compare_mode >= 1
    }

    fn entries(&self) -> &[Entry] {
        match self.entries.is_null() {
            true => &[],
            // SAFETY: the entries block holds `used` written entries.
            false => unsafe { core::slice::from_raw_parts(self.entries, self.used as usize) },
        }
    }

    fn entries_mut(&mut self) -> &mut [Entry] {
        match self.entries.is_null() {
            true => &mut [],
            // SAFETY: as in `entries`; the dictionary is borrowed mutably.
            false => unsafe { core::slice::from_raw_parts_mut(self.entries, self.used as usize) },
        }
    }

    fn slots_mut(&mut self) -> &mut [u32] {
        match self.slots.is_null() {
            true => &mut [],
            // SAFETY: the slots block holds `slot_mask + 1` slots.
            false => unsafe {
                core::slice::from_raw_parts_mut(self.slots, self.slot_mask as usize + 1)
            },
        }
    }

    fn find(&self, key: &Key<'_>, hash: u32) -> Option<u32> {
        if self.slots.is_null() {
            return None;
        }
        let entries = self.entries();
        let mut at = hash & self.slot_mask;
        loop {
            // SAFETY: `at` is masked into the slots block.
            let slot = unsafe { self.slots.wrapping_add(at as usize).read() };
            if slot == EMPTY_SLOT {
                return None;
            }
            if slot != REMOVED_SLOT {
                let entry = &entries[(slot - 1) as usize];
                // SAFETY: stored keys are valid, their strings owned by the entry.
                let stored = unsafe { key_of(&entry.key) };
                if entry.hash == hash
                    && let Ok(stored) = stored
                    && same(&stored, key, self.ignores_case())
                {
                    return Some(slot - 1);
                }
            }
            at = (at + 1) & self.slot_mask;
        }
    }

    fn place(&mut self, number: u32, hash: u32) {
        let mask = self.slot_mask;
        let slots = self.slots_mut();
        let mut at = hash & mask;
        while slots[at as usize] != EMPTY_SLOT && slots[at as usize] != REMOVED_SLOT {
            at = (at + 1) & mask;
        }
        slots[at as usize] = number + 1;
    }

    fn unplace(&mut self, number: u32, hash: u32) {
        let mask = self.slot_mask;
        let slots = self.slots_mut();
        let mut at = hash & mask;
        while slots[at as usize] != EMPTY_SLOT {
            if slots[at as usize] == number + 1 {
                slots[at as usize] = REMOVED_SLOT;
                return;
            }
            at = (at + 1) & mask;
        }
    }

    fn rebuild_slots(&mut self) {
        self.slots_mut().fill(EMPTY_SLOT);
        for number in 0..self.used {
            let entry = self.entries()[number as usize];
            if entry.live {
                self.place(number, entry.hash);
            }
        }
    }

    fn compact(&mut self) {
        let mut kept = 0;
        for number in 0..self.used as usize {
            let entry = self.entries()[number];
            if entry.live {
                self.entries_mut()[kept] = entry;
                kept += 1;
            }
        }
        self.used = kept as u32;
    }

    fn make_room(&mut self) -> Result<(), HResult> {
        if self.used - self.live >= self.used / 2 && self.used > 0 {
            self.compact();
            self.rebuild_slots();
            return Ok(());
        }
        let capacity = match self.capacity {
            0 => FIRST_CAPACITY,
            capacity if capacity < u32::MAX / 4 => capacity * 2,
            _ => return Err(E_OUTOFMEMORY),
        };
        let entries = super::allocate(capacity as usize * core::mem::size_of::<Entry>());
        let slot_count = capacity as usize * 2;
        let slots = super::allocate(slot_count * core::mem::size_of::<u32>());
        if entries.is_null() || slots.is_null() {
            // SAFETY: each is null or a fresh block nothing uses.
            unsafe { super::free(entries) };
            // SAFETY: as above.
            unsafe { super::free(slots) };
            return Err(E_OUTOFMEMORY);
        }
        self.compact();
        let old_entries = core::mem::replace(&mut self.entries, entries.cast());
        if !old_entries.is_null() {
            // SAFETY: both blocks hold at least `used` entries and are different blocks.
            unsafe { ptr::copy_nonoverlapping(old_entries, self.entries, self.used as usize) };
        }
        let old_slots = core::mem::replace(&mut self.slots, slots.cast());
        // SAFETY: the old blocks are the dictionary's, replaced now.
        unsafe { super::free(old_entries.cast()) };
        // SAFETY: as above.
        unsafe { super::free(old_slots.cast()) };
        self.capacity = capacity;
        self.slot_mask = (slot_count - 1) as u32;
        self.rebuild_slots();
        Ok(())
    }

    fn insert(&mut self, key: Variant, item: Variant, hash: u32) -> Result<u32, HResult> {
        if self.used == self.capacity
            && let Err(code) = self.make_room()
        {
            // SAFETY: the key and item were handed over and are given back.
            unsafe { variant::release(&key) };
            // SAFETY: as above.
            unsafe { variant::release(&item) };
            return Err(code);
        }
        let number = self.used;
        let entry = Entry {
            key,
            item,
            hash,
            live: true,
        };
        // SAFETY: the entries block has room for `capacity` entries, and `used` < `capacity`.
        unsafe { self.entries.wrapping_add(number as usize).write(entry) };
        self.used += 1;
        self.live += 1;
        self.place(number, hash);
        Ok(number)
    }

    fn release_entry(entry: &mut Entry) {
        // SAFETY: the entry owns its key and item, which go now.
        unsafe { variant::release(&entry.key) };
        // SAFETY: as above.
        unsafe { variant::release(&entry.item) };
        entry.key = Variant::EMPTY;
        entry.item = Variant::EMPTY;
        entry.live = false;
    }

    fn remove_entry(&mut self, number: u32) {
        let hash = self.entries()[number as usize].hash;
        self.unplace(number, hash);
        Self::release_entry(&mut self.entries_mut()[number as usize]);
        self.live -= 1;
    }

    fn clear(&mut self) {
        for entry in self.entries_mut() {
            if entry.live {
                Self::release_entry(entry);
            }
        }
        self.used = 0;
        self.live = 0;
        self.slots_mut().fill(EMPTY_SLOT);
    }

    unsafe fn lookup<'a>(&self, key: &'a Variant) -> Result<(Variant, Key<'a>, u32), HResult> {
        // SAFETY: the caller's duty.
        let value = unsafe { variant::view(key) }?;
        if value.kind & kind::ARRAY != 0 {
            return Err(INVALID_CALL);
        }
        // SAFETY: as above; the key's string is the caller's and outlives the call.
        let found = unsafe { key_of(&value) }?;
        Ok((value, found, hash(&found, self.ignores_case())))
    }

    unsafe fn store(&mut self, key: &Variant, item: &Variant, replace: bool) -> HResult {
        // SAFETY: the caller's duty.
        let (key_value, found, hash) = match unsafe { self.lookup(key) } {
            Ok(found) => found,
            Err(code) => return code,
        };
        // SAFETY: the caller's duty.
        let viewed = match unsafe { variant::view(item) } {
            Ok(viewed) => viewed,
            Err(code) => return code,
        };
        // SAFETY: `view` gave a valid value.
        let item = match unsafe { variant::duplicate(&viewed) } {
            Ok(item) => item,
            Err(code) => return code,
        };
        if let Some(number) = self.find(&found, hash) {
            if !replace {
                // SAFETY: the copy is this call's.
                unsafe { variant::release(&item) };
                return KEY_EXISTS;
            }
            let entry = &mut self.entries_mut()[number as usize];
            let old = core::mem::replace(&mut entry.item, item);
            // SAFETY: the old item was the entry's.
            unsafe { variant::release(&old) };
            return S_OK;
        }
        // SAFETY: `lookup` gave a valid value.
        let key_copy = match unsafe { variant::duplicate(&key_value) } {
            Ok(copy) => copy,
            Err(code) => {
                // SAFETY: the copy is this call's.
                unsafe { variant::release(&item) };
                return code;
            }
        };
        match self.insert(key_copy, item, hash) {
            Ok(_) => S_OK,
            Err(code) => code,
        }
    }

    unsafe fn item(&mut self, key: &Variant) -> Result<Variant, HResult> {
        // SAFETY: the caller's duty.
        let (key_value, found, hash) = unsafe { self.lookup(key) }?;
        let number = match self.find(&found, hash) {
            Some(number) => number,
            None => {
                // SAFETY: `lookup` gave a valid value.
                let key_copy = unsafe { variant::duplicate(&key_value) }?;
                self.insert(key_copy, Variant::EMPTY, hash)?
            }
        };
        // SAFETY: the entry's item is valid.
        unsafe { variant::duplicate(&self.entries()[number as usize].item) }
    }

    unsafe fn exists(&self, key: &Variant) -> Result<bool, HResult> {
        // SAFETY: the caller's duty.
        let (_, found, hash) = unsafe { self.lookup(key) }?;
        Ok(self.find(&found, hash).is_some())
    }

    unsafe fn remove(&mut self, key: &Variant) -> HResult {
        // SAFETY: the caller's duty.
        let (_, found, hash) = match unsafe { self.lookup(key) } {
            Ok(found) => found,
            Err(code) => return code,
        };
        match self.find(&found, hash) {
            Some(number) => {
                self.remove_entry(number);
                S_OK
            }
            None => ELEMENT_NOT_FOUND,
        }
    }

    unsafe fn rename(&mut self, key: &Variant, new_key: &Variant) -> HResult {
        // SAFETY: the caller's duty.
        let (_, found, hash) = match unsafe { self.lookup(key) } {
            Ok(found) => found,
            Err(code) => return code,
        };
        let Some(number) = self.find(&found, hash) else {
            return ELEMENT_NOT_FOUND;
        };
        // SAFETY: the caller's duty.
        let (new_value, new_found, new_hash) = match unsafe { self.lookup(new_key) } {
            Ok(found) => found,
            Err(code) => return code,
        };
        if self.find(&new_found, new_hash).is_some() {
            return KEY_EXISTS;
        }
        // SAFETY: `lookup` gave a valid value.
        let copy = match unsafe { variant::duplicate(&new_value) } {
            Ok(copy) => copy,
            Err(code) => return code,
        };
        self.unplace(number, hash);
        let entry = &mut self.entries_mut()[number as usize];
        let old = core::mem::replace(&mut entry.key, copy);
        entry.hash = new_hash;
        // SAFETY: the old key was the entry's.
        unsafe { variant::release(&old) };
        self.place(number, new_hash);
        S_OK
    }

    fn array(&self, keys: bool) -> Result<*mut SafeArray, HResult> {
        let bound = [Bound {
            elements: self.live,
            lower: 0,
        }];
        let array = safe_array::create(kind::VARIANT, &bound);
        if array.is_null() {
            return Err(E_OUTOFMEMORY);
        }
        // SAFETY: a new array of `live` VARIANTs, this function's.
        let elements = unsafe { safe_array::variants(array) };
        let live = self.entries().iter().filter(|entry| entry.live);
        for (element, entry) in elements.iter_mut().zip(live) {
            let source = if keys { &entry.key } else { &entry.item };
            // SAFETY: stored values are valid.
            match unsafe { variant::duplicate(source) } {
                Ok(copy) => *element = copy,
                Err(code) => {
                    // SAFETY: the array is this function's.
                    unsafe { safe_array::destroy(array) };
                    return Err(code);
                }
            }
        }
        Ok(array)
    }

    fn array_value(&self, keys: bool) -> Result<Variant, HResult> {
        let array = self.array(keys)?;
        Ok(Variant::new(kind::ARRAY | kind::VARIANT, array as u64))
    }

    fn set_compare_mode(&mut self, mode: i32) -> HResult {
        if mode < 0 || self.live > 0 {
            return INVALID_CALL;
        }
        self.compare_mode = mode;
        S_OK
    }

    unsafe fn hash_value(&self, key: &Variant) -> Result<i32, HResult> {
        // SAFETY: the caller's duty.
        let (_, _, hash) = unsafe { self.lookup(key) }?;
        Ok((hash % HASH_VALUE_BUCKETS) as i32)
    }

    fn new_enumerator(&self) -> Result<usize, HResult> {
        let keys = self.array(true)?;
        // SAFETY: the array is handed to the new enumerator.
        let object = unsafe { enumerator::create(self.enumerator_table, keys, 0) };
        if object.is_null() {
            return Err(E_OUTOFMEMORY);
        }
        Ok(object as usize)
    }

    unsafe fn destroy(this: *mut Dictionary) {
        // SAFETY: the caller's duty: the dictionary is this function's.
        let dictionary = unsafe { &mut *this };
        dictionary.clear();
        // SAFETY: the blocks are the dictionary's, which goes now.
        unsafe { super::free(dictionary.entries.cast()) };
        // SAFETY: as above.
        unsafe { super::free(dictionary.slots.cast()) };
        // SAFETY: as above.
        unsafe { super::free(this.cast()) };
    }
}

fn is_raised(code: HResult) -> bool {
    (code as u32) >> 16 == FACILITY_CONTROL
}

unsafe fn raise(exception: *mut ExcepInfo, code: HResult) -> HResult {
    if !exception.is_null() {
        let report = ExcepInfo {
            code: 0,
            reserved: 0,
            source: ptr::null_mut(),
            description: bstr::from_ascii(description(code)),
            help_file: ptr::null_mut(),
            help_context: 0,
            reserved_pointer: 0,
            deferred_fill_in: 0,
            status: code,
        };
        // SAFETY: the caller's duty.
        unsafe { exception.write_unaligned(report) };
    }
    DISP_E_EXCEPTION
}

struct Call<'a> {
    flags: u16,
    positional: &'a [Variant],
    value: Option<&'a Variant>,
}

impl<'a> Call<'a> {
    unsafe fn read(parameters: &'a DispParams, flags: u16) -> Result<Self, HResult> {
        let (count, named_count) = (parameters.count as usize, parameters.named_count as usize);
        if named_count > count || (count > 0 && parameters.arguments.is_null()) {
            return Err(E_INVALIDARG);
        }
        let arguments = match count {
            0 => &[][..],
            // SAFETY: the caller's duty: `count` arguments.
            _ => unsafe { core::slice::from_raw_parts(parameters.arguments, count) },
        };
        let named = match named_count {
            0 => &[][..],
            _ if parameters.named.is_null() => return Err(E_INVALIDARG),
            // SAFETY: the caller's duty: `named_count` identifiers.
            _ => unsafe { core::slice::from_raw_parts(parameters.named, named_count) },
        };
        let puts = flags & (DISPATCH_PROPERTYPUT | DISPATCH_PROPERTYPUTREF) != 0;
        match (puts, named) {
            (false, []) => Ok(Self {
                flags,
                positional: arguments,
                value: None,
            }),
            (false, _) => Err(DISP_E_NONAMEDARGS),
            (true, [] | [DISPID_PROPERTYPUT]) if count > 0 => Ok(Self {
                flags,
                positional: &arguments[1..],
                value: Some(&arguments[0]),
            }),
            (true, [] | [DISPID_PROPERTYPUT]) => Err(DISP_E_BADPARAMCOUNT),
            (true, _) => Err(DISP_E_PARAMNOTFOUND),
        }
    }

    fn gets(&self) -> bool {
        self.flags & (DISPATCH_METHOD | DISPATCH_PROPERTYGET) != 0
    }

    fn puts(&self) -> bool {
        self.value.is_some()
    }

    fn arguments<const N: usize>(&self) -> Result<[&'a Variant; N], HResult> {
        if self.positional.len() != N {
            return Err(DISP_E_BADPARAMCOUNT);
        }
        let positional = self.positional;
        Ok(core::array::from_fn(|index| {
            &positional[positional.len() - 1 - index]
        }))
    }
}

fn status(code: HResult) -> Result<Option<Variant>, HResult> {
    match code {
        0.. => Ok(None),
        failure => Err(failure),
    }
}

impl Dictionary {
    unsafe fn dispatch(
        &mut self,
        member: i32,
        call: &Call<'_>,
    ) -> Result<Option<Variant>, HResult> {
        let gets = call.gets() && !call.puts();
        match member {
            ITEM if call.puts() => {
                let [key] = call.arguments()?;
                let value = call.value.ok_or(DISP_E_BADPARAMCOUNT)?;
                // SAFETY: the caller's duty: the arguments are valid VARIANTs.
                status(unsafe { self.store(key, value, true) })
            }
            ITEM if gets => {
                let [key] = call.arguments()?;
                // SAFETY: as above.
                unsafe { self.item(key) }.map(Some)
            }
            ADD if gets => {
                let [key, item] = call.arguments()?;
                // SAFETY: as above.
                status(unsafe { self.store(key, item, false) })
            }
            COUNT if gets => {
                call.arguments::<0>()?;
                Ok(Some(Variant::long(self.live as i32)))
            }
            EXISTS if gets => {
                let [key] = call.arguments()?;
                // SAFETY: as above.
                unsafe { self.exists(key) }.map(|found| Some(Variant::boolean(found)))
            }
            ITEMS | KEYS if gets => {
                call.arguments::<0>()?;
                self.array_value(member == KEYS).map(Some)
            }
            KEY if call.puts() => {
                let [key] = call.arguments()?;
                let new_key = call.value.ok_or(DISP_E_BADPARAMCOUNT)?;
                // SAFETY: as above.
                status(unsafe { self.rename(key, new_key) })
            }
            REMOVE if gets => {
                let [key] = call.arguments()?;
                // SAFETY: as above.
                status(unsafe { self.remove(key) })
            }
            REMOVE_ALL if gets => {
                call.arguments::<0>()?;
                self.clear();
                Ok(None)
            }
            COMPARE_MODE if call.puts() => {
                call.arguments::<0>()?;
                let value = call.value.ok_or(DISP_E_BADPARAMCOUNT)?;
                // SAFETY: as above.
                let viewed = unsafe { variant::view(value) }?;
                // SAFETY: `view` gave a valid value.
                let mode = unsafe { conversion::change(&viewed, 0, kind::I4) }?;
                status(self.set_compare_mode(mode.data[0] as u32 as i32))
            }
            COMPARE_MODE if gets => {
                call.arguments::<0>()?;
                Ok(Some(Variant::long(self.compare_mode)))
            }
            HASH_VALUE if gets => {
                let [key] = call.arguments()?;
                // SAFETY: as above.
                unsafe { self.hash_value(key) }.map(|hash| Some(Variant::long(hash)))
            }
            NEW_ENUM if gets => {
                call.arguments::<0>()?;
                let object = self.new_enumerator()?;
                Ok(Some(Variant::new(kind::UNKNOWN, object as u64)))
            }
            _ => Err(DISP_E_MEMBERNOTFOUND),
        }
    }
}

unsafe fn dictionary<'a>(this: *mut Dictionary) -> &'a mut Dictionary {
    // SAFETY: the caller's duty.
    unsafe { &mut *this }
}

unsafe fn argument<'a>(value: *const Variant) -> Result<&'a Variant, HResult> {
    // SAFETY: the caller's duty.
    unsafe { value.as_ref() }.ok_or(E_POINTER)
}

unsafe fn answer<T>(result: *mut T, value: T, release: fn(&T)) -> HResult {
    if result.is_null() {
        release(&value);
        return E_POINTER;
    }
    // SAFETY: the caller's duty.
    unsafe { result.write_unaligned(value) };
    S_OK
}

fn release_variant(value: &Variant) {
    // SAFETY: the value is the caller's, which gives it up.
    unsafe { variant::release(value) };
}

fn keep<T>(_: &T) {}

/// `IUnknown::QueryInterface`: `IUnknown`, `IDispatch` and `IDictionary`.
///
/// # Safety
/// `this` must be a live dictionary; the others as `QueryInterface` says.
pub unsafe extern "efiapi" fn query_interface(
    this: *mut Dictionary,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let references = unsafe { &(*this).references };
    let interfaces = [guid::UNKNOWN, guid::DISPATCH, INTERFACE];
    // SAFETY: the caller's duty.
    unsafe { super::answer_query(this as usize, references, interface, result, &interfaces) }
}

/// `IUnknown::AddRef`.
///
/// # Safety
/// `this` must be a live dictionary.
pub unsafe extern "efiapi" fn add_reference(this: *mut Dictionary) -> u32 {
    // SAFETY: the caller's duty.
    let references = unsafe { &(*this).references };
    references.fetch_add(1, Ordering::Relaxed) + 1
}

/// `IUnknown::Release`: the last reference gives back the entries and the dictionary.
///
/// # Safety
/// `this` must be a live dictionary the caller holds a reference to.
pub unsafe extern "efiapi" fn release(this: *mut Dictionary) -> u32 {
    // SAFETY: the caller's duty.
    let (left, last) = super::drop_reference(unsafe { &(*this).references });
    if last {
        // SAFETY: no reference is left.
        unsafe { Dictionary::destroy(this) };
    }
    left
}

/// `IDispatch::GetTypeInfoCount`: 0, the dictionary gives no type information.
///
/// # Safety
/// `count` must be null or writable.
pub unsafe extern "efiapi" fn get_type_info_count(
    _this: *mut Dictionary,
    count: *mut u32,
) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { answer(count, 0, keep) }
}

/// `IDispatch::GetTypeInfo`: there is none, so every index is out of range.
///
/// # Safety
/// `info` must be null or writable.
pub unsafe extern "efiapi" fn get_type_info(
    _this: *mut Dictionary,
    _index: u32,
    _locale: u32,
    info: *mut usize,
) -> HResult {
    if !info.is_null() {
        // SAFETY: the caller's duty.
        unsafe { info.write_unaligned(0) };
    }
    safe_array::DISP_E_BADINDEX
}

fn member_named(name: &[u16]) -> Option<i32> {
    MEMBERS.iter().find_map(|&(member, id)| {
        let matches = member.len() == name.len()
            && member
                .iter()
                .zip(name)
                .all(|(&byte, &unit)| unit < 0x80 && (unit as u8).eq_ignore_ascii_case(&byte));
        matches.then_some(id)
    })
}

/// `IDispatch::GetIDsOfNames`: the member's identifier, names compared ignoring case; the
/// dictionary's members have no named parameters.
///
/// # Safety
/// `names` must hold `count` NUL-terminated names and `ids` room for `count` identifiers.
pub unsafe extern "efiapi" fn get_ids_of_names(
    _this: *mut Dictionary,
    interface: *const Guid,
    names: *const *const u16,
    count: u32,
    _locale: u32,
    ids: *mut i32,
) -> HResult {
    // SAFETY: the caller's duty.
    if unsafe { Guid::read(interface) } != Some(guid::NULL) {
        return DISP_E_UNKNOWNINTERFACE;
    }
    if names.is_null() || ids.is_null() || count == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty: `count` names.
    let names = unsafe { core::slice::from_raw_parts(names, count as usize) };
    // SAFETY: as above: each a NUL-terminated name.
    let found = member_named(unsafe { super::wide_text(names[0]) });
    for index in 0..count as usize {
        let id = match index {
            0 => found.unwrap_or(DISPID_UNKNOWN),
            _ => DISPID_UNKNOWN,
        };
        // SAFETY: the caller's duty: room for `count` identifiers.
        unsafe { ids.wrapping_add(index).write_unaligned(id) };
    }
    match found.is_some() && count == 1 {
        true => S_OK,
        false => DISP_E_UNKNOWNNAME,
    }
}

/// `IDispatch::Invoke`.
///
/// # Safety
/// `this` must be a live dictionary; the others as `Invoke` says.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "efiapi" fn invoke(
    this: *mut Dictionary,
    member: i32,
    interface: *const Guid,
    _locale: u32,
    flags: u16,
    parameters: *const DispParams,
    result: *mut Variant,
    exception: *mut ExcepInfo,
    _argument_error: *mut u32,
) -> HResult {
    // SAFETY: the caller's duty.
    if unsafe { Guid::read(interface) } != Some(guid::NULL) {
        return DISP_E_UNKNOWNINTERFACE;
    }
    // SAFETY: the caller's duty.
    let Some(parameters) = (unsafe { parameters.as_ref() }) else {
        return E_INVALIDARG;
    };
    // SAFETY: the caller's duty.
    let call = match unsafe { Call::read(parameters, flags) } {
        Ok(call) => call,
        Err(code) => return code,
    };
    // SAFETY: the caller's duty: a live dictionary.
    let object = unsafe { dictionary(this) };
    // SAFETY: the caller's duty.
    let outcome = unsafe { object.dispatch(member, &call) };
    match outcome {
        Ok(Some(value)) if result.is_null() => {
            release_variant(&value);
            S_OK
        }
        Ok(Some(value)) => {
            // SAFETY: the caller's duty: `result` is a VARIANT for the result.
            unsafe { result.write_unaligned(value) };
            S_OK
        }
        Ok(None) => S_OK,
        // SAFETY: the caller's duty.
        Err(code) if is_raised(code) => unsafe { raise(exception, code) },
        Err(code) => code,
    }
}

/// `IDictionary::putref_Item`.
///
/// # Safety
/// `this` must be a live dictionary; `key` and `item` VARIANTs.
pub unsafe extern "efiapi" fn put_item_reference(
    this: *mut Dictionary,
    key: *const Variant,
    item: *const Variant,
) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { put_item(this, key, item) }
}

/// `IDictionary::put_Item`: sets the key's item, adding the key when it is missing.
///
/// # Safety
/// `this` must be a live dictionary; `key` and `item` VARIANTs.
pub unsafe extern "efiapi" fn put_item(
    this: *mut Dictionary,
    key: *const Variant,
    item: *const Variant,
) -> HResult {
    // SAFETY: the caller's duty.
    let key = unsafe { argument(key) };
    // SAFETY: as above.
    let item = unsafe { argument(item) };
    let (Ok(key), Ok(item)) = (key, item) else {
        return E_POINTER;
    };
    // SAFETY: the caller's duty: a live dictionary.
    let object = unsafe { dictionary(this) };
    // SAFETY: the caller's duty.
    unsafe { object.store(key, item, true) }
}

/// `IDictionary::get_Item`: a copy of the key's item; a missing key is added with an empty item.
///
/// # Safety
/// `this` must be a live dictionary; `key` a VARIANT; `item` null or a writable VARIANT.
pub unsafe extern "efiapi" fn get_item(
    this: *mut Dictionary,
    key: *const Variant,
    item: *mut Variant,
) -> HResult {
    // SAFETY: the caller's duty.
    let Ok(key) = (unsafe { argument(key) }) else {
        return E_POINTER;
    };
    // SAFETY: the caller's duty: a live dictionary.
    let object = unsafe { dictionary(this) };
    // SAFETY: the caller's duty.
    match unsafe { object.item(key) } {
        // SAFETY: the caller's duty; the copy is this call's.
        Ok(value) => unsafe { answer(item, value, release_variant) },
        Err(code) => code,
    }
}

/// `IDictionary::Add`: a new key with its item; `KEY_EXISTS` (error 457) when it is there.
///
/// # Safety
/// `this` must be a live dictionary; `key` and `item` VARIANTs.
pub unsafe extern "efiapi" fn add(
    this: *mut Dictionary,
    key: *const Variant,
    item: *const Variant,
) -> HResult {
    // SAFETY: the caller's duty.
    let key = unsafe { argument(key) };
    // SAFETY: as above.
    let item = unsafe { argument(item) };
    let (Ok(key), Ok(item)) = (key, item) else {
        return E_POINTER;
    };
    // SAFETY: the caller's duty: a live dictionary.
    let object = unsafe { dictionary(this) };
    // SAFETY: the caller's duty.
    unsafe { object.store(key, item, false) }
}

/// `IDictionary::get_Count`.
///
/// # Safety
/// `this` must be a live dictionary; `count` null or writable.
pub unsafe extern "efiapi" fn get_count(this: *mut Dictionary, count: *mut i32) -> HResult {
    // SAFETY: the caller's duty.
    let live = unsafe { dictionary(this) }.live as i32;
    // SAFETY: the caller's duty.
    unsafe { answer(count, live, keep) }
}

/// `IDictionary::Exists`: `VARIANT_TRUE` or `VARIANT_FALSE`.
///
/// # Safety
/// `this` must be a live dictionary; `key` a VARIANT; `exists` null or writable.
pub unsafe extern "efiapi" fn exists(
    this: *mut Dictionary,
    key: *const Variant,
    exists: *mut i16,
) -> HResult {
    // SAFETY: the caller's duty.
    let Ok(key) = (unsafe { argument(key) }) else {
        return E_POINTER;
    };
    // SAFETY: the caller's duty: a live dictionary.
    let object = unsafe { dictionary(this) };
    // SAFETY: the caller's duty.
    match unsafe { object.exists(key) } {
        // SAFETY: the caller's duty.
        Ok(found) => unsafe { answer(exists, -i16::from(found), keep) },
        Err(code) => code,
    }
}

/// `IDictionary::Items`: a SAFEARRAY of copies of the items, in order.
///
/// # Safety
/// `this` must be a live dictionary; `items` null or a writable VARIANT.
pub unsafe extern "efiapi" fn items(this: *mut Dictionary, items: *mut Variant) -> HResult {
    // SAFETY: the caller's duty.
    match unsafe { dictionary(this) }.array_value(false) {
        // SAFETY: the caller's duty; the array is this call's.
        Ok(value) => unsafe { answer(items, value, release_variant) },
        Err(code) => code,
    }
}

/// `IDictionary::put_Key`: renames a key, keeping its item and place.
///
/// # Safety
/// `this` must be a live dictionary; `key` and `new_key` VARIANTs.
pub unsafe extern "efiapi" fn put_key(
    this: *mut Dictionary,
    key: *const Variant,
    new_key: *const Variant,
) -> HResult {
    // SAFETY: the caller's duty.
    let key = unsafe { argument(key) };
    // SAFETY: as above.
    let new_key = unsafe { argument(new_key) };
    let (Ok(key), Ok(new_key)) = (key, new_key) else {
        return E_POINTER;
    };
    // SAFETY: the caller's duty: a live dictionary.
    let object = unsafe { dictionary(this) };
    // SAFETY: the caller's duty.
    unsafe { object.rename(key, new_key) }
}

/// `IDictionary::Keys`: a SAFEARRAY of copies of the keys, in order.
///
/// # Safety
/// `this` must be a live dictionary; `keys` null or a writable VARIANT.
pub unsafe extern "efiapi" fn keys(this: *mut Dictionary, keys: *mut Variant) -> HResult {
    // SAFETY: the caller's duty.
    match unsafe { dictionary(this) }.array_value(true) {
        // SAFETY: the caller's duty; the array is this call's.
        Ok(value) => unsafe { answer(keys, value, release_variant) },
        Err(code) => code,
    }
}

/// `IDictionary::Remove`: `ELEMENT_NOT_FOUND` (error 32811) for a missing key.
///
/// # Safety
/// `this` must be a live dictionary; `key` a VARIANT.
pub unsafe extern "efiapi" fn remove(this: *mut Dictionary, key: *const Variant) -> HResult {
    // SAFETY: the caller's duty.
    let Ok(key) = (unsafe { argument(key) }) else {
        return E_POINTER;
    };
    // SAFETY: the caller's duty: a live dictionary.
    let object = unsafe { dictionary(this) };
    // SAFETY: the caller's duty.
    unsafe { object.remove(key) }
}

/// `IDictionary::RemoveAll`.
///
/// # Safety
/// `this` must be a live dictionary.
pub unsafe extern "efiapi" fn remove_all(this: *mut Dictionary) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { dictionary(this) }.clear();
    S_OK
}

/// `IDictionary::put_CompareMode`: only while the dictionary is empty.
///
/// # Safety
/// `this` must be a live dictionary.
pub unsafe extern "efiapi" fn put_compare_mode(this: *mut Dictionary, mode: i32) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { dictionary(this) }.set_compare_mode(mode)
}

/// `IDictionary::get_CompareMode`.
///
/// # Safety
/// `this` must be a live dictionary; `mode` null or writable.
pub unsafe extern "efiapi" fn get_compare_mode(this: *mut Dictionary, mode: *mut i32) -> HResult {
    // SAFETY: the caller's duty.
    let current = unsafe { dictionary(this) }.compare_mode;
    // SAFETY: the caller's duty.
    unsafe { answer(mode, current, keep) }
}

/// `IDictionary::_NewEnum`: an `IEnumVARIANT` over the keys.
///
/// # Safety
/// `this` must be a live dictionary; `result` null or writable.
pub unsafe extern "efiapi" fn new_enum(this: *mut Dictionary, result: *mut usize) -> HResult {
    // SAFETY: the caller's duty.
    match unsafe { dictionary(this) }.new_enumerator() {
        Ok(object) => {
            let release_object = |object: &usize| {
                // SAFETY: the new enumerator's only reference, unused.
                unsafe { super::release(*object) }
            };
            // SAFETY: the caller's duty.
            unsafe { answer(result, object, release_object) }
        }
        Err(code) => code,
    }
}

/// `IDictionary::get_HashVal`: the key's hash, a `VT_I4` from 0 to 1200 (the layer's own hash,
/// not Windows').
///
/// # Safety
/// `this` must be a live dictionary; `key` a VARIANT; `hash` null or a writable VARIANT.
pub unsafe extern "efiapi" fn get_hash_value(
    this: *mut Dictionary,
    key: *const Variant,
    hash: *mut Variant,
) -> HResult {
    // SAFETY: the caller's duty.
    let Ok(key) = (unsafe { argument(key) }) else {
        return E_POINTER;
    };
    // SAFETY: the caller's duty: a live dictionary.
    let object = unsafe { dictionary(this) };
    // SAFETY: the caller's duty.
    match unsafe { object.hash_value(key) } {
        // SAFETY: the caller's duty.
        Ok(value) => unsafe { answer(hash, Variant::long(value), keep) },
        Err(code) => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::enumerator;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    fn text(value: &str) -> Variant {
        Variant::new(
            kind::BSTR,
            bstr::from_units(&wide(value)[..value.len()]) as u64,
        )
    }

    fn free(mut value: Variant) {
        // SAFETY: the value is the test's.
        unsafe { variant::clear(&mut value) };
    }

    fn text_of(value: &Variant) -> String {
        assert_eq!(value.kind, kind::BSTR);
        // SAFETY: a BSTR.
        String::from_utf16(unsafe { bstr::units(value.pointer() as *const u16) }).unwrap()
    }

    fn new_dictionary() -> *mut Dictionary {
        create(Tables {
            dictionary: 0,
            enumerator: 0,
        })
    }

    fn id_of(dictionary: *mut Dictionary, name: &str) -> (HResult, i32) {
        let name = wide(name);
        let names = [name.as_ptr()];
        let mut id = 0;
        // SAFETY: one name and room for one identifier.
        let code =
            unsafe { get_ids_of_names(dictionary, &guid::NULL, names.as_ptr(), 1, 0, &mut id) };
        (code, id)
    }

    struct Outcome {
        code: HResult,
        result: Variant,
        exception: ExcepInfo,
    }

    fn call(dictionary: *mut Dictionary, name: &str, flags: u16, arguments: &[Variant]) -> Outcome {
        let (_, member) = id_of(dictionary, name);
        let mut reversed: Vec<Variant> = arguments.iter().rev().copied().collect();
        let mut named = [DISPID_PROPERTYPUT];
        let puts = flags & DISPATCH_PROPERTYPUT != 0;
        let parameters = DispParams {
            arguments: reversed.as_mut_ptr(),
            named: named.as_mut_ptr(),
            count: reversed.len() as u32,
            named_count: u32::from(puts),
        };
        let mut result = Variant::EMPTY;
        // SAFETY: an all-zero EXCEPINFO is valid.
        let mut exception: ExcepInfo = unsafe { core::mem::zeroed() };
        // SAFETY: a live dictionary and valid arguments.
        let code = unsafe {
            invoke(
                dictionary,
                member,
                &guid::NULL,
                0,
                flags,
                &parameters,
                &mut result,
                &mut exception,
                ptr::null_mut(),
            )
        };
        Outcome {
            code,
            result,
            exception,
        }
    }

    fn method(dictionary: *mut Dictionary, name: &str, arguments: &[Variant]) -> Outcome {
        call(dictionary, name, DISPATCH_METHOD, arguments)
    }

    fn property(dictionary: *mut Dictionary, name: &str, arguments: &[Variant]) -> Outcome {
        call(dictionary, name, DISPATCH_PROPERTYGET, arguments)
    }

    #[test]
    fn names_are_found_ignoring_case() {
        let dictionary = new_dictionary();
        assert_eq!(id_of(dictionary, "add"), (S_OK, ADD));
        assert_eq!(id_of(dictionary, "COUNT"), (S_OK, COUNT));
        assert_eq!(id_of(dictionary, "_NewEnum"), (S_OK, NEW_ENUM));
        assert_eq!(
            id_of(dictionary, "Nothing"),
            (DISP_E_UNKNOWNNAME, DISPID_UNKNOWN)
        );
        // SAFETY: the last reference.
        assert_eq!(unsafe { release(dictionary) }, 0);
    }

    #[test]
    fn keys_and_items_keep_the_order_they_were_added_in() {
        let dictionary = new_dictionary();
        let fruit = [
            ("Apples", 3),
            ("Bananas", 5),
            ("Cherries", 7),
            ("Dates", 11),
        ];
        for (name, count) in fruit {
            let key = text(name);
            let added = method(dictionary, "Add", &[key, Variant::long(count)]);
            assert_eq!(added.code, S_OK);
            free(key);
        }
        let count = property(dictionary, "Count", &[]);
        assert_eq!((count.code, count.result), (S_OK, Variant::long(4)));
        let keys = method(dictionary, "Keys", &[]);
        assert_eq!(keys.result.kind, kind::ARRAY | kind::VARIANT);
        let array = keys.result.pointer() as *mut SafeArray;
        // SAFETY: an array of VARIANTs.
        let names: Vec<String> = unsafe { safe_array::variants(array) }
            .iter()
            .map(text_of)
            .collect();
        assert_eq!(names, ["Apples", "Bananas", "Cherries", "Dates"]);
        free(keys.result);
        let items = method(dictionary, "Items", &[]);
        let array = items.result.pointer() as *mut SafeArray;
        // SAFETY: an array of VARIANTs.
        let counts: Vec<Variant> = unsafe { safe_array::variants(array) }.to_vec();
        let expected: Vec<Variant> = fruit.iter().map(|&(_, n)| Variant::long(n)).collect();
        assert_eq!(counts, expected);
        free(items.result);
        // SAFETY: the last reference.
        assert_eq!(unsafe { release(dictionary) }, 0);
    }

    #[test]
    fn item_reads_puts_and_adds_missing_keys() {
        let dictionary = new_dictionary();
        let key = text("Elderberries");
        let put = call(
            dictionary,
            "Item",
            DISPATCH_PROPERTYPUT,
            &[key, Variant::long(13)],
        );
        assert_eq!(put.code, S_OK);
        let read = property(dictionary, "Item", &[key]);
        assert_eq!((read.code, read.result), (S_OK, Variant::long(13)));
        let missing = text("Figs");
        let added = property(dictionary, "Item", &[missing]);
        assert_eq!((added.code, added.result), (S_OK, Variant::EMPTY));
        let exists = method(dictionary, "Exists", &[missing]);
        assert_eq!(exists.result, Variant::boolean(true));
        let mut by_reference = key;
        let reference = Variant::new(kind::VARIANT | kind::BYREF, &raw mut by_reference as u64);
        let through = property(dictionary, "Item", &[reference]);
        assert_eq!(through.result, Variant::long(13));
        free(key);
        free(missing);
        // SAFETY: the last reference.
        assert_eq!(unsafe { release(dictionary) }, 0);
    }

    #[test]
    fn errors_come_back_as_exceptions() {
        let dictionary = new_dictionary();
        let key = text("Apples");
        assert_eq!(
            method(dictionary, "Add", &[key, Variant::long(1)]).code,
            S_OK
        );
        let twice = method(dictionary, "Add", &[key, Variant::long(2)]);
        assert_eq!(
            (twice.code, twice.exception.status),
            (DISP_E_EXCEPTION, KEY_EXISTS)
        );
        free(Variant::new(kind::BSTR, twice.exception.description as u64));
        let missing = text("Grapes");
        let removed = method(dictionary, "Remove", &[missing]);
        assert_eq!(removed.code, DISP_E_EXCEPTION);
        assert_eq!(removed.exception.status, ELEMENT_NOT_FOUND);
        let description = Variant::new(kind::BSTR, removed.exception.description as u64);
        assert_eq!(text_of(&description), "Element not found");
        free(description);
        let wrong_count = method(dictionary, "Remove", &[]);
        assert_eq!(wrong_count.code, DISP_E_BADPARAMCOUNT);
        let mode = call(
            dictionary,
            "CompareMode",
            DISPATCH_PROPERTYPUT,
            &[Variant::long(1)],
        );
        assert_eq!(mode.exception.status, INVALID_CALL);
        assert_eq!(method(dictionary, "Remove", &[key]).code, S_OK);
        free(key);
        free(missing);
        // SAFETY: the last reference.
        assert_eq!(unsafe { release(dictionary) }, 0);
    }

    #[test]
    fn keys_compare_binary_or_as_text() {
        let dictionary = new_dictionary();
        let lower = text("apples");
        let upper = text("APPLES");
        assert_eq!(
            method(dictionary, "Add", &[lower, Variant::long(1)]).code,
            S_OK
        );
        assert_eq!(
            method(dictionary, "Exists", &[upper]).result,
            Variant::boolean(false)
        );
        let one = Variant::new(kind::R8, 1.0f64.to_bits());
        assert_eq!(
            method(dictionary, "Add", &[Variant::long(1), lower]).code,
            S_OK
        );
        assert_eq!(
            method(dictionary, "Exists", &[one]).result,
            Variant::boolean(true)
        );
        let one_text = text("1");
        assert_eq!(
            method(dictionary, "Exists", &[one_text]).result,
            Variant::boolean(false)
        );
        assert_eq!(method(dictionary, "RemoveAll", &[]).code, S_OK);
        let mode = call(
            dictionary,
            "CompareMode",
            DISPATCH_PROPERTYPUT,
            &[Variant::long(1)],
        );
        assert_eq!(mode.code, S_OK);
        assert_eq!(
            method(dictionary, "Add", &[lower, Variant::long(1)]).code,
            S_OK
        );
        assert_eq!(
            method(dictionary, "Exists", &[upper]).result,
            Variant::boolean(true)
        );
        let mode = property(dictionary, "CompareMode", &[]);
        assert_eq!(mode.result, Variant::long(1));
        for value in [lower, upper, one_text] {
            free(value);
        }
        // SAFETY: the last reference.
        assert_eq!(unsafe { release(dictionary) }, 0);
    }

    #[test]
    fn a_key_is_renamed_in_place() {
        let dictionary = new_dictionary();
        let (first, second, third) = (text("a"), text("b"), text("c"));
        assert_eq!(
            method(dictionary, "Add", &[first, Variant::long(1)]).code,
            S_OK
        );
        assert_eq!(
            method(dictionary, "Add", &[second, Variant::long(2)]).code,
            S_OK
        );
        let renamed = call(dictionary, "Key", DISPATCH_PROPERTYPUT, &[first, third]);
        assert_eq!(renamed.code, S_OK);
        let clash = call(dictionary, "Key", DISPATCH_PROPERTYPUT, &[third, second]);
        assert_eq!(clash.exception.status, KEY_EXISTS);
        free(Variant::new(kind::BSTR, clash.exception.description as u64));
        assert_eq!(
            property(dictionary, "Item", &[third]).result,
            Variant::long(1)
        );
        assert_eq!(
            method(dictionary, "Exists", &[first]).result,
            Variant::boolean(false)
        );
        let keys = method(dictionary, "Keys", &[]);
        let array = keys.result.pointer() as *mut SafeArray;
        // SAFETY: an array of VARIANTs.
        let names: Vec<String> = unsafe { safe_array::variants(array) }
            .iter()
            .map(text_of)
            .collect();
        assert_eq!(names, ["c", "b"]);
        free(keys.result);
        for value in [first, second, third] {
            free(value);
        }
        // SAFETY: the last reference.
        assert_eq!(unsafe { release(dictionary) }, 0);
    }

    #[test]
    fn the_table_grows_and_survives_removals() {
        let dictionary = new_dictionary();
        for number in 0..200 {
            assert_eq!(
                method(dictionary, "Add", &[Variant::long(number)]).code,
                DISP_E_BADPARAMCOUNT
            );
            let item = Variant::long(number * 2);
            assert_eq!(
                method(dictionary, "Add", &[Variant::long(number), item]).code,
                S_OK
            );
        }
        for number in (0..200).step_by(2) {
            assert_eq!(
                method(dictionary, "Remove", &[Variant::long(number)]).code,
                S_OK
            );
        }
        for number in 200..300 {
            let item = Variant::long(number * 2);
            assert_eq!(
                method(dictionary, "Add", &[Variant::long(number), item]).code,
                S_OK
            );
        }
        assert_eq!(
            property(dictionary, "Count", &[]).result,
            Variant::long(200)
        );
        for number in 0..300 {
            let read = method(dictionary, "Exists", &[Variant::long(number)]);
            let expected = number >= 200 || number % 2 == 1;
            assert_eq!(read.result, Variant::boolean(expected), "{number}");
        }
        let item = property(dictionary, "Item", &[Variant::long(151)]);
        assert_eq!(item.result, Variant::long(302));
        // SAFETY: the last reference.
        assert_eq!(unsafe { release(dictionary) }, 0);
    }

    #[test]
    fn new_enum_walks_the_keys() {
        let dictionary = new_dictionary();
        for number in [10, 20, 30] {
            assert_eq!(
                method(dictionary, "Add", &[Variant::long(number), Variant::EMPTY]).code,
                S_OK
            );
        }
        let flags = DISPATCH_METHOD | DISPATCH_PROPERTYGET;
        let walker = call(dictionary, "_NewEnum", flags, &[]);
        assert_eq!(walker.result.kind, kind::UNKNOWN);
        let walker = walker.result.pointer() as *mut enumerator::Enumerator;
        let mut keys = [Variant::EMPTY; 4];
        let mut fetched = 0;
        // SAFETY: a live enumerator with room for four values.
        let code = unsafe { enumerator::next(walker, 4, keys.as_mut_ptr(), &mut fetched) };
        assert_eq!((code, fetched), (crate::com::S_FALSE, 3));
        assert_eq!(
            &keys[..3],
            &[Variant::long(10), Variant::long(20), Variant::long(30)]
        );
        // SAFETY: the enumerator's last reference.
        assert_eq!(unsafe { enumerator::release(walker) }, 0);
        // SAFETY: the last reference.
        assert_eq!(unsafe { release(dictionary) }, 0);
    }

    #[test]
    fn the_dual_table_methods_work_directly() {
        let dictionary = new_dictionary();
        let key = text("Apples");
        let item = Variant::long(3);
        // SAFETY: a live dictionary and VARIANT arguments from here on.
        assert_eq!(unsafe { add(dictionary, &key, &item) }, S_OK);
        // SAFETY: as above.
        assert_eq!(unsafe { add(dictionary, &key, &item) }, KEY_EXISTS);
        let mut count = 0;
        // SAFETY: as above.
        assert_eq!(unsafe { get_count(dictionary, &mut count) }, S_OK);
        assert_eq!(count, 1);
        let mut found = 0i16;
        // SAFETY: as above.
        assert_eq!(unsafe { exists(dictionary, &key, &mut found) }, S_OK);
        assert_eq!(found, -1);
        let mut read = Variant::EMPTY;
        // SAFETY: as above.
        assert_eq!(unsafe { get_item(dictionary, &key, &mut read) }, S_OK);
        assert_eq!(read, item);
        // SAFETY: as above.
        assert_eq!(unsafe { remove(dictionary, &key) }, S_OK);
        // SAFETY: as above.
        assert_eq!(unsafe { remove(dictionary, &key) }, ELEMENT_NOT_FOUND);
        free(key);
        let mut answer = 0usize;
        // SAFETY: as above.
        let code = unsafe { query_interface(dictionary, &INTERFACE, &mut answer) };
        assert_eq!((code, answer), (S_OK, dictionary as usize));
        // SAFETY: two references now.
        assert_eq!(unsafe { release(dictionary) }, 1);
        // SAFETY: the last one.
        assert_eq!(unsafe { release(dictionary) }, 0);
    }
}
