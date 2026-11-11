//! The shell's forms (docs/ux/adaptive-shell-v0.5.md section 1, D303): desktop, tablet, phone
//! and TV are one shell whose panels, window placement and focus follow the form. The form is
//! chosen from the screen's logical size and the kinds of input present, or forced by the
//! setting `shell.form`; a forced form that does not fit the screen is saved but not used.

/// A form the shell can take, or the full-screen console when none fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Form {
    /// The desktop's panels and free windows.
    #[default]
    Desktop,
    /// A dock, and windows full or split (slice 3; laid out as the desktop until then).
    Tablet,
    /// A status bar, a navigation bar, the home grid and one full window.
    Phone,
    /// Rows navigated by a remote (slice 4; laid out as the desktop until then).
    Tv,
    /// No shell: the full-screen console.
    Console,
}

impl Form {
    /// Its name, as `shell.form_in_use` says it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Tablet => "tablet",
            Self::Phone => "phone",
            Self::Tv => "tv",
            Self::Console => "console",
        }
    }

    /// Whether windows fill the work area without chrome and cannot be moved or sized.
    pub const fn fixes_windows(self) -> bool {
        matches!(self, Self::Phone | Self::Tv)
    }
}

/// The kinds of input device present (section 1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InputKinds {
    pub touch: bool,
    pub keyboard: bool,
    pub pointer: bool,
    pub remote: bool,
}

impl InputKinds {
    /// `input.present`: the kinds in order, separated by commas, or `none`.
    pub fn text(self, out: &mut [u8; 32]) -> usize {
        let mut length = 0;
        for (present, name) in [
            (self.touch, "touch"),
            (self.keyboard, "keyboard"),
            (self.pointer, "pointer"),
            (self.remote, "remote"),
        ] {
            if !present {
                continue;
            }
            if length > 0 {
                out[length] = b',';
                length += 1;
            }
            out[length..length + name.len()].copy_from_slice(name.as_bytes());
            length += name.len();
        }
        if length == 0 {
            out[..4].copy_from_slice(b"none");
            length = 4;
        }
        length
    }
}

/// The setting `shell.form`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormSetting {
    /// Chosen from the screen and the input (section 1.2).
    #[default]
    Auto,
    /// This form whatever the machine has, when it fits.
    Forced(Form),
}

/// The values `shell.form` takes, in the order an error names them.
pub const FORM_SETTING_NAMES: [&str; 5] = ["auto", "desktop", "tablet", "phone", "tv"];

impl FormSetting {
    /// The setting named `name`, if it is one of [`FORM_SETTING_NAMES`].
    pub fn named(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"auto" => Self::Auto,
            b"desktop" => Self::Forced(Form::Desktop),
            b"tablet" => Self::Forced(Form::Tablet),
            b"phone" => Self::Forced(Form::Phone),
            b"tv" => Self::Forced(Form::Tv),
            _ => return None,
        })
    }

    /// Its name in the settings file.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Forced(form) => form.name(),
        }
    }
}

/// The short side under which a touch screen is a phone.
pub const PHONE_SHORT_SIDE: u32 = 600;
/// The smallest TV screen, logical px.
pub const TV_SIZE: (u32, u32) = (960, 540);
/// The smallest phone screen, logical px.
pub const PHONE_SIZE: (u32, u32) = (320, 480);

/// The form `auto` picks for a `width` by `height` logical screen with `inputs` (section 1.2).
pub fn automatic(width: u32, height: u32, inputs: InputKinds) -> Form {
    let short = width.min(height);
    if inputs.touch {
        if short < PHONE_SIZE.0 || width.max(height) < PHONE_SIZE.1 {
            return Form::Console;
        }
        return match (short < PHONE_SHORT_SIDE, inputs.keyboard && inputs.pointer) {
            (true, _) => Form::Phone,
            (false, true) => Form::Desktop,
            (false, false) => Form::Tablet,
        };
    }
    if inputs.remote && !inputs.pointer && width >= TV_SIZE.0 && height >= TV_SIZE.1 {
        return Form::Tv;
    }
    Form::Desktop
}

/// Why a forced form is not used on this screen: the form used instead, and the size it
/// needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotFitting {
    pub shown: Form,
    pub needs: (u32, u32),
}

/// The form on screen for `setting` on a `width` by `height` logical screen with `inputs`: the
/// automatic one, or the forced one when it fits (section 1.3), else the one used instead and
/// why.
pub fn in_use(setting: FormSetting, width: u32, height: u32, inputs: InputKinds) -> (Form, Option<NotFitting>) {
    let automatic = automatic(width, height, inputs);
    let Some(forced) = (match setting {
        FormSetting::Auto => None,
        FormSetting::Forced(form) => Some(form),
    }) else {
        return (automatic, None);
    };
    let short = width.min(height);
    let refused = |shown: Form, needs: (u32, u32)| (shown, Some(NotFitting { shown, needs }));
    match forced {
        Form::Tablet if short < PHONE_SHORT_SIDE => refused(Form::Phone, (PHONE_SHORT_SIDE, PHONE_SHORT_SIDE)),
        Form::Tv if width < TV_SIZE.0 || height < TV_SIZE.1 => refused(Form::Desktop, TV_SIZE),
        Form::Phone if short < PHONE_SIZE.0 || width.max(height) < PHONE_SIZE.1 => {
            refused(automatic, PHONE_SIZE)
        }
        form => (form, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOUCH: InputKinds = InputKinds {
        touch: true,
        keyboard: false,
        pointer: false,
        remote: false,
    };

    #[test]
    fn auto_picks_the_design_s_rows() {
        let with = |keyboard, pointer, remote| InputKinds {
            touch: false,
            keyboard,
            pointer,
            remote,
        };
        assert_eq!(automatic(390, 844, TOUCH), Form::Phone);
        assert_eq!(automatic(844, 390, TOUCH), Form::Phone);
        assert_eq!(automatic(1024, 768, TOUCH), Form::Tablet);
        assert_eq!(automatic(768, 1024, TOUCH), Form::Tablet);
        assert_eq!(automatic(1024, 768, InputKinds { keyboard: true, ..TOUCH }), Form::Tablet);
        assert_eq!(
            automatic(1024, 768, InputKinds { keyboard: true, pointer: true, ..TOUCH }),
            Form::Desktop
        );
        assert_eq!(automatic(960, 540, with(false, false, true)), Form::Tv);
        assert_eq!(automatic(960, 540, with(false, true, true)), Form::Desktop);
        assert_eq!(automatic(1280, 800, with(true, true, false)), Form::Desktop);
        assert_eq!(automatic(599, 900, TOUCH), Form::Phone);
        assert_eq!(automatic(600, 900, TOUCH), Form::Tablet);
        assert_eq!(automatic(300, 400, TOUCH), Form::Console);
    }

    #[test]
    fn a_forced_form_that_does_not_fit_is_not_used() {
        let tablet = FormSetting::Forced(Form::Tablet);
        assert_eq!(in_use(tablet, 390, 844, TOUCH).0, Form::Phone);
        assert!(in_use(tablet, 390, 844, TOUCH).1.is_some());
        assert_eq!(in_use(FormSetting::Forced(Form::Phone), 390, 844, InputKinds::default()), (Form::Phone, None));
        assert_eq!(FormSetting::named(b"watch"), None);
        let mut text = [0u8; 32];
        let length = InputKinds { keyboard: true, pointer: true, ..InputKinds::default() }.text(&mut text);
        assert_eq!(&text[..length], b"keyboard,pointer");
    }
}
