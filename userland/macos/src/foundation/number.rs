//! NSNumber and the constant numbers clang's literals use: `NSConstantIntegerNumber`
//! `{isa, encoding, value}` (also NSNumber's own layout, with doubles kept as their bits),
//! `NSConstantFloatNumber` `{isa, f32}` and `NSConstantDoubleNumber` `{isa, f64}`, and the two
//! boolean objects `__kCFBooleanTrue` and `__kCFBooleanFalse` that `@YES` and `@NO` name.
//! Descriptions print integers in decimal, doubles with `%.16g` and floats with `%.7g`, as
//! macOS does.

use super::{
    BOOLEAN, BOOLEAN_META, CONSTANT_DOUBLE, CONSTANT_DOUBLE_META, CONSTANT_FLOAT,
    CONSTANT_FLOAT_META, CONSTANT_INTEGER, CONSTANT_INTEGER_META, Definition, NSNUMBER,
    NSNUMBER_META, autoreleased, define as define_class, is_number, new_instance, string_from_utf8,
};
use crate::format::{self, Arguments, Output};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::class::{self, IMMORTAL};
use crate::objc::reference_count::objc_retain;
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods};
use core::cmp::Ordering as Order;
use core::sync::atomic::{AtomicUsize, Ordering};

#[repr(C)]
struct NumberObject {
    isa: usize,
    encoding: *const u8,
    value: u64,
}

#[derive(Debug, Clone, Copy)]
enum Value {
    Signed(i64),
    Unsigned(u64),
    Float(f32),
    Double(f64),
}

impl Value {
    fn as_f64(self) -> f64 {
        match self {
            Value::Signed(value) => value as f64,
            Value::Unsigned(value) => value as f64,
            Value::Float(value) => f64::from(value),
            Value::Double(value) => value,
        }
    }

    fn as_i64(self) -> i64 {
        match self {
            Value::Signed(value) => value,
            Value::Unsigned(value) => value as i64,
            Value::Float(value) => value as i64,
            Value::Double(value) => value as i64,
        }
    }

    fn is_integer(self) -> bool {
        matches!(self, Value::Signed(_) | Value::Unsigned(_))
    }
}

fn value_of(object: Id) -> Value {
    let class = class::class_of(object);
    if class == (&raw const CONSTANT_DOUBLE).cast_mut() {
        // SAFETY: a constant double holds an f64 after its class.
        return Value::Double(unsafe { object.cast::<f64>().wrapping_add(1).read() });
    }
    if class == (&raw const CONSTANT_FLOAT).cast_mut() {
        // SAFETY: a constant float holds an f32 after its class.
        return Value::Float(unsafe { object.cast::<u8>().wrapping_add(8).cast::<f32>().read() });
    }
    // SAFETY: every other number has the integer number's layout.
    let number = unsafe { &*object.cast::<NumberObject>() };
    let encoding = if number.encoding.is_null() {
        b'q'
    } else {
        // SAFETY: an encoding is a NUL-terminated type string.
        unsafe { number.encoding.read() }
    };
    match encoding {
        b'C' | b'S' | b'I' | b'L' | b'Q' => Value::Unsigned(number.value),
        b'f' => Value::Float(f64::from_bits(number.value) as f32),
        b'd' => Value::Double(f64::from_bits(number.value)),
        _ => Value::Signed(number.value as i64),
    }
}

fn new_number(class: *mut ClassObject, encoding: &'static core::ffi::CStr, value: u64) -> Id {
    let object = new_instance(class);
    if object.is_null() {
        return object;
    }
    // SAFETY: a new number object.
    let number = unsafe { &mut *object.cast::<NumberObject>() };
    number.encoding = encoding.as_ptr().cast();
    number.value = value;
    object
}

fn number_class() -> *mut ClassObject {
    (&raw const NSNUMBER).cast_mut()
}

macro_rules! constructors {
    ($($class_name:ident, $init_name:ident, $type:ty, $encoding:literal, $bits:expr;)*) => {
        $(
            extern "C" fn $class_name(_: Id, _: Sel, value: $type) -> Id {
                let bits: fn($type) -> u64 = $bits;
                autoreleased(new_number(number_class(), $encoding, bits(value)))
            }

            extern "C" fn $init_name(this: Id, _: Sel, value: $type) -> Id {
                let bits: fn($type) -> u64 = $bits;
                // SAFETY: an allocated NSNumber.
                let number = unsafe { &mut *this.cast::<NumberObject>() };
                number.encoding = $encoding.as_ptr().cast();
                number.value = bits(value);
                this
            }
        )*
    };
}

constructors! {
    number_with_char, init_with_char, i8, c"c", |value| value as i64 as u64;
    number_with_unsigned_char, init_with_unsigned_char, u8, c"C", u64::from;
    number_with_short, init_with_short, i16, c"s", |value| value as i64 as u64;
    number_with_unsigned_short, init_with_unsigned_short, u16, c"S", u64::from;
    number_with_int, init_with_int, i32, c"i", |value| value as i64 as u64;
    number_with_unsigned_int, init_with_unsigned_int, u32, c"I", u64::from;
    number_with_long, init_with_long, i64, c"q", |value| value as u64;
    number_with_unsigned_long, init_with_unsigned_long, u64, c"Q", |value| value;
    number_with_float, init_with_float, f32, c"f", |value| f64::from(value).to_bits();
    number_with_double, init_with_double, f64, c"d", f64::to_bits;
}

extern "C" fn number_with_bool(_: Id, _: Sel, value: u8) -> Id {
    boolean(value != 0)
}

extern "C" fn init_with_bool(this: Id, _: Sel, value: u8) -> Id {
    crate::objc::reference_count::objc_release(this);
    objc_retain(boolean(value != 0))
}

extern "C" fn char_value(this: Id, _: Sel) -> i8 {
    value_of(this).as_i64() as i8
}

extern "C" fn short_value(this: Id, _: Sel) -> i16 {
    value_of(this).as_i64() as i16
}

extern "C" fn int_value(this: Id, _: Sel) -> i32 {
    value_of(this).as_i64() as i32
}

extern "C" fn long_value(this: Id, _: Sel) -> i64 {
    value_of(this).as_i64()
}

extern "C" fn unsigned_long_value(this: Id, _: Sel) -> u64 {
    match value_of(this) {
        Value::Unsigned(value) => value,
        other => other.as_i64() as u64,
    }
}

extern "C" fn unsigned_int_value(this: Id, sel: Sel) -> u32 {
    unsigned_long_value(this, sel) as u32
}

extern "C" fn float_value(this: Id, _: Sel) -> f32 {
    value_of(this).as_f64() as f32
}

extern "C" fn double_value(this: Id, _: Sel) -> f64 {
    value_of(this).as_f64()
}

extern "C" fn bool_value(this: Id, _: Sel) -> u8 {
    let value = value_of(this);
    u8::from(if value.is_integer() {
        value.as_i64() != 0
    } else {
        value.as_f64() != 0.0
    })
}

extern "C" fn obj_c_type(this: Id, _: Sel) -> *const u8 {
    match value_of(this) {
        Value::Double(_) if class::class_of(this) == (&raw const CONSTANT_DOUBLE).cast_mut() => {
            c"d".as_ptr().cast::<u8>()
        }
        Value::Float(_) if class::class_of(this) == (&raw const CONSTANT_FLOAT).cast_mut() => {
            c"f".as_ptr().cast::<u8>()
        }
        _ => {
            // SAFETY: an integer-layout number.
            let encoding = unsafe { (*this.cast::<NumberObject>()).encoding };
            if encoding.is_null() {
                c"q".as_ptr().cast::<u8>()
            } else {
                encoding
            }
        }
    }
}

struct Text {
    bytes: [u8; 64],
    length: usize,
}

impl Output for Text {
    fn write(&mut self, bytes: &[u8]) {
        let taken = bytes.len().min(self.bytes.len() - self.length);
        self.bytes[self.length..self.length + taken].copy_from_slice(&bytes[..taken]);
        self.length += taken;
    }
}

struct One(u64);

impl Arguments for One {
    fn integer(&mut self) -> u64 {
        self.0
    }

    fn double(&mut self) -> f64 {
        f64::from_bits(self.0)
    }
}

fn formatted(format_text: &[u8], value: u64) -> Text {
    let mut text = Text {
        bytes: [0; 64],
        length: 0,
    };
    // SAFETY: no `%s` conversion.
    unsafe { format::format(format_text, &mut One(value), &mut text) };
    text
}

fn description_text(this: Id) -> Text {
    match value_of(this) {
        Value::Signed(value) => formatted(b"%lld", value as u64),
        Value::Unsigned(value) => formatted(b"%llu", value),
        Value::Float(value) => formatted(b"%.7g", f64::from(value).to_bits()),
        Value::Double(value) => formatted(b"%.16g", value.to_bits()),
    }
}

extern "C" fn description(this: Id, _: Sel) -> Id {
    let text = description_text(this);
    string_from_utf8(&text.bytes[..text.length])
}

fn compare_values(left: Value, right: Value) -> Order {
    if left.is_integer() && right.is_integer() {
        let wide = |value: Value| match value {
            Value::Unsigned(value) => i128::from(value),
            other => i128::from(other.as_i64()),
        };
        return wide(left).cmp(&wide(right));
    }
    left.as_f64()
        .partial_cmp(&right.as_f64())
        .unwrap_or(Order::Equal)
}

extern "C" fn compare(this: Id, _: Sel, other: Id) -> isize {
    match compare_values(value_of(this), value_of(other)) {
        Order::Less => -1,
        Order::Equal => 0,
        Order::Greater => 1,
    }
}

extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
    u8::from(
        this == other
            || (!other.is_null()
                && is_number(other)
                && compare_values(value_of(this), value_of(other)) == Order::Equal),
    )
}

extern "C" fn hash(this: Id, _: Sel) -> usize {
    let value = value_of(this);
    let real = value.as_f64();
    if value.is_integer() || real == (real as i64) as f64 {
        value.as_i64() as usize
    } else {
        real.to_bits() as usize
    }
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    objc_retain(this)
}

static TRUE: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
static FALSE: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];

fn boolean(value: bool) -> Id {
    if value {
        (&raw const TRUE).cast_mut().cast()
    } else {
        (&raw const FALSE).cast_mut().cast()
    }
}

/// Makes the two boolean numbers.
pub fn set_up_booleans() -> bool {
    for (object, value) in [(&TRUE, 1), (&FALSE, 0)] {
        object[0].store(&raw const BOOLEAN as usize, Ordering::Relaxed);
        object[1].store(c"c".as_ptr() as usize, Ordering::Relaxed);
        object[2].store(value, Ordering::Relaxed);
    }
    true
}

/// The address of `__kCFBooleanTrue` or `__kCFBooleanFalse`, the boolean objects themselves
/// (`kCFBooleanTrue` points at the first).
pub fn boolean_object(value: bool) -> u64 {
    boolean(value) as u64
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "initWithChar:" => init_with_char,
        "initWithUnsignedChar:" => init_with_unsigned_char,
        "initWithShort:" => init_with_short,
        "initWithUnsignedShort:" => init_with_unsigned_short,
        "initWithInt:" => init_with_int,
        "initWithUnsignedInt:" => init_with_unsigned_int,
        "initWithLong:" => init_with_long,
        "initWithLongLong:" => init_with_long,
        "initWithInteger:" => init_with_long,
        "initWithUnsignedLong:" => init_with_unsigned_long,
        "initWithUnsignedLongLong:" => init_with_unsigned_long,
        "initWithUnsignedInteger:" => init_with_unsigned_long,
        "initWithFloat:" => init_with_float,
        "initWithDouble:" => init_with_double,
        "initWithBool:" => init_with_bool,
        "charValue" => char_value,
        "unsignedCharValue" => char_value,
        "shortValue" => short_value,
        "unsignedShortValue" => short_value,
        "intValue" => int_value,
        "unsignedIntValue" => unsigned_int_value,
        "longValue" => long_value,
        "longLongValue" => long_value,
        "integerValue" => long_value,
        "unsignedLongValue" => unsigned_long_value,
        "unsignedLongLongValue" => unsigned_long_value,
        "unsignedIntegerValue" => unsigned_long_value,
        "floatValue" => float_value,
        "doubleValue" => double_value,
        "boolValue" => bool_value,
        "objCType" => obj_c_type,
        "stringValue" => description,
        "description" => description,
        "compare:" => compare,
        "isEqual:" => is_equal,
        "isEqualToNumber:" => is_equal,
        "hash" => hash,
        "copyWithZone:" => copy_with_zone,
    ];
    let class_methods = methods![
        "numberWithChar:" => number_with_char,
        "numberWithUnsignedChar:" => number_with_unsigned_char,
        "numberWithShort:" => number_with_short,
        "numberWithUnsignedShort:" => number_with_unsigned_short,
        "numberWithInt:" => number_with_int,
        "numberWithUnsignedInt:" => number_with_unsigned_int,
        "numberWithLong:" => number_with_long,
        "numberWithLongLong:" => number_with_long,
        "numberWithInteger:" => number_with_long,
        "numberWithUnsignedLong:" => number_with_unsigned_long,
        "numberWithUnsignedLongLong:" => number_with_unsigned_long,
        "numberWithUnsignedInteger:" => number_with_unsigned_long,
        "numberWithFloat:" => number_with_float,
        "numberWithDouble:" => number_with_double,
        "numberWithBool:" => number_with_bool,
    ];
    let size = core::mem::size_of::<NumberObject>();
    let constants: [(&'static [u8], &'static ClassObject, &'static ClassObject); 4] = [
        (
            b"NSConstantIntegerNumber\0",
            &CONSTANT_INTEGER,
            &CONSTANT_INTEGER_META,
        ),
        (
            b"NSConstantFloatNumber\0",
            &CONSTANT_FLOAT,
            &CONSTANT_FLOAT_META,
        ),
        (
            b"NSConstantDoubleNumber\0",
            &CONSTANT_DOUBLE,
            &CONSTANT_DOUBLE_META,
        ),
        (b"__NSCFBoolean\0", &BOOLEAN, &BOOLEAN_META),
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSNumber\0",
            class: &NSNUMBER,
            meta: &NSNUMBER_META,
            superclass: &NSOBJECT,
            instance_size: size,
            flags: 0,
            instance_methods: &instance,
            class_methods: &class_methods,
        },
    ) && constants.iter().all(|&(name, class, meta)| {
        define_class(
            runtime,
            &Definition {
                name,
                class,
                meta,
                superclass: &NSNUMBER,
                instance_size: size,
                flags: IMMORTAL,
                instance_methods: &[],
                class_methods: &[],
            },
        )
    })
}
