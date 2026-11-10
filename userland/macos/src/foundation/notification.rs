//! NSNotification and NSNotificationCenter (D232): a notification is a name, an object and a
//! user info dictionary; the default center keeps its observations, each in its own heap block
//! so a selector observer can be held weakly (an observer that is freed without removing
//! itself is skipped and forgotten, as macOS has done since 10.11), and a block observer by
//! the token it returns. Posting copies the matching observations out under the center's lock
//! and delivers outside it, in the order they were added, so an observer may add or remove
//! observers, or post, while it is told.

use super::{
    Definition, NSNOTIFICATION, NSNOTIFICATION_META, NSNOTIFICATIONCENTER,
    NSNOTIFICATIONCENTER_META, autoreleased, class_pointer, define as define_class, new_instance,
};
use crate::libsystem::{Exclusive, heap};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::block::invoke_address;
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::weak::{objc_destroy_weak, objc_init_weak, objc_load_weak_retained};
use crate::objc::{Runtime, methods, sel, send, send1, send_bool};
use core::sync::atomic::{AtomicUsize, Ordering};

#[repr(C)]
#[derive(Debug)]
struct NotificationObject {
    isa: usize,
    name: Id,
    object: Id,
    user_info: Id,
}

fn fields(object: Id) -> &'static mut NotificationObject {
    // SAFETY: the callers pass NSNotification instances, whose memory starts with these
    // fields.
    unsafe { &mut *object.cast::<NotificationObject>() }
}

#[repr(C)]
#[derive(Debug)]
struct Observation {
    observer: Id,
    selector: usize,
    name: Id,
    sender: Id,
    block: Id,
    token: Id,
}

#[derive(Debug, Clone, Copy)]
struct Delivery {
    observer: Id,
    selector: usize,
    name: Id,
    block: Id,
}

static OBSERVATIONS: Exclusive<Buffer<*mut Observation>> = Exclusive::new(Buffer::new());
static DEFAULT_CENTER: AtomicUsize = AtomicUsize::new(0);

fn retained(object: Id) -> Id {
    match object.is_null() {
        true => object,
        false => objc_retain(object),
    }
}

fn release(object: Id) {
    if !object.is_null() {
        objc_release(object);
    }
}

fn copied(object: Id) -> Id {
    match object.is_null() {
        true => object,
        false => send(object, sel!("copy")),
    }
}

fn new_notification(class: *mut ClassObject, name: Id, object: Id, user_info: Id) -> Id {
    let notification = new_instance(class);
    if notification.is_null() {
        return notification;
    }
    let fields = fields(notification);
    fields.name = copied(name);
    fields.object = retained(object);
    fields.user_info = copied(user_info);
    notification
}

/// A new autoreleased notification named `name` about `object`, with `user_info`.
pub fn notification(name: Id, object: Id, user_info: Id) -> Id {
    autoreleased(new_notification(
        class_pointer(&NSNOTIFICATION),
        name,
        object,
        user_info,
    ))
}

fn names_match(wanted: Id, posted: Id) -> bool {
    wanted.is_null()
        || wanted == posted
        || (!posted.is_null() && send_bool(wanted, sel!("isEqual:"), posted as usize))
}

fn observer_of(observation: *mut Observation) -> Id {
    // SAFETY: a registered observation's block, whose `observer` is a weak variable the center
    // initialized.
    unsafe { objc_load_weak_retained(&raw mut (*observation).observer) }
}

fn deliveries_for(sender: Id) -> Buffer<Delivery> {
    OBSERVATIONS.with(|observations| {
        let mut deliveries = Buffer::new();
        for &observation in observations.as_slice() {
            // SAFETY: a registered observation, freed only after it leaves the list.
            let entry = unsafe { &*observation };
            if !entry.sender.is_null() && entry.sender != sender {
                continue;
            }
            let observer = match entry.block.is_null() {
                true => observer_of(observation),
                false => core::ptr::null_mut(),
            };
            if entry.block.is_null() && observer.is_null() {
                continue;
            }
            let delivery = Delivery {
                observer,
                selector: entry.selector,
                name: retained(entry.name),
                block: retained(entry.block),
            };
            if !deliveries.push(delivery) {
                release(delivery.observer);
                release(delivery.name);
                release(delivery.block);
            }
        }
        deliveries
    })
}

/// Tells every observer of `notification`'s name and object, in the order they were added.
pub fn post(notification: Id) {
    if notification.is_null() {
        return;
    }
    let (name, sender) = (fields(notification).name, fields(notification).object);
    let mut deliveries = deliveries_for(sender);
    for delivery in deliveries.as_slice() {
        if names_match(delivery.name, name) {
            if !delivery.block.is_null() {
                // SAFETY: an observer block takes the block and the notification.
                let function: extern "C" fn(Id, Id) =
                    unsafe { core::mem::transmute(invoke_address(delivery.block)) };
                function(delivery.block, notification);
            } else {
                send1(
                    delivery.observer,
                    delivery.selector as Sel,
                    notification as usize,
                );
            }
        }
        release(delivery.observer);
        release(delivery.name);
        release(delivery.block);
    }
    deliveries.free();
}

/// Posts a new notification named `name` about `object` with `user_info`.
fn post_named(name: Id, object: Id, user_info: Id) -> Id {
    let made = notification(name, object, user_info);
    post(made);
    made
}

fn new_observation(entry: Observation) -> *mut Observation {
    let block = heap::allocate(core::mem::size_of::<Observation>(), true).cast::<Observation>();
    if block.is_null() {
        return block;
    }
    let observer = entry.observer;
    // SAFETY: a new block the size of an observation, not yet shared.
    unsafe {
        block.write(Observation {
            observer: core::ptr::null_mut(),
            ..entry
        })
    };
    if !observer.is_null() {
        // SAFETY: the new observation's weak variable, initialized once here.
        unsafe { objc_init_weak(&raw mut (*block).observer, observer) };
    }
    block
}

fn free_observation(observation: *mut Observation) {
    // SAFETY: an observation that has left the list, which only this thread now holds.
    let entry = unsafe { &mut *observation };
    if entry.block.is_null() {
        // SAFETY: its weak variable, initialized when it was made.
        unsafe { objc_destroy_weak(&raw mut entry.observer) };
    }
    release(entry.name);
    release(entry.block);
    release(entry.token);
    heap::release(observation.cast());
}

fn register(entry: Observation) -> bool {
    let observation = new_observation(entry);
    if observation.is_null() {
        return false;
    }
    if OBSERVATIONS.with(|observations| observations.push(observation)) {
        return true;
    }
    free_observation(observation);
    false
}

fn remove_where(mut matches: impl FnMut(&Observation, Id) -> bool) {
    let mut removed = OBSERVATIONS.with(|observations| {
        let mut removed = Buffer::new();
        let mut kept = 0;
        let mut index = 0;
        let list = observations.as_mut_slice();
        while let Some(&observation) = list.get(index) {
            index += 1;
            // SAFETY: a registered observation.
            let entry = unsafe { &*observation };
            let observer = entry.observer;
            let dead = entry.block.is_null() && observer.is_null();
            if (dead || matches(entry, observer)) && removed.push(observation) {
                continue;
            }
            list[kept] = observation;
            kept += 1;
        }
        while observations.len() > kept {
            observations.pop();
        }
        removed
    });
    for &observation in removed.as_slice() {
        free_observation(observation);
    }
    removed.free();
}

extern "C" fn notification_with_name(this: Id, _: Sel, name: Id, object: Id) -> Id {
    autoreleased(new_notification(
        this.cast(),
        name,
        object,
        core::ptr::null_mut(),
    ))
}

extern "C" fn notification_with_name_user_info(
    this: Id,
    _: Sel,
    name: Id,
    object: Id,
    user_info: Id,
) -> Id {
    autoreleased(new_notification(this.cast(), name, object, user_info))
}

extern "C" fn init_with_name(this: Id, _: Sel, name: Id, object: Id, user_info: Id) -> Id {
    let fields = fields(this);
    fields.name = copied(name);
    fields.object = retained(object);
    fields.user_info = copied(user_info);
    this
}

extern "C" fn name(this: Id, _: Sel) -> Id {
    fields(this).name
}

extern "C" fn object(this: Id, _: Sel) -> Id {
    fields(this).object
}

extern "C" fn user_info(this: Id, _: Sel) -> Id {
    fields(this).user_info
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    objc_retain(this)
}

extern "C" fn notification_dealloc(this: Id, _: Sel) {
    let fields = fields(this);
    for object in [fields.name, fields.object, fields.user_info] {
        release(object);
    }
    dispose(this);
}

extern "C" fn default_center(_: Id, _: Sel) -> Id {
    let existing = DEFAULT_CENTER.load(Ordering::Acquire);
    if existing != 0 {
        return existing as Id;
    }
    let made = new_instance(class_pointer(&NSNOTIFICATIONCENTER));
    match DEFAULT_CENTER.compare_exchange(0, made as usize, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => made,
        Err(winner) => {
            dispose(made);
            winner as Id
        }
    }
}

extern "C" fn add_observer(_: Id, _: Sel, observer: Id, selector: Sel, name: Id, sender: Id) {
    if observer.is_null() || selector.is_null() {
        return;
    }
    register(Observation {
        observer,
        selector: selector as usize,
        name: copied(name),
        sender,
        block: core::ptr::null_mut(),
        token: core::ptr::null_mut(),
    });
}

extern "C" fn add_observer_for_name(
    _: Id,
    _: Sel,
    name: Id,
    sender: Id,
    _queue: Id,
    block: Id,
) -> Id {
    if block.is_null() {
        return core::ptr::null_mut();
    }
    let token = new_instance(class_pointer(&NSOBJECT));
    if token.is_null() {
        return token;
    }
    let registered = register(Observation {
        observer: core::ptr::null_mut(),
        selector: 0,
        name: copied(name),
        sender,
        block: send(block, sel!("copy")),
        token: objc_retain(token),
    });
    autoreleased(match registered {
        true => token,
        false => {
            objc_release(token);
            core::ptr::null_mut()
        }
    })
}

extern "C" fn post_notification(_: Id, _: Sel, notification: Id) {
    post(notification);
}

extern "C" fn post_notification_name(_: Id, _: Sel, name: Id, sender: Id) {
    post_named(name, sender, core::ptr::null_mut());
}

extern "C" fn post_notification_name_user_info(
    _: Id,
    _: Sel,
    name: Id,
    sender: Id,
    user_info: Id,
) {
    post_named(name, sender, user_info);
}

fn is_observer(entry: &Observation, observer: Id, wanted: Id) -> bool {
    !wanted.is_null() && (entry.token == wanted || (entry.block.is_null() && observer == wanted))
}

extern "C" fn remove_observer(_: Id, _: Sel, wanted: Id) {
    remove_where(|entry, observer| is_observer(entry, observer, wanted));
}

extern "C" fn remove_observer_name_object(_: Id, _: Sel, wanted: Id, name: Id, sender: Id) {
    remove_where(|entry, observer| {
        is_observer(entry, observer, wanted)
            && (name.is_null() || (!entry.name.is_null() && names_match(entry.name, name)))
            && (sender.is_null() || entry.sender == sender)
    });
}

extern "C" fn this_object(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn nothing(_: Id, _: Sel) {}

pub fn define(runtime: &mut Runtime) -> bool {
    let notification_instance = methods![
        "initWithName:object:userInfo:" => init_with_name,
        "name" => name,
        "object" => object,
        "userInfo" => user_info,
        "copyWithZone:" => copy_with_zone,
        "dealloc" => notification_dealloc,
    ];
    let notification_class = methods![
        "notificationWithName:object:" => notification_with_name,
        "notificationWithName:object:userInfo:" => notification_with_name_user_info,
    ];
    let center_instance = methods![
        "addObserver:selector:name:object:" => add_observer,
        "addObserverForName:object:queue:usingBlock:" => add_observer_for_name,
        "postNotification:" => post_notification,
        "postNotificationName:object:" => post_notification_name,
        "postNotificationName:object:userInfo:" => post_notification_name_user_info,
        "removeObserver:" => remove_observer,
        "removeObserver:name:object:" => remove_observer_name_object,
        "retain" => this_object,
        "release" => nothing,
        "autorelease" => this_object,
    ];
    let center_class = methods!["defaultCenter" => default_center];
    define_class(
        runtime,
        &Definition {
            name: b"NSNotification\0",
            class: &NSNOTIFICATION,
            meta: &NSNOTIFICATION_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<NotificationObject>(),
            flags: 0,
            instance_methods: &notification_instance,
            class_methods: &notification_class,
        },
    ) && define_class(
        runtime,
        &Definition {
            name: b"NSNotificationCenter\0",
            class: &NSNOTIFICATIONCENTER,
            meta: &NSNOTIFICATIONCENTER_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<usize>(),
            flags: 0,
            instance_methods: &center_instance,
            class_methods: &center_class,
        },
    )
}
