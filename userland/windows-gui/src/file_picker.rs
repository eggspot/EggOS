//! EggOS's file picker behind `GetOpenFileNameW` and `GetSaveFileNameW` (section 7 of the
//! design): its fixed content size, where each of its controls sits, and its titles.

use crate::surface::Rect;

/// The picker's content size.
pub const CONTENT: (i32, i32) = (560, 400);
/// The `Up` push button.
pub const UP: Rect = Rect::new(16, 16, 80, 44);
/// The read-only edit showing the folder.
pub const FOLDER: Rect = Rect::new(88, 16, 544, 44);
/// The list of the folder's entries.
pub const ENTRIES: Rect = Rect::new(16, 56, 544, 298);
/// The `File name:` label.
pub const NAME_LABEL: Rect = Rect::new(16, 320, 96, 336);
/// The file name edit.
pub const NAME: Rect = Rect::new(104, 314, 544, 342);
/// The label showing the filter's description.
pub const FILTER: Rect = Rect::new(16, 362, 336, 378);
/// The `Cancel` push button.
pub const CANCEL: Rect = Rect::new(352, 356, 448, 384);
/// The `Open` or `Save` default push button.
pub const ACCEPT: Rect = Rect::new(448, 356, 544, 384);

pub const UP_LABEL: &str = "Up";
pub const NAME_LABEL_TEXT: &str = "File name:";
pub const CANCEL_LABEL: &str = "Cancel";
pub const OPEN_LABEL: &str = "Open";
pub const SAVE_LABEL: &str = "Save";
pub const OPEN_TITLE: &str = "Open";
pub const SAVE_TITLE: &str = "Save As";

/// Which picker it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Open,
    Save,
}

impl PickerKind {
    /// The title when the program gives none.
    pub fn title(self) -> &'static str {
        match self {
            PickerKind::Open => OPEN_TITLE,
            PickerKind::Save => SAVE_TITLE,
        }
    }

    /// The default button's label.
    pub fn accept_label(self) -> &'static str {
        match self {
            PickerKind::Open => OPEN_LABEL,
            PickerKind::Save => SAVE_LABEL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_control_sits_inside_the_content_and_the_list_shows_twelve_rows() {
        let content = Rect::new(0, 0, CONTENT.0, CONTENT.1);
        for rect in [
            UP, FOLDER, ENTRIES, NAME_LABEL, NAME, FILTER, CANCEL, ACCEPT,
        ] {
            assert_eq!(rect.intersection(&content), rect);
        }
        assert_eq!(
            crate::controls::list_box::visible_rows(ENTRIES.height()),
            12
        );
        assert_eq!(ENTRIES.height(), 242);
    }
}
