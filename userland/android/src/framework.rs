//! The Android framework as the layer provides it (D472, D500): the classes an app's first
//! screens need (`Object`, `Activity` and its AndroidX forms, `View`, `TextView`, `Button`,
//! `EditText`, `LinearLayout`), each method a native function of the layer, told apart by
//! name and short signature, and where every call an app makes goes: to the app's own
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
/// The framework's views that show text.
pub const TEXT_VIEWS: [&str; 3] = [TEXT_VIEW, BUTTON, EDIT_TEXT];
/// The framework's button.
pub const BUTTON: &str = "Landroid/widget/Button;";
/// The framework's text field.
pub const EDIT_TEXT: &str = "Landroid/widget/EditText;";
/// The framework's linear layout.
pub const LINEAR_LAYOUT: &str = "Landroid/widget/LinearLayout;";
/// AndroidX's `RecyclerView`, which apps carry in their own DEX file and the layer stands in for
/// (D516), with its layout managers.
pub const RECYCLER_VIEW: &str = "Landroidx/recyclerview/widget/RecyclerView;";
/// The view holder class whose `itemView` field holds a row's view.
pub const VIEW_HOLDER: &str = "Landroidx/recyclerview/widget/RecyclerView$ViewHolder;";
const REPLACED: [&str; 3] = [
    RECYCLER_VIEW,
    "Landroidx/recyclerview/widget/LinearLayoutManager;",
    "Landroidx/recyclerview/widget/GridLayoutManager;",
];

/// Whether the layer stands in for `class` even when the app's DEX file defines it.
pub fn is_replaced(class: &str) -> bool {
    REPLACED.contains(&class)
}

/// The framework's image view (D509).
pub const IMAGE_VIEW: &str = "Landroid/widget/ImageView;";
/// The framework's list view (D508).
pub const LIST_VIEW: &str = "Landroid/widget/ListView;";
const ARRAY_LIST: &str = "Ljava/util/ArrayList;";
const LISTS: [&str; 3] = [ARRAY_LIST, "Ljava/util/List;", "Ljava/util/AbstractList;"];
const ARRAY_ADAPTER: &str = "Landroid/widget/ArrayAdapter;";
const ADAPTER_VIEWS: [&str; 3] = [LIST_VIEW, "Landroid/widget/AdapterView;", RECYCLER_VIEW];
const LAYOUT_INFLATER: &str = "Landroid/view/LayoutInflater;";
const VIEW: &str = "Landroid/view/View;";
const VIEW_GROUP: &str = "Landroid/view/ViewGroup;";
const CONTEXT_CLASSES: [&str; 2] = [
    "Landroid/content/Context;",
    "Landroid/content/ContextWrapper;",
];
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
    /// `Activity.setContentView(int)`: the layout resource the activity shows, inflated (D500).
    SetContentLayout,
    /// `findViewById(int)`, on an activity or a view: the view with that id in what it shows.
    FindViewById,
    /// A view's or layout's constructor: an empty one of the object's class.
    ViewInit,
    /// `View.setOnClickListener(OnClickListener)`.
    SetOnClickListener,
    /// `TextView.setText(int)`: a string resource's text.
    SetTextResource,
    /// `TextView.getText()`: its text, as a string.
    GetText,
    /// `getString(int)` on a context: a string resource's text.
    GetString,
    /// `ViewGroup.addView(View)`.
    AddView,
    /// `LinearLayout.setOrientation(int)`.
    SetOrientation,
    /// `TextView.setHint(CharSequence)` on an `EditText` (D507).
    SetHint,
    /// `ArrayList`'s constructors (D508): an empty list.
    ListInit,
    /// `List.add(Object)`.
    ListAdd,
    /// `List.get(int)`.
    ListGet,
    /// `List.set(int, Object)`.
    ListSet,
    /// `List.remove(int)`.
    ListRemove,
    /// `List.size()`.
    ListSize,
    /// `List.clear()`.
    ListClear,
    /// `ArrayAdapter(Context, int, T[] or List)`: the items it shows.
    AdapterInit,
    /// `ListView.setAdapter(ListAdapter)`.
    SetAdapter,
    /// `AdapterView.setOnItemClickListener(OnItemClickListener)`.
    SetOnItemClickListener,
    /// `ImageView.setImageResource(int)` (D509).
    SetImageResource,
    /// `LayoutInflater.from(Context)` and `getLayoutInflater()` (D511): an inflater.
    InflaterFrom,
    /// `LayoutInflater.inflate(int, ViewGroup[, boolean])`: the layout, inflated.
    Inflate,
    /// An exception's constructor with its message first (D504).
    ExceptionInit,
    /// `Throwable.getMessage()`: its message, or null.
    GetMessage,
}

const THROWABLE: &str = "Ljava/lang/Throwable;";
const EXCEPTION: &str = "Ljava/lang/Exception;";
const ERROR: &str = "Ljava/lang/Error;";
const RUNTIME_EXCEPTION: &str = "Ljava/lang/RuntimeException;";
const ILLEGAL_ARGUMENT: &str = "Ljava/lang/IllegalArgumentException;";
const INDEX_OUT_OF_BOUNDS: &str = "Ljava/lang/IndexOutOfBoundsException;";
/// Java's `NullPointerException`, which the layer throws for a null object (D504).
pub const NULL_POINTER: &str = "Ljava/lang/NullPointerException;";
/// `ArithmeticException`, for a division by zero.
pub const ARITHMETIC: &str = "Ljava/lang/ArithmeticException;";
/// `ArrayIndexOutOfBoundsException`.
pub const ARRAY_INDEX: &str = "Ljava/lang/ArrayIndexOutOfBoundsException;";
/// `NegativeArraySizeException`.
pub const NEGATIVE_ARRAY_SIZE: &str = "Ljava/lang/NegativeArraySizeException;";
/// `ClassCastException`.
pub const CLASS_CAST: &str = "Ljava/lang/ClassCastException;";
/// `OutOfMemoryError`.
pub const OUT_OF_MEMORY: &str = "Ljava/lang/OutOfMemoryError;";

/// The framework's exception classes and their superclasses.
const EXCEPTION_PARENTS: [(&str, &str); 16] = [
    (THROWABLE, OBJECT),
    (EXCEPTION, THROWABLE),
    (ERROR, THROWABLE),
    (RUNTIME_EXCEPTION, EXCEPTION),
    ("Ljava/io/IOException;", EXCEPTION),
    (ARITHMETIC, RUNTIME_EXCEPTION),
    (NULL_POINTER, RUNTIME_EXCEPTION),
    (CLASS_CAST, RUNTIME_EXCEPTION),
    ("Ljava/lang/IllegalStateException;", RUNTIME_EXCEPTION),
    (ILLEGAL_ARGUMENT, RUNTIME_EXCEPTION),
    ("Ljava/lang/NumberFormatException;", ILLEGAL_ARGUMENT),
    (INDEX_OUT_OF_BOUNDS, RUNTIME_EXCEPTION),
    (ARRAY_INDEX, INDEX_OUT_OF_BOUNDS),
    (NEGATIVE_ARRAY_SIZE, RUNTIME_EXCEPTION),
    ("Ljava/lang/UnsupportedOperationException;", RUNTIME_EXCEPTION),
    (OUT_OF_MEMORY, ERROR),
];

/// The superclass of framework class `class`, when the layer knows it (D504).
pub fn framework_superclass(class: &str) -> Option<&'static str> {
    EXCEPTION_PARENTS
        .iter()
        .find(|(named, _)| *named == class)
        .map(|(_, parent)| *parent)
}

fn is_exception(class: &str) -> bool {
    EXCEPTION_PARENTS.iter().any(|(named, _)| *named == class)
}

/// The framework's implementation of `class`'s method `name` with short signature `shorty`
/// (`VI` takes an `int`, `VL` an object), if the layer has one.
pub fn native(class: &str, name: &str, shorty: &str) -> Option<Native> {
    let activity = ACTIVITY_CLASSES.contains(&class);
    let context = activity || CONTEXT_CLASSES.contains(&class);
    let text = TEXT_VIEWS.contains(&class);
    let view = text
        || matches!(class, VIEW | VIEW_GROUP | LINEAR_LAYOUT | LIST_VIEW | IMAGE_VIEW)
        || class == RECYCLER_VIEW;
    let list = LISTS.contains(&class);
    let adapter_view = ADAPTER_VIEWS.contains(&class);
    let takes_int = shorty == "VI";
    Some(match (class, name) {
        (OBJECT, "<init>") => Native::Nothing,
        (_, "<init>" | "onCreate" | "onStart" | "onResume") if activity => Native::Nothing,
        (_, "setContentView") if activity && takes_int => Native::SetContentLayout,
        (_, "setContentView") if activity => Native::SetContentView,
        (_, "findViewById") if activity || view => Native::FindViewById,
        (_, "getString") if context => Native::GetString,
        (TEXT_VIEW, "<init>") => Native::TextViewInit,
        (_, "<init>") if view => Native::ViewInit,
        (_, "setText") if text && takes_int => Native::SetTextResource,
        (_, "setText") if text => Native::SetText,
        (_, "getText") if text => Native::GetText,
        (_, "setOnClickListener") if view => Native::SetOnClickListener,
        (_, "addView") if view => Native::AddView,
        (_, "setOrientation") if view => Native::SetOrientation,
        (EDIT_TEXT, "setHint") if !takes_int => Native::SetHint,
        (IMAGE_VIEW, "setImageResource") if takes_int => Native::SetImageResource,
        (LAYOUT_INFLATER, "from") => Native::InflaterFrom,
        (_, "getLayoutInflater") if activity => Native::InflaterFrom,
        (LAYOUT_INFLATER, "inflate") if shorty.starts_with("LI") => Native::Inflate,
        (ARRAY_LIST, "<init>") => Native::ListInit,
        (_, "add") if list && shorty == "ZL" => Native::ListAdd,
        (_, "get") if list => Native::ListGet,
        (_, "set") if list => Native::ListSet,
        (_, "remove") if list && shorty == "LI" => Native::ListRemove,
        (_, "size") if list => Native::ListSize,
        (_, "clear") if list => Native::ListClear,
        (ARRAY_ADAPTER, "<init>") if shorty == "VLIL" => Native::AdapterInit,
        (ARRAY_ADAPTER, "<init>") => Native::Nothing,
        (ARRAY_ADAPTER, "notifyDataSetChanged") => Native::Nothing,
        (_, "setAdapter") if adapter_view => Native::SetAdapter,
        (_, "setOnItemClickListener") if adapter_view => Native::SetOnItemClickListener,
        (_, "<init>") if is_exception(class) && shorty.starts_with("VL") => Native::ExceptionInit,
        (_, "<init>") if is_exception(class) => Native::Nothing,
        (_, "getMessage" | "getLocalizedMessage") if is_exception(class) => Native::GetMessage,
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
    let class_of_method = dex.method_class(method);
    if let Some(class) = class_of_method.as_deref().filter(|class| is_replaced(class)) {
        let name = dex.method_name(method).unwrap_or_default();
        let shorty = dex.method_shorty(method).unwrap_or_default();
        return Target::Native(native(class, &name, &shorty).unwrap_or(Native::Nothing));
    }
    if let Some(&offset) = compiled.get(&method) {
        return Target::Compiled(offset);
    }
    let (Some(name), Some(mut class)) = (dex.method_name(method), class_of_method) else {
        return Target::Missing;
    };
    let shorty = dex.method_shorty(method).unwrap_or_default();
    for _ in 0..MOST_ANCESTORS {
        if let Some(native) = native(&class, &name, &shorty) {
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
    let shorty = dex.method_shorty(method).unwrap_or_default();
    if is_replaced(receiver) {
        return Target::Native(native(receiver, &name, &shorty).unwrap_or(Native::Nothing));
    }
    let mut class = String::from(receiver);
    for _ in 0..MOST_ANCESTORS {
        if let Some(native) = native(&class, &name, &shorty) {
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
        let parent = match dex.class_index(&class) {
            Some(index) => dex.superclass(index),
            None => Some(String::from(framework_superclass(&class)?)),
        };
        match parent {
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
/// text, an array's elements, and a view's state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Object {
    pub class: String,
    pub fields: BTreeMap<u32, u64>,
    pub text: Option<String>,
    pub elements: Option<alloc::vec::Vec<u64>>,
    pub view: Option<View>,
    /// Another object this one stands on: an adapter's items, a list view's adapter (D508).
    pub reference: u64,
}

/// What a view holds (D500): its id, the views inside it (a layout's), whether a linear
/// layout stacks them, the activity method a click calls by name (`android:onClick`), and the
/// click listener object set on it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct View {
    pub id: u32,
    pub children: alloc::vec::Vec<u64>,
    pub vertical: bool,
    pub on_click: Option<String>,
    pub listener: u64,
    /// An `EditText`'s hint, shown while it is empty (D507).
    pub hint: Option<String>,
    /// An image view's picture, its place among the layer's decoded images (D509).
    pub image: Option<u32>,
}

/// The framework class a layout element names: `TextView` and the other short names are in
/// `android.widget` (`View` and `ViewGroup` in `android.view`), a dotted name is whole.
pub fn element_class(element: &str) -> String {
    match element {
        _ if element.contains('.') => format!("L{};", element.replace('.', "/")),
        "View" | "ViewGroup" => format!("Landroid/view/{element};"),
        _ => format!("Landroid/widget/{element};"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activities_and_text_views_are_the_framework_s() {
        assert_eq!(native("Landroid/app/Activity;", "onCreate", "VL"), Some(Native::Nothing));
        assert_eq!(
            native("Landroidx/appcompat/app/AppCompatActivity;", "setContentView", "VL"),
            Some(Native::SetContentView)
        );
        assert_eq!(
            native("Landroid/app/Activity;", "setContentView", "VI"),
            Some(Native::SetContentLayout)
        );
        assert_eq!(native(TEXT_VIEW, "setText", "VL"), Some(Native::SetText));
        assert_eq!(native(BUTTON, "setText", "VI"), Some(Native::SetTextResource));
        assert_eq!(native(BUTTON, "setOnClickListener", "VL"), Some(Native::SetOnClickListener));
        assert_eq!(native("Landroid/widget/ImageView;", "setText", "VL"), None);
    }

    #[test]
    fn layout_elements_name_framework_classes() {
        assert_eq!(element_class("Button"), "Landroid/widget/Button;");
        assert_eq!(element_class("View"), "Landroid/view/View;");
        assert_eq!(
            element_class("androidx.constraintlayout.widget.ConstraintLayout"),
            "Landroidx/constraintlayout/widget/ConstraintLayout;"
        );
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
