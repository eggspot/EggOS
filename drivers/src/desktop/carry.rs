//! Drag and drop between windows (D393, docs/ux/toolkit-v0.5.md section 7.2's later slice): a
//! window that holds the pointer with the left button down hands its in-window drag to the
//! server (`START_DRAG`), which carries it across windows: the ghost, the toolkit's tile with
//! the payload's name, drawn at the pointer above every window; `DragOver` and `DragLeave`
//! to the window under the pointer as it moves; on the release a `Drop` to that window (which
//! reads the payload with `GET_DRAG`) and a `DragEnded` to the source either way. Esc cancels.

use super::paint::{self, Canvas};
use super::protocol::{EventRecord, EventSink, WindowError, event};
use super::{Desktop, colour};
use crate::display::{Damage, Rect};
use crate::font::GLYPH_WIDTH;

/// The longest ghost text, in bytes of UTF-8.
pub const CARRY_TEXT_CAPACITY: usize = 64;
const GHOST_OFFSET: u32 = 12;
const GHOST_HEIGHT: u32 = 24;
const GHOST_TEXT_X: u32 = 8;
const GHOST_TEXT_Y: u32 = 4;
const GHOST_PADDING: u32 = 16;

/// A drag the server carries between windows.
#[derive(Debug, Clone, Copy)]
pub(super) struct Carry {
    source: usize,
    kind: u32,
    serial: u32,
    text: [u8; CARRY_TEXT_CAPACITY],
    length: usize,
    over: Option<usize>,
}

impl Desktop {
    /// `START_DRAG` (D393): client `client`'s window `id`, which holds the pointer with the
    /// left button down, hands its drag over, the payload of `kind` already kept as `serial`
    /// and `text` its ghost's words. Refused (`Invalid`) unless the window holds the pointer.
    pub fn start_carry(
        &mut self,
        client: u16,
        id: u32,
        (kind, serial): (u32, u32),
        text: &[u8],
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, id)?;
        if self.grab != Some(slot) || self.buttons & 1 == 0 || self.carried.is_some() {
            return Err(WindowError::Invalid);
        }
        let mut words = [0u8; CARRY_TEXT_CAPACITY];
        let mut length = text.len().min(CARRY_TEXT_CAPACITY);
        while core::str::from_utf8(&text[..length]).is_err() {
            length -= 1;
        }
        words[..length].copy_from_slice(&text[..length]);
        self.carried = Some(Carry {
            source: slot,
            kind,
            serial,
            text: words,
            length,
            over: None,
        });
        self.grab = None;
        let (x, y) = self.pointer;
        self.carry_motion(x, y, damage, events);
        Ok(())
    }

    /// Whether a drag between windows is on.
    pub fn carrying(&self) -> bool {
        self.carried.is_some()
    }

    fn carry_text(carry: &Carry) -> &str {
        core::str::from_utf8(&carry.text[..carry.length]).unwrap_or("")
    }

    fn ghost_rect(&self, carry: &Carry) -> Rect {
        let (x, y) = self.pointer;
        let glyphs = Self::carry_text(carry).chars().count() as u32;
        let width = glyphs * GLYPH_WIDTH as u32 + GHOST_PADDING;
        Rect::new(x + GHOST_OFFSET, y + GHOST_OFFSET, width, GHOST_HEIGHT)
    }

    fn carry_record(&self, kind: u16, slot: usize, carry: &Carry) -> EventRecord {
        let (cx, cy) = self.content_point(slot);
        EventRecord::new(kind, 0, cx, cy, carry.kind, carry.serial)
    }

    /// The pointer moved to (`x`, `y`) during a carried drag: the ghost follows, and the window
    /// under it hears `DragOver`, the one it left `DragLeave`.
    pub(super) fn carry_motion(
        &mut self,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let Some(mut carry) = self.carried else {
            return;
        };
        damage.add(self.ghost_rect(&carry));
        let under = self.top_window_at(x, y);
        if under != carry.over {
            if let Some(left) = carry.over {
                let record = self.carry_record(event::DRAG_LEAVE, left, &carry);
                self.send(left, record, events);
            }
            carry.over = under;
        }
        if let Some(over) = under {
            let record = self.carry_record(event::DRAG_OVER, over, &carry);
            self.send(over, record, events);
        }
        self.carried = Some(carry);
        damage.add(self.ghost_rect(&carry));
    }

    fn end_carry(&mut self, dropped: bool, damage: &mut Damage, events: &mut dyn EventSink) {
        let Some(carry) = self.carried.take() else {
            return;
        };
        damage.add(self.ghost_rect(&carry));
        match (dropped, carry.over) {
            (true, Some(over)) => {
                let record = self.carry_record(event::DROP, over, &carry);
                self.send(over, record, events);
            }
            (false, Some(over)) => {
                let record = self.carry_record(event::DRAG_LEAVE, over, &carry);
                self.send(over, record, events);
            }
            _ => {}
        }
        let landed = i32::from(dropped && carry.over.is_some());
        let record = EventRecord::new(event::DRAG_ENDED, 0, landed, 0, carry.kind, carry.serial);
        self.send(carry.source, record, events);
    }

    /// The left button went up during a carried drag: it drops on the window under the
    /// pointer, if any.
    pub(super) fn carry_release(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        self.end_carry(true, damage, events);
    }

    /// Esc, or the source going away, cancels a carried drag.
    pub(super) fn cancel_carry(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        self.end_carry(false, damage, events);
    }

    /// Forgets the window in `slot`, which is gone, from a carried drag: its own drag ends
    /// with no drop, and a window it was over is no longer.
    pub(super) fn forget_in_carry(
        &mut self,
        slot: usize,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let Some(mut carry) = self.carried else {
            return;
        };
        if carry.source == slot {
            self.carried = None;
            damage.add(self.ghost_rect(&carry));
            if let Some(over) = carry.over.filter(|&over| over != slot) {
                let record = self.carry_record(event::DRAG_LEAVE, over, &carry);
                self.send(over, record, events);
            }
            return;
        }
        if carry.over == Some(slot) {
            carry.over = None;
            self.carried = Some(carry);
        }
    }

    pub(super) fn carry_overlaps(&self, clip: Rect) -> bool {
        self.carried
            .is_some_and(|carry| self.ghost_rect(&carry).overlaps(&clip))
    }

    pub(super) fn draw_carry(&self, canvas: &mut Canvas<'_>) {
        let Some(carry) = self.carried else {
            return;
        };
        let ghost = self.ghost_rect(&carry);
        if !ghost.overlaps(&canvas.clip()) {
            return;
        }
        canvas.rounded(ghost, &paint::INSET_R4, true, true, colour::MENU_BORDER);
        canvas.rounded(
            Rect::new(ghost.x + 1, ghost.y + 1, ghost.width - 2, ghost.height - 2),
            &paint::INSET_INSIDE_R4,
            true,
            true,
            colour::MENU,
        );
        canvas.text(
            ghost.x + GHOST_TEXT_X,
            ghost.y + GHOST_TEXT_Y,
            Self::carry_text(&carry),
            colour::TEXT,
        );
    }
}
