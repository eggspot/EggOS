//! The Android framework as the layer provides it, part 1 (D472): the classes an app's first
//! screen needs (`Object`, `Activity` and its AndroidX forms, `TextView`), each method a
//! native function of the layer, and where every call an app makes goes: to the app's own
//! compiled code, found up its class's superclasses, or to the framework.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use eggos_android_package::dex::Dex;

/// The framework's activity classes an app's activity extends.
pub const ACTIVITY_CLASSES: [&str; 4] = [
    "Landroid/app/Activity;",
    "Landroidx/appcompat/app/AppCompatActivity;",
    "Landroidx/activity/ComponentActivity;",
    "Landroidx/fragment/app/FragmentActivity;",
];
/// The string class.
pub const STRING_CLASS: &str = "Ljava/lang/String;";
const TEXT_VIEW: &str = "Landroid/widget/TextView;";
const OBJECT: &str = "Ljava/lang/Object;";
const MOST_ANCESTORS: usize = 16;

/// A framework method the layer implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Native {
    /// A constructor or a lifecycle step with nothing to do yet.
    Nothing,
    /// `Activity.setContentView(View)`: the view the activity shows.
    SetContentView,
    /// `TextView.<init>(Context)`: an empty text view.
    TextViewInit,
    /// `TextView.setText(CharSequence)`.
    SetText,
}

/// The framework's implementation of `class`'s method `name`, if the layer has one.
pub fn native(class: &str, name: &str) -> Option<Native> {
    let activity = ACTIVITY_CLASSES.contains(&class);
    Some(match (class, name) {
        (OBJECT, "<init>") => Native::Nothing,
        (_, "<init>" | "onCreate" | "onStart" | "onResume") if activity => Native::Nothing,
        (_, "setContentView") if activity => Native::SetContentView,
        (TEXT_VIEW, "<init>") => Native::TextViewInit,
        (TEXT_VIEW, "setText") => Native::SetText,
        _ => return None,
    })
}

/// Where a call to a method goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The app's compiled code, at this offset in its code.
    Compiled(usize),
    /// The framework.
    Native(Native),
    /// Nowhere the layer knows: calling it stops the app with the method's name.
    Missing,
}

fn defined_in(
    dex: &Dex<'_>,
    class: u32,
    name: &str,
    compiled: &BTreeMap<u32, usize>,
) -> Option<usize> {
    dex.methods(class).iter().find_map(|method| {
        let offset = compiled.get(&method.index)?;
        (dex.method_name(method.index)? == name).then_some(*offset)
    })
}

/// Where a call to method `method` of `dex` goes, given the code offsets of the methods that
/// were compiled: the method itself when it was, else the first class up its class's
/// superclasses that defines a compiled method of that name or whose framework method the
/// layer implements.
pub fn resolve(dex: &Dex<'_>, method: u32, compiled: &BTreeMap<u32, usize>) -> Target {
    if let Some(&offset) = compiled.get(&method) {
        return Target::Compiled(offset);
    }
    let (Some(name), Some(mut class)) = (dex.method_name(method), dex.method_class(method)) else {
        return Target::Missing;
    };
    for _ in 0..MOST_ANCESTORS {
        if let Some(native) = native(&class, &name) {
            return Target::Native(native);
        }
        let Some(index) = dex.class_index(&class) else {
            return Target::Missing;
        };
        if let Some(offset) = defined_in(dex, index, &name, compiled) {
            return Target::Compiled(offset);
        }
        match dex.superclass(index) {
            Some(parent) => class = parent,
            None => return Target::Missing,
        }
    }
    Target::Missing
}

fn overriding(
    dex: &Dex<'_>,
    class: u32,
    method: u32,
    compiled: &BTreeMap<u32, usize>,
) -> Option<usize> {
    let name = dex.method_name(method)?;
    let prototype = dex.method_prototype(method)?;
    dex.methods(class).iter().find_map(|candidate| {
        let offset = compiled.get(&candidate.index)?;
        let same = dex.method_name(candidate.index)? == name
            && dex.method_prototype(candidate.index)? == prototype;
        same.then_some(*offset)
    })
}

/// Where a virtual or interface call of method `method` on an object of class `receiver` goes
/// (D499): the first class up the receiver's superclasses that the framework implements the
/// method for or that defines a compiled method of the same name and prototype; else where
/// the call would go without dispatch ([`resolve`]).
pub fn resolve_virtual(
    dex: &Dex<'_>,
    receiver: &str,
    method: u32,
    compiled: &BTreeMap<u32, usize>,
) -> Target {
    let Some(name) = dex.method_name(method) else {
        return Target::Missing;
    };
    let mut class = String::from(receiver);
    for _ in 0..MOST_ANCESTORS {
        if let Some(native) = native(&class, &name) {
            return Target::Native(native);
        }
        let Some(index) = dex.class_index(&class) else {
            break;
        };
        if let Some(offset) = overriding(dex, index, method, compiled) {
            return Target::Compiled(offset);
        }
        match dex.superclass(index) {
            Some(parent) => class = parent,
            None => break,
        }
    }
    resolve(dex, method, compiled)
}

/// Whether an object of class `class` is a `wanted` (D499): the same class, or one up its
/// superclasses the DEX file defines, or `Object`; `None` when its ancestry leaves the file
/// before deciding (a framework class, or an interface, which the layer does not follow yet).
pub fn is_instance(dex: &Dex<'_>, class: &str, wanted: &str) -> Option<bool> {
    if wanted == OBJECT {
        return Some(true);
    }
    let mut class = String::from(class);
    for _ in 0..MOST_ANCESTORS {
        if class == wanted {
            return Some(true);
        }
        let index = dex.class_index(&class)?;
        match dex.superclass(index) {
            Some(parent) => class = parent,
            None => return Some(false),
        }
    }
    None
}

/// An array element `value` as an array of `descriptor` keeps it: narrowed to the element
/// type's width, signed but for `char` and `boolean`, as Java's stores do.
pub fn array_element(descriptor: &str, value: u64) -> u64 {
    match descriptor.as_bytes().get(1) {
        Some(b'B') => value as u8 as i8 as i32 as u32 as u64,
        Some(b'S') => value as u16 as i16 as i32 as u32 as u64,
        Some(b'C') => u64::from(value as u16),
        Some(b'Z') => value & 1,
        Some(b'I') => u64::from(value as u32),
        _ => value,
    }
}

/// The descriptor of the activity a manifest names as `name` in package `package`:
/// `.MainActivity` and `MainActivity` are in the package, `org.example.Main` is whole.
pub fn activity_descriptor(package: &str, name: &str) -> String {
    let whole = match (name.strip_prefix('.'), name.contains('.')) {
        (Some(relative), _) => format!("{package}.{relative}"),
        (None, false) => format!("{package}.{name}"),
        (None, true) => String::from(name),
    };
    format!("L{};", whole.replace('.', "/"))
}

/// An object an app made: its class, its fields by field index, a string's or a text view's
/// text, and an array's elements.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Object {
    pub class: String,
    pub fields: BTreeMap<u32, u64>,
    pub text: Option<String>,
    pub elements: Option<alloc::vec::Vec<u64>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activities_and_text_views_are_the_framework_s() {
        assert_eq!(native("Landroid/app/Activity;", "onCreate"), Some(Native::Nothing));
        assert_eq!(
            native("Landroidx/appcompat/app/AppCompatActivity;", "setContentView"),
            Some(Native::SetContentView)
        );
        assert_eq!(native(TEXT_VIEW, "setText"), Some(Native::SetText));
        assert_eq!(native("Landroid/widget/Button;", "setText"), None);
    }

    #[test]
    fn array_elements_are_narrowed_as_java_stores_them() {
        assert_eq!(array_element("[C", 0xffff_ffff), 0xffff);
        assert_eq!(array_element("[S", 0xffff), 0xffff_ffff);
        assert_eq!(array_element("[B", 0x80), 0xffff_ff80);
        assert_eq!(array_element("[Z", 3), 1);
        assert_eq!(array_element("[J", u64::MAX), u64::MAX);
    }

    #[test]
    fn launcher_names_become_descriptors_in_their_package() {
        assert_eq!(
            activity_descriptor("org.example.notes", ".MainActivity"),
            "Lorg/example/notes/MainActivity;"
        );
        assert_eq!(
            activity_descriptor("org.example.notes", "MainActivity"),
            "Lorg/example/notes/MainActivity;"
        );
        assert_eq!(
            activity_descriptor("org.example.notes", "org.other.Start"),
            "Lorg/other/Start;"
        );
    }
}
