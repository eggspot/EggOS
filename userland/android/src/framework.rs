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
const REPLACED: [&str; 7] = [
    RECYCLER_VIEW,
    "Landroidx/recyclerview/widget/LinearLayoutManager;",
    "Landroidx/recyclerview/widget/GridLayoutManager;",
    FRAGMENT,
    FRAGMENT_MANAGER,
    FRAGMENT_TRANSACTION,
    "Landroidx/fragment/app/BackStackRecord;",
];

/// `android.content.Intent` (D765).
pub const INTENT: &str = "Landroid/content/Intent;";
/// `java.lang.Class`, which `const-class` makes.
pub const CLASS_CLASS: &str = "Ljava/lang/Class;";
/// AndroidX's `Fragment`, which the layer stands in for (D765), and the platform's.
pub const FRAGMENT: &str = "Landroidx/fragment/app/Fragment;";
const PLATFORM_FRAGMENT: &str = "Landroid/app/Fragment;";
/// AndroidX's `FragmentManager`.
pub const FRAGMENT_MANAGER: &str = "Landroidx/fragment/app/FragmentManager;";
/// AndroidX's `FragmentTransaction`.
pub const FRAGMENT_TRANSACTION: &str = "Landroidx/fragment/app/FragmentTransaction;";
const FRAGMENTS: [&str; 2] = [FRAGMENT, PLATFORM_FRAGMENT];
const FRAGMENT_MANAGERS: [&str; 2] = [FRAGMENT_MANAGER, "Landroid/app/FragmentManager;"];
const FRAGMENT_TRANSACTIONS: [&str; 2] =
    [FRAGMENT_TRANSACTION, "Landroid/app/FragmentTransaction;"];

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
    /// An exception's constructor without a message (D761): its stack trace recorded.
    ExceptionInitEmpty,
    /// `Throwable.printStackTrace()` (D761): the trace written to the standard error.
    PrintStackTrace,
    /// `Throwable.getStackTrace()`: an array of `StackTraceElement`s, innermost first.
    GetStackTrace,
    /// `Throwable.getCause()`: what an `ExceptionInInitializerError` wraps, or null.
    GetCause,
    /// `Throwable.fillInStackTrace()`: the trace recorded again, the exception returned.
    FillInStackTrace,
    /// `StackTraceElement.getClassName()`.
    FrameClassName,
    /// `StackTraceElement.getMethodName()`.
    FrameMethodName,
    /// `StackTraceElement.toString()`.
    FrameText,
    /// `TextView.addTextChangedListener(TextWatcher)` (D762).
    AddTextChangedListener,
    /// `TextView.removeTextChangedListener(TextWatcher)`.
    RemoveTextChangedListener,
    /// `TextView.setOnEditorActionListener(OnEditorActionListener)`.
    SetOnEditorActionListener,
    /// `TextView.setInputType(int)`.
    SetInputType,
    /// `TextView.getInputType()`.
    GetInputType,
    /// `TextView.setImeOptions(int)`.
    SetImeOptions,
    /// `BitmapFactory.decodeResource(Resources, int)` (D763): the drawable's picture.
    DecodeResource,
    /// `BitmapFactory.decodeFile(String)`: a PNG or WebP file's picture.
    DecodeFile,
    /// `Bitmap.getWidth()`.
    BitmapWidth,
    /// `Bitmap.getHeight()`.
    BitmapHeight,
    /// `ImageView.setImageBitmap(Bitmap)`.
    SetImageBitmap,
    /// `ImageView.setScaleType(ImageView.ScaleType)`.
    SetScaleType,
    /// `Context.getResources()` (D764): the app's resources.
    GetResources,
    /// `Resources.getDimension(int)`: a dimension in pixels, as a float.
    GetDimension,
    /// `Resources.getDimensionPixelSize(int)` and `getDimensionPixelOffset(int)`.
    GetDimensionPixels,
    /// `Resources.getColor(int)` and `Context.getColor(int)`: `0xAARRGGBB`.
    GetColour,
    /// `Resources.getInteger(int)`.
    GetInteger,
    /// `Resources.getBoolean(int)`.
    GetBoolean,
    /// `Intent(Context, Class)` (D765): an explicit intent for an activity of the app.
    IntentInit,
    /// `Intent(String)` and `Intent()`: an intent by action, or empty.
    IntentAction,
    /// `Intent.putExtra(String, value)`: the value kept under its name; the intent returned.
    PutExtra,
    /// `Intent.getStringExtra(String)`, and the other getters with a default.
    GetExtra,
    /// `Intent.hasExtra(String)`.
    HasExtra,
    /// `Activity.startActivity(Intent)`.
    StartActivity,
    /// `Activity.startActivityForResult(Intent, int)`.
    StartActivityForResult,
    /// `Activity.finish()` and the default `onBackPressed()`.
    Finish,
    /// `Activity.getIntent()`.
    GetIntent,
    /// `Activity.setResult(int[, Intent])`.
    SetResult,
    /// `getSupportFragmentManager()`, `getFragmentManager()`, `getChildFragmentManager()`.
    GetFragmentManager,
    /// `FragmentManager.beginTransaction()`.
    BeginTransaction,
    /// `FragmentTransaction.add(int, Fragment[, String])` or `add(Fragment, String)`: kept for
    /// `commit`.
    TransactionAdd,
    /// `FragmentTransaction.replace(int, Fragment[, String])`.
    TransactionReplace,
    /// `FragmentTransaction.remove(Fragment)`.
    TransactionRemove,
    /// `FragmentTransaction.commit()` and its kin: the operations done.
    Commit,
    /// A method that answers its receiver (`addToBackStack`, `setReorderingAllowed`).
    ReturnThis,
    /// `Fragment.getActivity()`, `requireActivity()`, `getContext()`, `requireContext()`.
    FragmentHost,
    /// `Fragment.getView()` and `requireView()`.
    FragmentView,
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
/// `ExceptionInInitializerError` (D761), around an exception a static initializer threw.
pub const EXCEPTION_IN_INITIALIZER: &str = "Ljava/lang/ExceptionInInitializerError;";
/// `NoClassDefFoundError`, for a class whose static initializer failed before.
pub const NO_CLASS_DEFINITION: &str = "Ljava/lang/NoClassDefFoundError;";
/// `StackTraceElement`, one frame of a stack trace.
pub const STACK_TRACE_ELEMENT: &str = "Ljava/lang/StackTraceElement;";
const LINKAGE_ERROR: &str = "Ljava/lang/LinkageError;";

const EXCEPTION_PARENTS: [(&str, &str); 20] = [
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
    (
        "Ljava/lang/UnsupportedOperationException;",
        RUNTIME_EXCEPTION,
    ),
    (OUT_OF_MEMORY, ERROR),
    (LINKAGE_ERROR, ERROR),
    (EXCEPTION_IN_INITIALIZER, LINKAGE_ERROR),
    (NO_CLASS_DEFINITION, LINKAGE_ERROR),
    (RESOURCE_NOT_FOUND, RUNTIME_EXCEPTION),
];

/// Whether class `class` is an `Error`, which a static initializer's failure passes on
/// unwrapped (D761).
pub fn is_error(dex: &Dex<'_>, class: &str) -> bool {
    is_instance(dex, class, ERROR) == Some(true)
}

/// A class descriptor as Java writes the class's name: `Lcom/example/Main;` is
/// `com.example.Main`.
pub fn class_name(descriptor: &str) -> String {
    descriptor
        .strip_prefix('L')
        .and_then(|inner| inner.strip_suffix(';'))
        .unwrap_or(descriptor)
        .replace('/', ".")
}

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
    let view =
        text || matches!(
            class,
            VIEW | VIEW_GROUP | LINEAR_LAYOUT | LIST_VIEW | IMAGE_VIEW
        ) || class == RECYCLER_VIEW;
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
        (_, "getResources") if context => Native::GetResources,
        (INTENT, "<init>") if shorty == "VLL" => Native::IntentInit,
        (INTENT, "<init>") => Native::IntentAction,
        (INTENT, "putExtra") => Native::PutExtra,
        (INTENT, "hasExtra") => Native::HasExtra,
        (INTENT, name) if name.starts_with("get") && name.ends_with("Extra") => Native::GetExtra,
        (_, "startActivity") if activity => Native::StartActivity,
        (_, "startActivityForResult") if activity => Native::StartActivityForResult,
        (_, "finish" | "onBackPressed") if activity => Native::Finish,
        (_, "getIntent") if activity => Native::GetIntent,
        (_, "setResult") if activity => Native::SetResult,
        (_, "getSupportFragmentManager" | "getFragmentManager") if activity => {
            Native::GetFragmentManager
        }
        (_, "getChildFragmentManager" | "getParentFragmentManager")
            if FRAGMENTS.contains(&class) =>
        {
            Native::GetFragmentManager
        }
        (_, "beginTransaction") if FRAGMENT_MANAGERS.contains(&class) => Native::BeginTransaction,
        (_, "add") if FRAGMENT_TRANSACTIONS.contains(&class) => Native::TransactionAdd,
        (_, "replace") if FRAGMENT_TRANSACTIONS.contains(&class) => Native::TransactionReplace,
        (_, "remove") if FRAGMENT_TRANSACTIONS.contains(&class) => Native::TransactionRemove,
        (_, "commit" | "commitNow" | "commitAllowingStateLoss" | "commitNowAllowingStateLoss")
            if FRAGMENT_TRANSACTIONS.contains(&class) =>
        {
            Native::Commit
        }
        (_, _) if FRAGMENT_TRANSACTIONS.contains(&class) => Native::ReturnThis,
        (_, "getActivity" | "requireActivity" | "getContext" | "requireContext")
            if FRAGMENTS.contains(&class) =>
        {
            Native::FragmentHost
        }
        (_, "getView" | "requireView") if FRAGMENTS.contains(&class) => Native::FragmentView,
        (_, "<init>") if FRAGMENTS.contains(&class) => Native::Nothing,
        (_, "getColor") if context => Native::GetColour,
        (RESOURCES, "getString") => Native::GetString,
        (RESOURCES, "getDimension") => Native::GetDimension,
        (RESOURCES, "getDimensionPixelSize" | "getDimensionPixelOffset") => {
            Native::GetDimensionPixels
        }
        (RESOURCES, "getColor") => Native::GetColour,
        (RESOURCES, "getInteger") => Native::GetInteger,
        (RESOURCES, "getBoolean") => Native::GetBoolean,
        (TEXT_VIEW, "<init>") => Native::TextViewInit,
        (_, "<init>") if view => Native::ViewInit,
        (_, "setText") if text && takes_int => Native::SetTextResource,
        (_, "setText") if text => Native::SetText,
        (_, "getText") if text => Native::GetText,
        (_, "setOnClickListener") if view => Native::SetOnClickListener,
        (_, "addView") if view => Native::AddView,
        (_, "setOrientation") if view => Native::SetOrientation,
        (EDIT_TEXT, "setHint") if !takes_int => Native::SetHint,
        (_, "addTextChangedListener") if text => Native::AddTextChangedListener,
        (_, "removeTextChangedListener") if text => Native::RemoveTextChangedListener,
        (_, "setOnEditorActionListener") if text => Native::SetOnEditorActionListener,
        (_, "setInputType") if text => Native::SetInputType,
        (_, "getInputType") if text => Native::GetInputType,
        (_, "setImeOptions") if text => Native::SetImeOptions,
        (BITMAP_FACTORY, "decodeResource") => Native::DecodeResource,
        (BITMAP_FACTORY, "decodeFile") => Native::DecodeFile,
        (BITMAP, "getWidth") => Native::BitmapWidth,
        (BITMAP, "getHeight") => Native::BitmapHeight,
        (IMAGE_VIEW, "setImageBitmap") => Native::SetImageBitmap,
        (IMAGE_VIEW, "setScaleType") => Native::SetScaleType,
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
        (_, "<init>") if is_exception(class) => Native::ExceptionInitEmpty,
        (_, "getMessage" | "getLocalizedMessage") if is_exception(class) => Native::GetMessage,
        (_, "printStackTrace") if is_exception(class) && shorty == "V" => Native::PrintStackTrace,
        (_, "getStackTrace") if is_exception(class) => Native::GetStackTrace,
        (_, "getCause") if is_exception(class) => Native::GetCause,
        (_, "fillInStackTrace") if is_exception(class) => Native::FillInStackTrace,
        (STACK_TRACE_ELEMENT, "getClassName") => Native::FrameClassName,
        (STACK_TRACE_ELEMENT, "getMethodName") => Native::FrameMethodName,
        (STACK_TRACE_ELEMENT, "toString") => Native::FrameText,
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
    if let Some(class) = class_of_method
        .as_deref()
        .filter(|class| is_replaced(class))
    {
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
    /// Another object this one stands on: an adapter's items, a list view's adapter (D508),
    /// an exception's cause (D761).
    pub reference: u64,
    /// An exception's stack trace: the methods it was made in, innermost first (D761).
    pub trace: alloc::vec::Vec<u32>,
    /// An intent's extras by name (D765): a string's object or a number.
    pub extras: alloc::vec::Vec<(String, u64)>,
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
    /// A text field's `TextWatcher`s, called around every edit (D762).
    pub watchers: alloc::vec::Vec<u64>,
    /// A text field's `OnEditorActionListener`, called for Enter.
    pub editor_listener: u64,
    /// A text field's input type flags: digits only, a masked password (D762).
    pub input_type: u32,
    /// The editor action Enter reports (`imeOptions`; 0, `IME_NULL`, when none).
    pub ime_action: u32,
    /// Its width and height in pixels when its layout fixes them (D763).
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// An image view's `ScaleType` ordinal; `fitCenter` when unset.
    pub scale_type: Option<u32>,
}

/// `Resources.NotFoundException`, for a resource id the table lacks (D764).
pub const RESOURCE_NOT_FOUND: &str = "Landroid/content/res/Resources$NotFoundException;";

/// `android.content.res.Resources` (D764).
pub const RESOURCES: &str = "Landroid/content/res/Resources;";

/// `android.graphics.Bitmap` (D763): a picture the layer decoded.
pub const BITMAP: &str = "Landroid/graphics/Bitmap;";
/// `android.graphics.BitmapFactory`.
pub const BITMAP_FACTORY: &str = "Landroid/graphics/BitmapFactory;";
/// `ImageView.ScaleType`, whose constants the layer makes on their first use.
pub const SCALE_TYPE: &str = "Landroid/widget/ImageView$ScaleType;";
/// `ImageView.ScaleType`'s constants, by ordinal.
pub const SCALE_TYPES: [&str; 8] = [
    "MATRIX",
    "FIT_XY",
    "FIT_START",
    "FIT_CENTER",
    "FIT_END",
    "CENTER",
    "CENTER_CROP",
    "CENTER_INSIDE",
];

const INPUT_CLASS: u32 = 0xf;
const CLASS_TEXT: u32 = 0x1;
const CLASS_NUMBER: u32 = 0x2;
const CLASS_PHONE: u32 = 0x3;
const VARIATION: u32 = 0xff0;
const TEXT_PASSWORD_VARIATIONS: [u32; 2] = [0x80, 0xe0];
const NUMBER_PASSWORD: u32 = 0x10;
const NUMBER_SIGNED: u32 = 0x1000;
const NUMBER_DECIMAL: u32 = 0x2000;
const PHONE_CHARACTERS: &str = "0123456789+-() #*.,;/N";

/// Whether input type `input_type` hides what is typed (a password field, D762).
pub fn is_password(input_type: u32) -> bool {
    let variation = input_type & VARIATION;
    match input_type & INPUT_CLASS {
        CLASS_TEXT => TEXT_PASSWORD_VARIATIONS.contains(&variation),
        CLASS_NUMBER => variation == NUMBER_PASSWORD,
        _ => false,
    }
}

/// What a field of input type `input_type` keeps of `typed`: a number field its digits (a
/// leading minus when signed, one decimal point when decimal), a phone field its dialling
/// characters, any other field everything (D762).
pub fn accepted(input_type: u32, typed: &str) -> String {
    match input_type & INPUT_CLASS {
        CLASS_NUMBER => {
            let mut kept = String::new();
            let mut point = false;
            for character in typed.chars() {
                let keep = match character {
                    '0'..='9' => true,
                    '-' => input_type & NUMBER_SIGNED != 0 && kept.is_empty(),
                    '.' => input_type & NUMBER_DECIMAL != 0 && !point,
                    _ => false,
                };
                if keep {
                    point |= character == '.';
                    kept.push(character);
                }
            }
            kept
        }
        CLASS_PHONE => typed
            .chars()
            .filter(|character| PHONE_CHARACTERS.contains(*character))
            .collect(),
        _ => String::from(typed),
    }
}

/// Where `after` differs from `before` (D762): the first changed character, how many
/// characters were replaced, and how many replaced them, as `TextWatcher` reports an edit.
pub fn changed_span(before: &str, after: &str) -> (usize, usize, usize) {
    let old: alloc::vec::Vec<char> = before.chars().collect();
    let new: alloc::vec::Vec<char> = after.chars().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    (prefix, old.len() - prefix - suffix, new.len() - prefix - suffix)
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
        assert_eq!(
            native("Landroid/app/Activity;", "onCreate", "VL"),
            Some(Native::Nothing)
        );
        assert_eq!(
            native(
                "Landroidx/appcompat/app/AppCompatActivity;",
                "setContentView",
                "VL"
            ),
            Some(Native::SetContentView)
        );
        assert_eq!(
            native("Landroid/app/Activity;", "setContentView", "VI"),
            Some(Native::SetContentLayout)
        );
        assert_eq!(native(TEXT_VIEW, "setText", "VL"), Some(Native::SetText));
        assert_eq!(
            native(BUTTON, "setText", "VI"),
            Some(Native::SetTextResource)
        );
        assert_eq!(
            native(BUTTON, "setOnClickListener", "VL"),
            Some(Native::SetOnClickListener)
        );
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

    #[test]
    fn input_types_filter_and_mask_what_is_typed() {
        assert!(is_password(0x81));
        assert!(is_password(0x12));
        assert!(!is_password(0x1));
        assert_eq!(accepted(0x2, "a1-2.3"), "123");
        assert_eq!(accepted(0x3002, "-1.2.3x"), "-1.23");
        assert_eq!(accepted(0x3, "+1 (555) abc"), "+1 (555) ");
        assert_eq!(accepted(0x1, "any text"), "any text");
    }

    #[test]
    fn an_edit_is_reported_as_its_changed_span() {
        assert_eq!(changed_span("hello", "help"), (3, 2, 1));
        assert_eq!(changed_span("", "abc"), (0, 0, 3));
        assert_eq!(changed_span("aaa", "aaaa"), (3, 0, 1));
        assert_eq!(changed_span("same", "same"), (4, 0, 0));
    }
}
