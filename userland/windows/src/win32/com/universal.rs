//! Custom interfaces out of process (D746): a proxy for an interface the other end's object
//! serves at its own vtable, built from the interface's type information (registered by
//! `RegisterTypeLib`, D745). Each vtable slot of the proxy is a function that reads the
//! program's arguments by the method's description (a value in its word, a `VARIANT` passed by
//! value through its pointer, or a place the method reads or fills through a pointer), sends
//! them as one call, and writes the places' new values back; the other end's stub calls the
//! object's slot with cells of its own. Methods with more than seven parameters or with
//! floating-point parameters passed by value are refused with `E_NOTIMPL`.

use super::carry;
use super::local_server;
use super::proxy::{self, LinkObjects, Proxy};
use crate::com::variant::{self, Variant, kind};
use crate::com::wire::{self, Argument, Value, passing};
use crate::com::{E_INVALIDARG, HResult};
use alloc::vec::Vec;

const E_NOTIMPL: HResult = 0x8000_4001_u32 as i32;
const RPC_E_DISCONNECTED: HResult = 0x8001_0108_u32 as i32;
const DISP_E_TYPEMISMATCH: HResult = 0x8002_0005_u32 as i32;
/// The most parameters a proxied method may have.
pub(super) const MOST_PARAMETERS: usize = 7;
/// The vtable slots a universal proxy has.
pub(super) const SLOTS: usize = 64;

/// How a method takes one parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ParameterShape {
    pub(super) passing: u8,
    pub(super) kind: u16,
    pub(super) out_only: bool,
}

/// The methods of a custom interface, by vtable slot; `None` for slots it cannot carry.
#[derive(Debug, Default)]
pub(super) struct Shapes {
    pub(super) methods: Vec<Option<Vec<ParameterShape>>>,
}

fn word_value(objects: &mut LinkObjects, kind_of: u16, word: usize) -> Result<Value, HResult> {
    let held = match kind_of {
        kind::BSTR | kind::DISPATCH | kind::UNKNOWN => Variant::new(kind_of, word as u64),
        other if other & kind::ARRAY != 0 => Variant::new(other, word as u64),
        other => {
            let size = variant::value_size(other).ok_or(DISP_E_TYPEMISMATCH)?;
            let mask = match size {
                1 => 0xff,
                2 => 0xffff,
                4 => 0xffff_ffff,
                _ => u64::MAX,
            };
            Variant::new(other, word as u64 & mask)
        }
    };
    carry::value_of(objects, &held)
}

fn place_value(
    objects: &mut LinkObjects,
    shape: ParameterShape,
    word: usize,
) -> Result<Value, HResult> {
    if word == 0 {
        return Err(E_INVALIDARG);
    }
    if shape.out_only {
        return Ok(Value::Plain {
            kind: kind::EMPTY,
            word: 0,
        });
    }
    match shape.kind {
        kind::VARIANT => {
            // SAFETY: the program's `VARIANT*` argument.
            let held = unsafe { (word as *const Variant).read_unaligned() };
            carry::value_of(objects, &held)
        }
        other => {
            let size = match other & kind::ARRAY {
                0 => variant::value_size(other).ok_or(DISP_E_TYPEMISMATCH)?,
                _ => 8,
            };
            let mut bytes = [0u8; 8];
            let from = word as *const u8;
            // SAFETY: the program's pointer to a value of the parameter's kind.
            unsafe { core::ptr::copy_nonoverlapping(from, bytes.as_mut_ptr(), size.min(8)) };
            word_value(objects, other, u64::from_le_bytes(bytes) as usize)
        }
    }
}

/// A call of slot `slot` of the universal proxy `this` with the program's `words`.
///
/// # Safety
/// `this` must be a live universal proxy and `words` the arguments the program passed.
pub(super) unsafe fn call_slot(
    this: *mut Proxy,
    slot: usize,
    words: [usize; MOST_PARAMETERS],
) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { proxy::view(this) };
    let Some(Some(shapes)) = proxy::shapes_of(found).and_then(|shapes| shapes.methods.get(slot))
    else {
        return E_NOTIMPL;
    };
    let mut objects = LinkObjects::new(proxy::link_of(found));
    let mut arguments = Vec::with_capacity(shapes.len());
    for (shape, &word) in shapes.iter().zip(&words) {
        let value = match shape.passing {
            passing::WORD => word_value(&mut objects, shape.kind, word),
            passing::VARIANT => {
                // SAFETY: the program's pointer to the `VARIANT` copy it passes.
                let held = unsafe { (word as *const Variant).read_unaligned() };
                carry::value_of(&mut objects, &held)
            }
            _ => place_value(&mut objects, *shape, word),
        };
        match value {
            Ok(value) => arguments.push(Argument {
                passing: shape.passing,
                kind: shape.kind,
                value,
            }),
            Err(status) => return status,
        }
    }
    let message = wire::call(proxy::id_of(found), slot as u16, &arguments);
    let Some((status, changed)) = proxy::ask_link(found, &message)
        .and_then(|body| wire::read_called(&body))
    else {
        return RPC_E_DISCONNECTED;
    };
    let places = shapes
        .iter()
        .zip(&words)
        .filter(|(shape, _)| shape.passing == passing::PLACE);
    for ((shape, &word), value) in places.zip(&changed) {
        let Ok(made) = carry::variant_of(&mut objects, value) else {
            continue;
        };
        // SAFETY: the program's place for the parameter, of its kind.
        unsafe {
            match shape.out_only {
                true => carry::write_fresh(word, shape.kind, made),
                false => carry::write_back(word, shape.kind, made),
            }
        }
    }
    status
}

/// Serves a `Call` request on connection `fd`: the object's slot `slot` with `arguments`.
pub(super) fn serve_call(fd: usize, object: u32, slot: u16, arguments: &[Argument]) -> Vec<u8> {
    let Some(target) = local_server::object_of(fd, object) else {
        return wire::called(RPC_E_DISCONNECTED, &[]);
    };
    if arguments.len() > MOST_PARAMETERS || usize::from(slot) >= SLOTS {
        return wire::called(E_NOTIMPL, &[]);
    }
    let link = local_server::link_of(fd);
    if link.is_null() {
        return wire::called(RPC_E_DISCONNECTED, &[]);
    }
    let mut objects = LinkObjects::new(link);
    let mut cells = [Variant::EMPTY; MOST_PARAMETERS];
    let mut made = 0;
    let mut words = [0usize; 9];
    words[0] = target;
    for (index, argument) in arguments.iter().enumerate() {
        let value = match carry::variant_of(&mut objects, &argument.value) {
            Ok(value) => value,
            Err(status) => {
                clear(&mut cells[..made]);
                return wire::called(status, &[]);
            }
        };
        cells[index] = match (argument.passing, argument.kind) {
            (passing::PLACE, kind::VARIANT) | (passing::VARIANT, _) => value,
            (passing::PLACE, base) if value.kind == kind::EMPTY => Variant::new(base, 0),
            _ => value,
        };
        made = index + 1;
        words[index + 1] = match argument.passing {
            passing::WORD => cells[index].data[0] as usize,
            passing::VARIANT => &raw mut cells[index] as usize,
            _ => carry::cell_place(argument.kind, &mut cells[index]),
        };
    }
    // SAFETY: a live object: its first word is its table, which holds `slot`.
    let address = unsafe {
        (target as *const *const usize)
            .read()
            .add(usize::from(slot))
            .read()
    };
    // SAFETY: the object's method with the cells, all alive across the call.
    let returned = unsafe { super::super::call_program_nine(address, words) };
    let Some(returned) = returned else {
        clear(&mut cells[..made]);
        return wire::called(E_NOTIMPL, &[]);
    };
    let mut changed = Vec::new();
    for (argument, cell) in arguments.iter().zip(cells.iter_mut()) {
        if argument.passing != passing::PLACE {
            continue;
        }
        let now = carry::cell_value(argument.kind, cell);
        let carried = carry::value_of(&mut objects, &now).unwrap_or(Value::Plain {
            kind: kind::EMPTY,
            word: 0,
        });
        changed.push(carried);
        *cell = now;
    }
    clear(&mut cells[..made]);
    wire::called(returned as u32 as i32, &changed)
}

fn clear(cells: &mut [Variant]) {
    for cell in cells {
        // SAFETY: a VARIANT the stub made and owns.
        unsafe { variant::clear(cell) };
    }
}

#[derive(Debug)]
#[repr(transparent)]
struct Table<const N: usize>([*const (); N]);

// SAFETY: the table holds functions' addresses, fixed for the program's life.
unsafe impl<const N: usize> Sync for Table<N> {}

macro_rules! slot_functions {
    ($($name:ident = $slot:literal),* $(,)?) => {
        $(
            #[expect(clippy::too_many_arguments, reason = "a vtable slot's own parameters")]
            unsafe extern "efiapi" fn $name(
                this: *mut Proxy,
                a: usize,
                b: usize,
                c: usize,
                d: usize,
                e: usize,
                f: usize,
                g: usize,
            ) -> HResult {
                // SAFETY: the program calls this slot of a universal proxy with its arguments.
                unsafe { call_slot(this, $slot, [a, b, c, d, e, f, g]) }
            }
        )*

        static UNIVERSAL: Table<SLOTS> = Table([
            export_entry!(proxy::query_interface),
            export_entry!(proxy::add_reference),
            export_entry!(proxy::release),
            $(export_entry!($name),)*
        ]);
    };
}

slot_functions!(
    slot_3 = 3,
    slot_4 = 4,
    slot_5 = 5,
    slot_6 = 6,
    slot_7 = 7,
    slot_8 = 8,
    slot_9 = 9,
    slot_10 = 10,
    slot_11 = 11,
    slot_12 = 12,
    slot_13 = 13,
    slot_14 = 14,
    slot_15 = 15,
    slot_16 = 16,
    slot_17 = 17,
    slot_18 = 18,
    slot_19 = 19,
    slot_20 = 20,
    slot_21 = 21,
    slot_22 = 22,
    slot_23 = 23,
    slot_24 = 24,
    slot_25 = 25,
    slot_26 = 26,
    slot_27 = 27,
    slot_28 = 28,
    slot_29 = 29,
    slot_30 = 30,
    slot_31 = 31,
    slot_32 = 32,
    slot_33 = 33,
    slot_34 = 34,
    slot_35 = 35,
    slot_36 = 36,
    slot_37 = 37,
    slot_38 = 38,
    slot_39 = 39,
    slot_40 = 40,
    slot_41 = 41,
    slot_42 = 42,
    slot_43 = 43,
    slot_44 = 44,
    slot_45 = 45,
    slot_46 = 46,
    slot_47 = 47,
    slot_48 = 48,
    slot_49 = 49,
    slot_50 = 50,
    slot_51 = 51,
    slot_52 = 52,
    slot_53 = 53,
    slot_54 = 54,
    slot_55 = 55,
    slot_56 = 56,
    slot_57 = 57,
    slot_58 = 58,
    slot_59 = 59,
    slot_60 = 60,
    slot_61 = 61,
    slot_62 = 62,
    slot_63 = 63,
);

/// The address of the universal proxy's table.
pub(super) fn table_address() -> usize {
    UNIVERSAL.0.as_ptr() as usize
}
