use super::protocol::{DiscardEvents, EventRecord};
use super::*;
use alloc::vec::Vec;

const SOURCE: u16 = 3;
const TARGET: u16 = 4;

#[derive(Default)]
struct Recorded(Vec<(u16, EventRecord)>);

impl EventSink for Recorded {
    fn send(&mut self, client: u16, record: EventRecord) {
        self.0.push((client, record));
    }
}

impl Recorded {
    fn kinds_for(&self, client: u16) -> Vec<u16> {
        self.0
            .iter()
            .filter(|(to, _)| *to == client)
            .map(|(_, record)| record.kind)
            .collect()
    }
}

fn shown(desktop: &mut Desktop, client: u16, width: u32, height: u32) -> WindowId {
    let created = desktop.create(client, 0, 0, width, height, 0).unwrap();
    desktop
        .commit(client, u32::from(created.window), &[], &mut Damage::new(), &mut DiscardEvents)
        .unwrap();
    created.window
}

fn centre(desktop: &Desktop, id: WindowId) -> (u32, u32) {
    let slot = desktop.find(id).unwrap();
    let outside = desktop.window_at(slot).outside();
    (outside.x + outside.width / 2, outside.y + outside.height / 2)
}

fn set_up() -> (Desktop, WindowId, WindowId) {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let target = shown(&mut desktop, TARGET, 300, 200);
    let source = shown(&mut desktop, SOURCE, 300, 200);
    desktop.set_position(TARGET, u32::from(target), 0, 0, &mut Damage::new()).unwrap();
    desktop.set_position(SOURCE, u32::from(source), 500, 0, &mut Damage::new()).unwrap();
    let (x, y) = centre(&desktop, source);
    desktop.pointer_moved(x, y, &mut Damage::new(), &mut DiscardEvents);
    desktop.left_button(true, &mut Damage::new(), &mut DiscardEvents);
    (desktop, source, target)
}

#[test]
fn a_window_holding_the_pointer_hands_its_drag_to_the_server() {
    let (mut desktop, source, _) = set_up();
    let started = desktop.start_carry(SOURCE, u32::from(source), (1, 7), b"notes.txt", &mut Damage::new(), &mut DiscardEvents);
    assert_eq!(started, Ok(()));
    assert!(desktop.carrying());
}

#[test]
fn a_window_not_holding_the_pointer_cannot_start_a_drag() {
    let (mut desktop, _, target) = set_up();
    let started = desktop.start_carry(TARGET, u32::from(target), (1, 7), b"x", &mut Damage::new(), &mut DiscardEvents);
    assert_eq!(started, Err(WindowError::Invalid));
}

#[test]
fn the_target_hears_over_and_drop_and_the_source_that_it_landed() {
    let (mut desktop, source, target) = set_up();
    desktop
        .start_carry(SOURCE, u32::from(source), (1, 7), b"notes.txt", &mut Damage::new(), &mut DiscardEvents)
        .unwrap();
    let mut events = Recorded::default();
    let (x, y) = centre(&desktop, target);
    desktop.pointer_moved(x, y, &mut Damage::new(), &mut events);
    desktop.left_button(false, &mut Damage::new(), &mut events);
    assert!(events.kinds_for(TARGET).contains(&event::DRAG_OVER));
    assert!(events.kinds_for(TARGET).contains(&event::DROP));
    let ended = events
        .0
        .iter()
        .find(|(to, record)| *to == SOURCE && record.kind == event::DRAG_ENDED)
        .map(|(_, record)| record.a);
    assert_eq!(ended, Some(1));
    assert!(!desktop.carrying());
}

#[test]
fn esc_cancels_the_drag() {
    let (mut desktop, source, _) = set_up();
    desktop
        .start_carry(SOURCE, u32::from(source), (2, 8), b"text", &mut Damage::new(), &mut DiscardEvents)
        .unwrap();
    let mut events = Recorded::default();
    desktop.key(key::ESC, true, crate::input::keymap::Modifiers::NONE, &mut Damage::new(), &mut events);
    let ended = events
        .0
        .iter()
        .find(|(to, record)| *to == SOURCE && record.kind == event::DRAG_ENDED)
        .map(|(_, record)| record.a);
    assert_eq!(ended, Some(0));
    assert!(!desktop.carrying());
}
