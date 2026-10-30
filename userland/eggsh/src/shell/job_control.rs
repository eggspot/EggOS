//! The job builtins (D181): `jobs`, `fg`, `bg`, `wait` and `kill`, as bash has them for people
//! coming from Linux and macOS. `fg` and `bg` need job control, which an interactive shell
//! has; `jobs`, `wait` and `kill` work in scripts too.

use super::{Output, STANDARD_ERROR, Shell, System, USAGE};
use crate::jobs::{MAX_JOBS, NoSuchJob, State};
use crate::status::{Change, Ended};
use crate::syntax::{Words, decimal};

const SIGNAL_NAMES: [&[u8]; 31] = [
    b"HUP", b"INT", b"QUIT", b"ILL", b"TRAP", b"ABRT", b"BUS", b"FPE", b"KILL", b"USR1", b"SEGV",
    b"USR2", b"PIPE", b"ALRM", b"TERM", b"STKFLT", b"CHLD", b"CONT", b"STOP", b"TSTP", b"TTIN",
    b"TTOU", b"URG", b"XCPU", b"XFSZ", b"VTALRM", b"PROF", b"WINCH", b"IO", b"PWR", b"SYS",
];

const SIGKILL: u8 = 9;
const SIGTERM: u8 = 15;
const SIGCONT: u8 = 18;
const LAST_SIGNAL: u8 = 64;
const NOT_A_CHILD: i32 = 127;

/// The signal `name` names: a number from 0 to 64, or a name with or without `SIG`, in any
/// case (`TERM`, `sigterm`, `15`).
pub fn signal_number(name: &[u8]) -> Option<u8> {
    if !name.is_empty() && name.iter().all(u8::is_ascii_digit) {
        if name.len() > 2 {
            return None;
        }
        let number = name
            .iter()
            .fold(0u8, |value, &digit| value * 10 + (digit - b'0'));
        return (number <= LAST_SIGNAL).then_some(number);
    }
    let bare = match name.get(..3) {
        Some(prefix) if prefix.eq_ignore_ascii_case(b"SIG") => &name[3..],
        _ => name,
    };
    SIGNAL_NAMES
        .iter()
        .position(|known| known.eq_ignore_ascii_case(bare))
        .map(|index| index as u8 + 1)
}

fn process_id(word: &[u8]) -> Option<i32> {
    let (negative, digits) = match word {
        [b'-', rest @ ..] => (true, rest),
        _ => (false, word),
    };
    if digits.is_empty() || digits.len() > 10 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value = digits.iter().try_fold(0i32, |value, &digit| {
        value.checked_mul(10)?.checked_add(i32::from(digit - b'0'))
    })?;
    Some(if negative { -value } else { value })
}

impl<S: System> Shell<'_, S> {
    fn job_numbers(&self) -> ([u16; MAX_JOBS], usize) {
        let mut numbers = [0; MAX_JOBS];
        let mut count = 0;
        for job in self.jobs.iter() {
            numbers[count] = job.number();
            count += 1;
        }
        (numbers, count)
    }

    fn no_such_job(&mut self, builtin: &[u8], spec: Option<&[u8]>, why: NoSuchJob) {
        match (spec, why) {
            (Some(spec), NoSuchJob::Named) => {
                self.error(&[builtin, b": ", spec, b": no such job"]);
            }
            _ => self.error(&[builtin, b": current: no such job"]),
        }
    }

    fn job_for(&mut self, builtin: &[u8], words: &Words) -> Option<u16> {
        if !self.interactive {
            self.error(&[builtin, b": no job control"]);
            return None;
        }
        self.collect_job_changes();
        let spec = (words.command_length() > 1).then(|| words.command(1));
        match self.jobs.find(spec) {
            Ok(number) => Some(number),
            Err(why) => {
                self.no_such_job(builtin, spec, why);
                None
            }
        }
    }

    fn wait_for_job(&mut self, number: u16, foreground: bool) -> i32 {
        let mut programs = [0; super::MAX_PIPELINE];
        let mut count = 0;
        if let Some(job) = self.jobs.get(number) {
            for program in job.running_programs() {
                programs[count] = program;
                count += 1;
            }
        }
        for &program in &programs[..count] {
            let change = self
                .wait_for(program)
                .unwrap_or(Change::Ended(Ended::Exited(0)));
            self.jobs.record(program, change);
            if let Change::Stopped(_) = change {
                break;
            }
        }
        if foreground {
            self.system.take_back_terminal();
        }
        let Some(job) = self.jobs.get(number).copied() else {
            return 0;
        };
        match job.state() {
            State::Stopped(signal) => {
                if foreground {
                    self.system.write(STANDARD_ERROR, b"\n");
                }
                self.jobs.set_state(number, job.state());
                self.write_job(number, STANDARD_ERROR, false);
                128 + i32::from(signal)
            }
            State::Done(ended) => {
                if foreground {
                    let report = ended.report();
                    if !report.is_empty() {
                        self.system.write(STANDARD_ERROR, report);
                    }
                }
                self.jobs.remove(number);
                ended.shell_status()
            }
            State::Running => 0,
        }
    }

    /// `fg [job]`: gives the job the terminal, continues it if it is stopped and waits for it
    /// as for a command typed at the prompt; writes its command line first.
    pub(super) fn continue_in_foreground(&mut self, words: &Words) -> i32 {
        let Some(number) = self.job_for(b"fg", words) else {
            return 1;
        };
        let Some(job) = self.jobs.get(number).copied() else {
            return 1;
        };
        if let State::Done(_) = job.state() {
            self.error(&[b"fg: job has terminated"]);
            self.report_jobs();
            return 1;
        }
        self.system.give_terminal(job.group());
        {
            let mut output = Output::new(&mut self.system, self.output);
            output.push(job.text());
            output.push(b"\n");
        }
        if let State::Stopped(_) = job.state() {
            let _ = self.system.kill(-job.group(), SIGCONT);
        }
        self.jobs.set_state(number, State::Running);
        self.wait_for_job(number, true)
    }

    /// `bg [job]`: continues a stopped job in the background and writes `[n]+ command &`.
    pub(super) fn continue_in_background(&mut self, words: &Words) -> i32 {
        let Some(number) = self.job_for(b"bg", words) else {
            return 1;
        };
        let Some(job) = self.jobs.get(number).copied() else {
            return 1;
        };
        match job.state() {
            State::Done(_) => {
                self.error(&[b"bg: job has terminated"]);
                self.report_jobs();
                return 1;
            }
            State::Running => {
                let mut digits = [0; 20];
                let number = decimal(u64::from(number), &mut digits);
                self.error(&[b"bg: job ", number, b" already in background"]);
                return 0;
            }
            State::Stopped(_) => {
                let _ = self.system.kill(-job.group(), SIGCONT);
                self.jobs.set_state(number, State::Running);
            }
        }
        let mark = self.jobs.mark(number);
        let mut digits = [0; 20];
        let mut output = Output::new(&mut self.system, self.output);
        output.push(b"[");
        output.push(decimal(u64::from(number), &mut digits));
        output.push(b"]");
        output.push(mark);
        output.push(b" ");
        output.push(job.text());
        output.push(b" &\n");
        0
    }

    /// `jobs [-l | -p]`: one line a job, as bash writes them (`-l` adds the process ids,
    /// `-p` writes only them); jobs that are done are listed once, then forgotten.
    pub(super) fn list_jobs(&mut self, words: &Words) -> i32 {
        let mut program = false;
        let mut only_programs = false;
        for index in 1..words.command_length() {
            match words.command(index) {
                b"-l" => program = true,
                b"-p" => only_programs = true,
                option => {
                    self.error(&[b"jobs: ", option, b": invalid option"]);
                    self.error(&[b"jobs: usage: jobs [-l | -p]"]);
                    return USAGE;
                }
            }
        }
        self.collect_job_changes();
        let (numbers, count) = self.job_numbers();
        for &number in &numbers[..count] {
            let Some(job) = self.jobs.get(number).copied() else {
                continue;
            };
            if only_programs {
                let mut digits = [0; 20];
                let shown = job.last_program().or(job.running_programs().last());
                if let Some(shown) = shown {
                    let mut output = Output::new(&mut self.system, self.output);
                    output.push(decimal(shown as u64, &mut digits));
                    output.push(b"\n");
                }
            } else {
                self.write_job(number, self.output, program);
            }
            self.jobs.set_state(number, job.state());
        }
        self.jobs.remove_reported();
        0
    }

    /// `wait [pid | %job ...]`: waits for every job, or for each one named, and has the status
    /// of the last one named (127 for a process that is not a child of the shell).
    pub(super) fn wait(&mut self, words: &Words) -> i32 {
        self.collect_job_changes();
        if words.command_length() == 1 {
            let (numbers, count) = self.job_numbers();
            for &number in &numbers[..count] {
                if self
                    .jobs
                    .get(number)
                    .is_some_and(|job| job.state() == State::Running)
                {
                    self.wait_for_job(number, false);
                }
            }
            let (numbers, count) = self.job_numbers();
            for &number in &numbers[..count] {
                if self
                    .jobs
                    .get(number)
                    .is_some_and(|job| matches!(job.state(), State::Done(_)))
                {
                    self.jobs.remove(number);
                }
            }
            return 0;
        }
        let mut status = 0;
        for index in 1..words.command_length() {
            let word = words.command(index);
            let number = match word.first() {
                Some(b'%') => match self.jobs.find(Some(word)) {
                    Ok(number) => Some(number),
                    Err(why) => {
                        self.no_such_job(b"wait", Some(word), why);
                        status = NOT_A_CHILD;
                        continue;
                    }
                },
                _ => match process_id(word) {
                    Some(pid) => self.jobs.with_program(pid),
                    None => {
                        self.error(&[b"wait: `", word, b"': not a pid or valid job spec"]);
                        status = NOT_A_CHILD;
                        continue;
                    }
                },
            };
            let Some(number) = number else {
                self.error(&[b"wait: pid ", word, b" is not a child of this shell"]);
                status = NOT_A_CHILD;
                continue;
            };
            status = match self.jobs.get(number).map(|job| job.state()) {
                Some(State::Done(ended)) => {
                    self.jobs.remove(number);
                    ended.shell_status()
                }
                Some(State::Stopped(signal)) => 128 + i32::from(signal),
                Some(State::Running) => self.wait_for_job(number, false),
                None => NOT_A_CHILD,
            };
        }
        status
    }

    /// `kill [-s signal | -signal] pid | %job ...` and `kill -l`: sends the signal (`TERM`
    /// unless named) to each process, or to each job's process group; a stopped job is
    /// continued too, so it takes the signal, unless the signal is `KILL` or `CONT`.
    pub(super) fn kill(&mut self, words: &Words) -> i32 {
        let length = words.command_length();
        let mut signal = SIGTERM;
        let mut first = 1;
        if length > 1 {
            match words.command(1) {
                b"-l" | b"-L" => {
                    let mut output = Output::new(&mut self.system, self.output);
                    for (index, name) in SIGNAL_NAMES.iter().enumerate() {
                        if index > 0 {
                            output.push(b" ");
                        }
                        output.push(name);
                    }
                    output.push(b"\n");
                    return 0;
                }
                b"--" => first = 2,
                b"-s" if length > 2 => {
                    let Some(named) = signal_number(words.command(2)) else {
                        self.error(&[
                            b"kill: ",
                            words.command(2),
                            b": invalid signal specification",
                        ]);
                        return 1;
                    };
                    signal = named;
                    first = 3;
                }
                option if option.len() > 1 && option[0] == b'-' && process_id(option).is_none() => {
                    let Some(named) = signal_number(&option[1..]) else {
                        self.error(&[b"kill: ", &option[1..], b": invalid signal specification"]);
                        return 1;
                    };
                    signal = named;
                    first = 2;
                }
                option if option.len() > 1 && option[0] == b'-' => {
                    if let Some(named) = signal_number(&option[1..]) {
                        signal = named;
                        first = 2;
                    }
                }
                _ => {}
            }
        }
        if first > 1 && first < length && words.command(first) == b"--" {
            first += 1;
        }
        if first >= length {
            self.error(&[b"kill: usage: kill [-s signal | -signal] pid | %job ... or kill -l"]);
            return USAGE;
        }
        let mut status = 0;
        for index in first..length {
            let word = words.command(index);
            let reached = match word.first() {
                Some(b'%') => self.kill_job(word, signal),
                _ => match process_id(word) {
                    Some(pid) => self.kill_process(word, pid, signal),
                    None => {
                        self.error(&[b"kill: ", word, b": arguments must be process or job IDs"]);
                        false
                    }
                },
            };
            if !reached {
                status = 1;
            }
        }
        status
    }

    fn kill_process(&mut self, word: &[u8], pid: i32, signal: u8) -> bool {
        match self.system.kill(pid, signal) {
            Ok(()) => true,
            Err(errno) => {
                self.error(&[b"kill: (", word, b") - ", errno.message()]);
                false
            }
        }
    }

    fn kill_job(&mut self, spec: &[u8], signal: u8) -> bool {
        let number = match self.jobs.find(Some(spec)) {
            Ok(number) => number,
            Err(why) => {
                self.no_such_job(b"kill", Some(spec), why);
                return false;
            }
        };
        let Some(job) = self.jobs.get(number).copied() else {
            return false;
        };
        let mut reached = true;
        let stopped = matches!(job.state(), State::Stopped(_));
        let continued = stopped && signal != SIGKILL && signal != SIGCONT;
        if job.group() != 0 {
            reached = self.kill_process(spec, -job.group(), signal);
            if reached && continued {
                let _ = self.system.kill(-job.group(), SIGCONT);
            }
        } else {
            let mut programs = [0; super::MAX_PIPELINE];
            let mut count = 0;
            for program in job.running_programs() {
                programs[count] = program;
                count += 1;
            }
            for &program in &programs[..count] {
                reached &= self.kill_process(spec, program, signal);
                if continued {
                    let _ = self.system.kill(program, SIGCONT);
                }
            }
        }
        reached
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_are_named_as_kill_names_them() {
        assert_eq!(signal_number(b"TERM"), Some(15));
        assert_eq!(signal_number(b"SIGKILL"), Some(9));
        assert_eq!(signal_number(b"sigcont"), Some(18));
        assert_eq!(signal_number(b"tstp"), Some(20));
        assert_eq!(signal_number(b"WINCH"), Some(28));
        assert_eq!(signal_number(b"0"), Some(0));
        assert_eq!(signal_number(b"64"), Some(64));
        assert_eq!(signal_number(b"65"), None);
        assert_eq!(signal_number(b"100"), None);
        assert_eq!(signal_number(b"NOPE"), None);
        assert_eq!(signal_number(b""), None);
        assert_eq!(SIGNAL_NAMES[usize::from(SIGTERM) - 1], b"TERM");
        assert_eq!(SIGNAL_NAMES[usize::from(SIGCONT) - 1], b"CONT");
    }

    #[test]
    fn process_ids_are_decimal_and_may_name_a_group() {
        assert_eq!(process_id(b"42"), Some(42));
        assert_eq!(process_id(b"-42"), Some(-42));
        assert_eq!(process_id(b"x"), None);
        assert_eq!(process_id(b"-"), None);
        assert_eq!(process_id(b"99999999999"), None);
    }
}
