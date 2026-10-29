//! The texts a moment is shown in (D150), written to any [`core::fmt::Write`].

use crate::{DateTime, MONTHS, WEEKDAYS};
use core::fmt::{self, Write};

/// `YYYY-MM-DD HH:MM:SS`: the kernel's boot line and `hwclock`.
pub fn iso(out: &mut impl Write, seconds: i64) -> fmt::Result {
    let moment = DateTime::from_unix(seconds);
    write!(
        out,
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        moment.year, moment.month, moment.day, moment.hour, moment.minute, moment.second
    )
}

/// `date`'s text with no format, as in the C locale: `Thu Oct 29 14:05:09 UTC 2026`.
pub fn date_default(out: &mut impl Write, seconds: i64) -> fmt::Result {
    let moment = DateTime::from_unix(seconds);
    write!(
        out,
        "{} {} {:2} {:02}:{:02}:{:02} UTC {}",
        &WEEKDAYS[moment.weekday()][..3],
        &MONTHS[moment.month as usize - 1][..3],
        moment.day,
        moment.hour,
        moment.minute,
        moment.second,
        moment.year
    )
}

/// Settings' `Now` line (docs/ux/date-time-v0.3.md section 3):
/// `Thursday 29 October 2026, 14:05:09 UTC`.
pub fn sentence(out: &mut impl Write, seconds: i64) -> fmt::Result {
    let moment = DateTime::from_unix(seconds);
    write!(
        out,
        "{} {} {} {}, {:02}:{:02}:{:02} UTC",
        WEEKDAYS[moment.weekday()],
        moment.day,
        MONTHS[moment.month as usize - 1],
        moment.year,
        moment.hour,
        moment.minute,
        moment.second
    )
}

/// `/proc/driver/rtc`'s text for a real-time clock reading `seconds`, in Linux's layout (its
/// `rtc-proc`): `rtc_time`, `rtc_date` and `24hr` lines, read back by
/// [`crate::parse_proc_driver_rtc`].
pub fn proc_driver_rtc(out: &mut impl Write, seconds: i64) -> fmt::Result {
    let moment = DateTime::from_unix(seconds);
    write!(
        out,
        "rtc_time\t: {:02}:{:02}:{:02}\nrtc_date\t: {:04}-{:02}-{:02}\n24hr\t\t: yes\n",
        moment.hour, moment.minute, moment.second, moment.year, moment.month, moment.day
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(write: fn(&mut String, i64) -> fmt::Result, seconds: i64) -> String {
        let mut out = String::new();
        write(&mut out, seconds).unwrap();
        out
    }

    #[test]
    fn each_text_has_its_documented_form() {
        assert_eq!(text(iso, 1_793_282_709), "2026-10-29 14:05:09");
        assert_eq!(text(iso, 0), "1970-01-01 00:00:00");
        assert_eq!(
            text(date_default, 1_793_282_709),
            "Thu Oct 29 14:05:09 UTC 2026"
        );
        assert_eq!(text(date_default, 0), "Thu Jan  1 00:00:00 UTC 1970");
        assert_eq!(
            text(sentence, 1_793_282_709),
            "Thursday 29 October 2026, 14:05:09 UTC"
        );
        assert_eq!(text(sentence, 12), "Thursday 1 January 1970, 00:00:12 UTC");
        let proc = text(proc_driver_rtc, 1_793_282_709);
        assert_eq!(
            proc,
            "rtc_time\t: 14:05:09\nrtc_date\t: 2026-10-29\n24hr\t\t: yes\n"
        );
        assert_eq!(
            crate::parse_proc_driver_rtc(proc.as_bytes()),
            Some(1_793_282_709)
        );
    }
}
