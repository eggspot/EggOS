//! The core of Foundation and CoreFoundation (D142): NSString, NSArray, NSDictionary, NSNumber
//! and their mutable and constant (compiler-emitted literal) classes, NSBundle, NSLog and the
//! class and selector name functions, as classes of the layer's own Objective-C runtime; and
//! (D175) NSException, NSProcessInfo, NSDate, NSError, NSData, NSSet, NSTimer, NSRunLoop and
//! the string constants Foundation exports.

pub mod string_text;

#[cfg(target_env = "musl")]
mod array;
#[cfg(target_env = "musl")]
mod bundle;
#[cfg(target_env = "musl")]
mod constants;
#[cfg(target_env = "musl")]
mod data;
#[cfg(target_env = "musl")]
mod date;
#[cfg(target_env = "musl")]
mod description;
#[cfg(target_env = "musl")]
mod dictionary;
#[cfg(target_env = "musl")]
mod error;
#[cfg(target_env = "musl")]
mod exception;
#[cfg(target_env = "musl")]
mod log;
#[cfg(target_env = "musl")]
mod number;
#[cfg(target_env = "musl")]
mod process_info;
#[cfg(target_env = "musl")]
mod set;
#[cfg(target_env = "musl")]
mod string;
#[cfg(target_env = "musl")]
mod timer;

#[cfg(target_env = "musl")]
pub use runtime::*;

#[cfg(target_env = "musl")]
mod runtime {
    use super::{
        array, bundle, constants, data, date, dictionary, error, exception, log, number,
        process_info, set, string, timer,
    };
    use crate::objc::abi::{ClassObject, Id};
    use crate::objc::class::{self, BuiltinClass};
    use crate::objc::root;
    use crate::objc::{RUNTIME, Runtime, reference_count};

    macro_rules! class_pairs {
        ($($class:ident, $meta:ident;)*) => {
            $(
                pub static $class: ClassObject = ClassObject::EMPTY;
                pub static $meta: ClassObject = ClassObject::EMPTY;
            )*
        };
    }

    class_pairs! {
        NSSTRING, NSSTRING_META;
        NSMUTABLESTRING, NSMUTABLESTRING_META;
        CONSTANT_STRING, CONSTANT_STRING_META;
        UTF8_BUFFER, UTF8_BUFFER_META;
        NSARRAY, NSARRAY_META;
        NSMUTABLEARRAY, NSMUTABLEARRAY_META;
        CONSTANT_ARRAY, CONSTANT_ARRAY_META;
        NSDICTIONARY, NSDICTIONARY_META;
        NSMUTABLEDICTIONARY, NSMUTABLEDICTIONARY_META;
        CONSTANT_DICTIONARY, CONSTANT_DICTIONARY_META;
        NSNUMBER, NSNUMBER_META;
        CONSTANT_INTEGER, CONSTANT_INTEGER_META;
        CONSTANT_FLOAT, CONSTANT_FLOAT_META;
        CONSTANT_DOUBLE, CONSTANT_DOUBLE_META;
        BOOLEAN, BOOLEAN_META;
        NSBUNDLE, NSBUNDLE_META;
        NSEXCEPTION, NSEXCEPTION_META;
        NSPROCESSINFO, NSPROCESSINFO_META;
        NSDATE, NSDATE_META;
        NSERROR, NSERROR_META;
        NSDATA, NSDATA_META;
        NSMUTABLEDATA, NSMUTABLEDATA_META;
        NSSET, NSSET_META;
        NSMUTABLESET, NSMUTABLESET_META;
        NSTIMER, NSTIMER_META;
        NSRUNLOOP, NSRUNLOOP_META;
    }

    /// The class object `class` as the pointer the runtime takes.
    pub fn class_pointer(class: &'static ClassObject) -> *mut ClassObject {
        (class as *const ClassObject).cast_mut()
    }

    /// A new instance of `class`, counted once.
    pub fn new_instance(class: *mut ClassObject) -> Id {
        reference_count::create_instance(class, 0)
    }

    /// Puts a counted-once object in the current pool and returns it.
    pub fn autoreleased(object: Id) -> Id {
        if object.is_null() {
            return object;
        }
        reference_count::objc_autorelease(object)
    }

    /// Whether `object` is one of Foundation's strings (any of the three classes).
    pub fn is_string(object: Id) -> bool {
        root::is_kind_of(object, &NSSTRING)
    }

    pub fn is_array(object: Id) -> bool {
        root::is_kind_of(object, &NSARRAY)
    }

    pub fn is_dictionary(object: Id) -> bool {
        root::is_kind_of(object, &NSDICTIONARY)
    }

    pub fn is_number(object: Id) -> bool {
        root::is_kind_of(object, &NSNUMBER)
    }

    pub fn is_date(object: Id) -> bool {
        root::is_kind_of(object, &NSDATE)
    }

    pub fn is_data(object: Id) -> bool {
        root::is_kind_of(object, &NSDATA)
    }

    pub fn is_set(object: Id) -> bool {
        root::is_kind_of(object, &NSSET)
    }

    /// One of Foundation's classes to define.
    pub struct Definition<'a> {
        pub name: &'static [u8],
        pub class: &'static ClassObject,
        pub meta: &'static ClassObject,
        pub superclass: &'static ClassObject,
        pub instance_size: usize,
        pub flags: u32,
        pub instance_methods: &'a [(&'static [u8], usize)],
        pub class_methods: &'a [(&'static [u8], usize)],
    }

    pub fn define(runtime: &mut Runtime, definition: &Definition<'_>) -> bool {
        class::define(
            runtime,
            &BuiltinClass {
                name: definition.name,
                class: definition.class,
                meta: definition.meta,
                superclass: Some(definition.superclass),
                instance_size: definition.instance_size,
                flags: definition.flags,
                instance_methods: definition.instance_methods,
                class_methods: definition.class_methods,
            },
        )
    }

    /// Defines Foundation's classes and its additions to NSObject; false if there is no
    /// memory.
    pub fn set_up() -> bool {
        RUNTIME.with(|runtime| {
            string::define(runtime)
                && array::define(runtime)
                && dictionary::define(runtime)
                && number::define(runtime)
                && bundle::define(runtime)
                && exception::define(runtime)
                && process_info::define(runtime)
                && date::define(runtime)
                && error::define(runtime)
                && data::define(runtime)
                && set::define(runtime)
                && timer::define(runtime)
                && super::description::extend_object(runtime)
        }) && number::set_up_booleans()
            && array::set_up_empty()
            && dictionary::set_up_empty()
            && constants::set_up()
    }

    /// The address of the Foundation or CoreFoundation function or variable `name`.
    pub fn resolve(name: &[u8]) -> Option<u64> {
        let class = |object: &'static ClassObject| Some(object as *const ClassObject as u64);
        match name {
            b"_OBJC_CLASS_$_NSString" => class(&NSSTRING),
            b"_OBJC_METACLASS_$_NSString" => class(&NSSTRING_META),
            b"_OBJC_CLASS_$_NSMutableString" => class(&NSMUTABLESTRING),
            b"_OBJC_METACLASS_$_NSMutableString" => class(&NSMUTABLESTRING_META),
            b"___CFConstantStringClassReference" => class(&CONSTANT_STRING),
            b"_OBJC_CLASS_$_NSArray" => class(&NSARRAY),
            b"_OBJC_METACLASS_$_NSArray" => class(&NSARRAY_META),
            b"_OBJC_CLASS_$_NSMutableArray" => class(&NSMUTABLEARRAY),
            b"_OBJC_METACLASS_$_NSMutableArray" => class(&NSMUTABLEARRAY_META),
            b"_OBJC_CLASS_$_NSConstantArray" => class(&CONSTANT_ARRAY),
            b"_OBJC_CLASS_$_NSDictionary" => class(&NSDICTIONARY),
            b"_OBJC_METACLASS_$_NSDictionary" => class(&NSDICTIONARY_META),
            b"_OBJC_CLASS_$_NSMutableDictionary" => class(&NSMUTABLEDICTIONARY),
            b"_OBJC_METACLASS_$_NSMutableDictionary" => class(&NSMUTABLEDICTIONARY_META),
            b"_OBJC_CLASS_$_NSConstantDictionary" => class(&CONSTANT_DICTIONARY),
            b"_OBJC_CLASS_$_NSNumber" => class(&NSNUMBER),
            b"_OBJC_METACLASS_$_NSNumber" => class(&NSNUMBER_META),
            b"_OBJC_CLASS_$_NSConstantIntegerNumber" => class(&CONSTANT_INTEGER),
            b"_OBJC_CLASS_$_NSConstantFloatNumber" => class(&CONSTANT_FLOAT),
            b"_OBJC_CLASS_$_NSConstantDoubleNumber" => class(&CONSTANT_DOUBLE),
            b"_OBJC_CLASS_$_NSBundle" => class(&NSBUNDLE),
            b"_OBJC_METACLASS_$_NSBundle" => class(&NSBUNDLE_META),
            b"_OBJC_CLASS_$_NSException" => class(&NSEXCEPTION),
            b"_OBJC_METACLASS_$_NSException" => class(&NSEXCEPTION_META),
            b"_OBJC_CLASS_$_NSProcessInfo" => class(&NSPROCESSINFO),
            b"_OBJC_METACLASS_$_NSProcessInfo" => class(&NSPROCESSINFO_META),
            b"_OBJC_CLASS_$_NSDate" => class(&NSDATE),
            b"_OBJC_METACLASS_$_NSDate" => class(&NSDATE_META),
            b"_OBJC_CLASS_$_NSError" => class(&NSERROR),
            b"_OBJC_METACLASS_$_NSError" => class(&NSERROR_META),
            b"_OBJC_CLASS_$_NSData" => class(&NSDATA),
            b"_OBJC_METACLASS_$_NSData" => class(&NSDATA_META),
            b"_OBJC_CLASS_$_NSMutableData" => class(&NSMUTABLEDATA),
            b"_OBJC_METACLASS_$_NSMutableData" => class(&NSMUTABLEDATA_META),
            b"_OBJC_CLASS_$_NSSet" => class(&NSSET),
            b"_OBJC_METACLASS_$_NSSet" => class(&NSSET_META),
            b"_OBJC_CLASS_$_NSMutableSet" => class(&NSMUTABLESET),
            b"_OBJC_METACLASS_$_NSMutableSet" => class(&NSMUTABLESET_META),
            b"_OBJC_CLASS_$_NSTimer" => class(&NSTIMER),
            b"_OBJC_METACLASS_$_NSTimer" => class(&NSTIMER_META),
            b"_OBJC_CLASS_$_NSRunLoop" => class(&NSRUNLOOP),
            b"_OBJC_METACLASS_$_NSRunLoop" => class(&NSRUNLOOP_META),
            b"___NSArray0__struct" => Some(array::empty_address()),
            b"___NSDictionary0__struct" => Some(dictionary::empty_address()),
            b"___kCFBooleanTrue" => Some(number::boolean_object(true)),
            b"___kCFBooleanFalse" => Some(number::boolean_object(false)),
            _ => log::resolve(name)
                .or_else(|| exception::resolve(name))
                .or_else(|| constants::resolve(name)),
        }
    }

    pub use super::bundle::{LONGEST_PATH, app_name, info_string, own_icon_path};
    pub use super::exception::terminate_with;
    pub use super::timer::{fire_due, next_due_in_ms};
    pub use super::string::{string_from_utf8, text_of, with_utf8};
}
