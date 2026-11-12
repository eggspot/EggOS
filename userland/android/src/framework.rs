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

/// An object an app made: its class, its fields by field index, and a string's or a text
/// view's text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Object {
    pub class: String,
    pub fields: BTreeMap<u32, u64>,
    pub text: Option<String>,
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
