//! Jobs (D181): the pipelines the shell runs in the background (`&`) and those a stop signal
//! stopped (`^Z`), each a number from 1, its process group, its programs' process ids and how
//! each ended, and the command line as typed, for `jobs`, `fg`, `bg`, `wait` and `kill` and the
//! reports the shell writes before a prompt. The current job (`%+`, `%%`, `fg` alone) is the
//! one stopped or started last, the previous one (`%-`) the one before it. Pure logic, with
//! host tests; fixed room, no allocator.

use crate::status::{Change, Ended, stopped_state};

/// The most jobs the shell keeps at once.
pub const MAX_JOBS: usize = 16;

/// The most programs a job has: a pipeline's.
pub const MAX_PROGRAMS: usize = 16;

/// The most bytes of a job's command line kept for its reports.
pub const TEXT_CAPACITY: usize = 96;

/// Where a job stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Its programs run.
    Running,
    /// This signal stopped it.
    Stopped(u8),
    /// Its programs ended; the last one as this says.
    Done(Ended),
}

/// One job.
#[derive(Debug, Clone, Copy)]
pub struct Job {
    number: u16,
    group: i32,
    programs: [i32; MAX_PROGRAMS],
    count: usize,
    ended: u16,
    last: Option<Ended>,
    last_program: Option<i32>,
    state: State,
    changed: bool,
    text: [u8; TEXT_CAPACITY],
    text_length: usize,
}

impl Job {
    /// Its number, from 1.
    pub fn number(&self) -> u16 {
        self.number
    }

    /// Its process group, which its programs lead or join; 0 without job control.
    pub fn group(&self) -> i32 {
        self.group
    }

    /// Where it stands.
    pub fn state(&self) -> State {
        self.state
    }

    /// Its command line as typed.
    pub fn text(&self) -> &[u8] {
        &self.text[..self.text_length]
    }

    /// The process id of its last program, if that is a program: what `$!` and `jobs -p`
    /// give.
    pub fn last_program(&self) -> Option<i32> {
        self.last_program
    }

    /// Its programs that have not ended yet.
    pub fn running_programs(&self) -> impl Iterator<Item = i32> + '_ {
        self.programs[..self.count]
            .iter()
            .enumerate()
            .filter(|&(index, _)| self.ended & (1 << index) == 0)
            .map(|(_, &program)| program)
    }

    fn has(&self, program: i32) -> Option<usize> {
        self.programs[..self.count]
            .iter()
            .position(|&known| known == program)
    }

    fn record(&mut self, index: usize, change: Change) -> bool {
        match change {
            Change::Ended(ended) => {
                self.ended |= 1 << index;
                if Some(self.programs[index]) == self.last_program {
                    self.last = Some(ended);
                }
                if self.running_programs().next().is_none() {
                    self.state = State::Done(self.last.unwrap_or(Ended::Exited(0)));
                    return true;
                }
                false
            }
            Change::Stopped(signal) => {
                let changed = !matches!(self.state, State::Stopped(_));
                self.state = State::Stopped(signal);
                changed
            }
            Change::Continued => {
                let changed = matches!(self.state, State::Stopped(_));
                if changed {
                    self.state = State::Running;
                }
                changed
            }
        }
    }
}

/// A new job: its process group, its programs and how each that ended already ended (the
/// shell waited for them), the last of the pipeline's commands (`Ok` with its program, or
/// `Err` with a builtin's status), its command line and where it stands.
#[derive(Debug, Clone, Copy)]
pub struct NewJob<'a> {
    pub group: i32,
    pub programs: &'a [i32],
    pub ended: u16,
    pub last: Result<i32, Ended>,
    pub text: &'a [u8],
    pub state: State,
}

/// The table is full: the shell keeps at most [`MAX_JOBS`] jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

/// Why a job specification names no job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoSuchJob {
    /// None was given and there is no current job.
    NoCurrent,
    /// The one given names none.
    Named,
}

/// The shell's jobs: see the module documentation.
#[derive(Debug)]
pub struct Jobs {
    slots: [Option<Job>; MAX_JOBS],
    current: Option<u16>,
    previous: Option<u16>,
}

impl Default for Jobs {
    fn default() -> Self {
        Self {
            slots: [None; MAX_JOBS],
            current: None,
            previous: None,
        }
    }
}

impl Jobs {
    /// Whether it holds no job.
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// Whether a job is stopped.
    pub fn any_stopped(&self) -> bool {
        self.iter()
            .any(|job| matches!(job.state, State::Stopped(_)))
    }

    /// Its jobs, by number.
    pub fn iter(&self) -> impl Iterator<Item = &Job> {
        let mut jobs: [Option<&Job>; MAX_JOBS] = [None; MAX_JOBS];
        for (count, job) in self.slots.iter().flatten().enumerate() {
            let at = jobs[..count]
                .iter()
                .position(|other| other.is_some_and(|other| other.number > job.number))
                .unwrap_or(count);
            jobs.copy_within(at..count, at + 1);
            jobs[at] = Some(job);
        }
        jobs.into_iter().flatten()
    }

    /// Job `number`.
    pub fn get(&self, number: u16) -> Option<&Job> {
        self.slots.iter().flatten().find(|job| job.number == number)
    }

    fn get_mut(&mut self, number: u16) -> Option<&mut Job> {
        self.slots
            .iter_mut()
            .flatten()
            .find(|job| job.number == number)
    }

    /// Adds `new` as the current job and returns its number, one more than the highest in
    /// use. Without room, a job that is done makes it, one already reported first. [`Full`]
    /// if every job runs or is stopped.
    pub fn add(&mut self, new: NewJob<'_>) -> Result<u16, Full> {
        let done = |reported: bool| {
            move |slot: &Option<Job>| {
                slot.is_some_and(|job| {
                    matches!(job.state, State::Done(_)) && (!reported || !job.changed)
                })
            }
        };
        let free = self
            .slots
            .iter()
            .position(Option::is_none)
            .or_else(|| self.slots.iter().position(done(true)))
            .or_else(|| self.slots.iter().position(done(false)))
            .ok_or(Full)?;
        if let Some(old) = self.slots[free].take() {
            self.forget(old.number);
        }
        let number = self.iter().map(|job| job.number).max().unwrap_or(0) + 1;
        let count = new.programs.len().min(MAX_PROGRAMS);
        let mut programs = [0; MAX_PROGRAMS];
        programs[..count].copy_from_slice(&new.programs[..count]);
        let text = trimmed(new.text);
        let text_length = text.len().min(TEXT_CAPACITY);
        let mut kept = [0; TEXT_CAPACITY];
        kept[..text_length].copy_from_slice(&text[..text_length]);
        let (last_program, last) = match new.last {
            Ok(program) => (Some(program), None),
            Err(ended) => (None, Some(ended)),
        };
        let mut job = Job {
            number,
            group: new.group,
            programs,
            count,
            ended: new.ended,
            last,
            last_program,
            state: new.state,
            changed: false,
            text: kept,
            text_length,
        };
        if job.running_programs().next().is_none() {
            job.state = State::Done(job.last.unwrap_or(Ended::Exited(0)));
            job.changed = true;
        }
        self.slots[free] = Some(job);
        self.make_current(number);
        Ok(number)
    }

    fn make_current(&mut self, number: u16) {
        if self.current != Some(number) {
            self.previous = self.current;
            self.current = Some(number);
        }
    }

    /// Records `change` of program `program`, if it is one of a job's, and returns that job's
    /// number. A job that stops becomes the current one; its change waits to be reported
    /// ([`Self::take_changed`]), but a continuation, which is not reported.
    pub fn record(&mut self, program: i32, change: Change) -> Option<u16> {
        let job = self
            .slots
            .iter_mut()
            .flatten()
            .find(|job| job.has(program).is_some())?;
        let index = job.has(program)?;
        let number = job.number;
        if job.record(index, change) {
            job.changed = !matches!(change, Change::Continued);
            if matches!(change, Change::Stopped(_)) {
                self.make_current(number);
            }
        }
        Some(number)
    }

    /// Sets job `number`'s state, as `fg` and `bg` do when they continue it, and says it was
    /// reported.
    pub fn set_state(&mut self, number: u16, state: State) {
        if let Some(job) = self.get_mut(number) {
            job.state = state;
            job.changed = false;
        }
    }

    /// The first job whose change is not reported yet, which counts as reported from now on.
    pub fn take_changed(&mut self) -> Option<Job> {
        let number = self.iter().find(|job| job.changed).map(|job| job.number)?;
        let job = self.get_mut(number)?;
        job.changed = false;
        Some(*job)
    }

    /// Removes job `number`.
    pub fn remove(&mut self, number: u16) {
        for slot in &mut self.slots {
            if slot.is_some_and(|job| job.number == number) {
                *slot = None;
            }
        }
        self.forget(number);
    }

    fn forget(&mut self, number: u16) {
        if self.previous == Some(number) {
            self.previous = None;
        }
        if self.current == Some(number) {
            self.current = self.previous.take();
        }
        if self.current.is_none() {
            self.current = self.newest_but(None);
        }
        if self.previous.is_none() {
            self.previous = self.newest_but(self.current);
        }
    }

    fn newest_but(&self, except: Option<u16>) -> Option<u16> {
        self.iter()
            .map(|job| job.number)
            .filter(|&number| Some(number) != except)
            .max()
    }

    /// Removes every job that is done and was reported.
    pub fn remove_reported(&mut self) {
        let done: [Option<u16>; MAX_JOBS] = core::array::from_fn(|index| {
            self.slots[index]
                .filter(|job| matches!(job.state, State::Done(_)) && !job.changed)
                .map(|job| job.number)
        });
        for number in done.into_iter().flatten() {
            self.remove(number);
        }
    }

    /// The job `spec` names (`fg`, `bg`, `wait`, `kill`): none, `%%`, `%+` or `%` the current
    /// one, `%-` the previous one, `%n` or `n` job `n`, `%name` the newest whose command line
    /// starts with `name`.
    pub fn find(&self, spec: Option<&[u8]>) -> Result<u16, NoSuchJob> {
        let spec = match spec {
            None | Some(b"%" | b"%%" | b"%+") => {
                return self.current.ok_or(NoSuchJob::NoCurrent);
            }
            Some(b"%-") => return self.previous.ok_or(NoSuchJob::Named),
            Some(spec) => spec.strip_prefix(b"%").unwrap_or(spec),
        };
        if let Some(number) = number(spec) {
            return self
                .get(number)
                .map(|job| job.number)
                .ok_or(NoSuchJob::Named);
        }
        self.iter()
            .filter(|job| job.text().starts_with(spec))
            .map(|job| job.number)
            .max()
            .ok_or(NoSuchJob::Named)
    }

    /// What job `number`'s reports write after its number: `+` for the current job, `-` for
    /// the previous one, a space for the others.
    pub fn mark(&self, number: u16) -> &'static [u8] {
        match Some(number) {
            number if number == self.current => b"+",
            number if number == self.previous => b"-",
            _ => b" ",
        }
    }

    /// The job that has program `program`.
    pub fn with_program(&self, program: i32) -> Option<u16> {
        self.iter()
            .find(|job| job.has(program).is_some())
            .map(|job| job.number)
    }

    /// Writes job `job`'s line as `jobs` and the reports show it, as bash does:
    /// `[1]+  Stopped                 sleep 30`, ` &` after a running job's command line; with
    /// `program`, the job's last program's process id after the mark.
    pub fn write_line(&self, job: &Job, program: bool, out: &mut impl FnMut(&[u8])) {
        let mut digits = [0; 20];
        out(b"[");
        out(crate::syntax::decimal(u64::from(job.number), &mut digits));
        out(b"]");
        out(self.mark(job.number));
        let shown = job
            .last_program
            .or(job.programs[..job.count].last().copied())
            .filter(|_| program);
        match shown {
            Some(last) => {
                out(b" ");
                out(crate::syntax::decimal(last as u64, &mut digits));
                out(b" ");
            }
            None => out(b"  "),
        }
        let mut state = [0; 24];
        let state: &[u8] = match job.state {
            State::Running => b"Running",
            State::Stopped(signal) => stopped_state(signal),
            State::Done(ended) => ended.job_state(&mut state),
        };
        out(state);
        for _ in state.len()..STATE_COLUMN {
            out(b" ");
        }
        out(job.text());
        if job.state == State::Running {
            out(b" &");
        }
        out(b"\n");
    }
}

const STATE_COLUMN: usize = 24;

fn number(text: &[u8]) -> Option<u16> {
    if text.is_empty() || text.len() > 5 || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value = text
        .iter()
        .fold(0u32, |value, &digit| value * 10 + u32::from(digit - b'0'));
    u16::try_from(value).ok().filter(|&value| value > 0)
}

fn trimmed(text: &[u8]) -> &[u8] {
    let start = text
        .iter()
        .position(|&byte| !matches!(byte, b' ' | b'\t' | b'\n'))
        .unwrap_or(text.len());
    let end = text
        .iter()
        .rposition(|&byte| !matches!(byte, b' ' | b'\t' | b'\n'))
        .map_or(start, |at| at + 1);
    &text[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn background(jobs: &mut Jobs, programs: &[i32], text: &str) -> u16 {
        jobs.add(NewJob {
            group: programs[0],
            programs,
            ended: 0,
            last: Ok(*programs.last().unwrap()),
            text: text.as_bytes(),
            state: State::Running,
        })
        .unwrap()
    }

    fn line(jobs: &Jobs, number: u16, program: bool) -> String {
        let mut written = Vec::new();
        jobs.write_line(jobs.get(number).unwrap(), program, &mut |bytes| {
            written.extend_from_slice(bytes)
        });
        String::from_utf8(written).unwrap()
    }

    #[test]
    fn jobs_are_numbered_from_one_and_listed_as_bash_lists_them() {
        let mut jobs = Jobs::default();
        assert!(jobs.is_empty());
        assert_eq!(background(&mut jobs, &[10, 11], "  sleep 30 | cat \n"), 1);
        assert_eq!(background(&mut jobs, &[20], "sleep 40"), 2);
        assert_eq!(
            line(&jobs, 1, false),
            "[1]-  Running                 sleep 30 | cat &\n"
        );
        assert_eq!(
            line(&jobs, 2, true),
            "[2]+ 20 Running                 sleep 40 &\n"
        );
        assert_eq!(jobs.record(20, Change::Stopped(20)), Some(2));
        assert_eq!(
            line(&jobs, 2, false),
            "[2]+  Stopped                 sleep 40\n"
        );
        assert!(jobs.any_stopped());
        assert_eq!(jobs.take_changed().map(|job| job.number()), Some(2));
        assert!(jobs.take_changed().is_none());
        assert_eq!(jobs.record(10, Change::Ended(Ended::Exited(0))), Some(1));
        assert!(jobs.take_changed().is_none(), "one of two programs ended");
        jobs.record(11, Change::Ended(Ended::Exited(3)));
        assert_eq!(
            line(&jobs, 1, false),
            "[1]-  Exit 3                  sleep 30 | cat\n"
        );
        let done = jobs.take_changed().unwrap();
        assert_eq!(done.state(), State::Done(Ended::Exited(3)));
        jobs.remove_reported();
        assert!(jobs.get(1).is_none());
        assert_eq!(jobs.find(None), Ok(2));
        assert_eq!(background(&mut jobs, &[30], "true"), 3);
        assert_eq!(jobs.record(99, Change::Continued), None);
    }

    #[test]
    fn job_specifications_name_the_current_the_previous_a_number_or_a_command() {
        let mut jobs = Jobs::default();
        assert_eq!(jobs.find(None), Err(NoSuchJob::NoCurrent));
        background(&mut jobs, &[10], "sleep 1");
        background(&mut jobs, &[20], "cat notes");
        for current in [None, Some(&b"%%"[..]), Some(&b"%+"[..]), Some(&b"%"[..])] {
            assert_eq!(jobs.find(current), Ok(2));
        }
        assert_eq!(jobs.find(Some(b"%-")), Ok(1));
        assert_eq!(jobs.find(Some(b"%1")), Ok(1));
        assert_eq!(jobs.find(Some(b"1")), Ok(1));
        assert_eq!(jobs.find(Some(b"%cat")), Ok(2));
        assert_eq!(jobs.find(Some(b"%sl")), Ok(1));
        assert_eq!(jobs.find(Some(b"%5")), Err(NoSuchJob::Named));
        assert_eq!(jobs.find(Some(b"%vi")), Err(NoSuchJob::Named));
        assert_eq!(jobs.with_program(20), Some(2));
        jobs.record(10, Change::Stopped(21));
        assert_eq!(
            jobs.find(None),
            Ok(1),
            "a job that stops becomes the current one"
        );
        assert_eq!(jobs.find(Some(b"%-")), Ok(2));
        jobs.remove(1);
        assert_eq!(jobs.find(None), Ok(2));
        assert_eq!(jobs.find(Some(b"%-")), Err(NoSuchJob::Named));
    }

    #[test]
    fn a_continued_job_runs_again_without_a_report() {
        let mut jobs = Jobs::default();
        background(&mut jobs, &[10], "vi");
        jobs.record(10, Change::Stopped(22));
        assert_eq!(jobs.get(1).unwrap().state(), State::Stopped(22));
        jobs.take_changed();
        jobs.record(10, Change::Continued);
        assert_eq!(jobs.get(1).unwrap().state(), State::Running);
        assert!(jobs.take_changed().is_none());
        jobs.set_state(1, State::Stopped(20));
        assert!(jobs.take_changed().is_none());
    }

    #[test]
    fn a_full_table_makes_room_only_from_reported_jobs() {
        let mut jobs = Jobs::default();
        for index in 0..MAX_JOBS as i32 {
            background(&mut jobs, &[100 + index], "sleep");
        }
        let full = jobs.add(NewJob {
            group: 1,
            programs: &[1],
            ended: 0,
            last: Ok(1),
            text: b"one more",
            state: State::Running,
        });
        assert_eq!(full, Err(Full));
        jobs.record(101, Change::Ended(Ended::Killed(15)));
        jobs.record(100, Change::Ended(Ended::Exited(0)));
        jobs.take_changed();
        assert_eq!(background_fits(&mut jobs), Ok(MAX_JOBS as u16 + 1));
        assert!(
            jobs.get(1).is_none(),
            "the done job already reported goes first"
        );
        assert!(jobs.get(2).is_some());
        assert_eq!(background_fits(&mut jobs), Ok(MAX_JOBS as u16 + 2));
        assert!(jobs.get(2).is_none(), "then one done but not reported");
        assert_eq!(background_fits(&mut jobs), Err(Full));
    }

    fn background_fits(jobs: &mut Jobs) -> Result<u16, Full> {
        jobs.add(NewJob {
            group: 1,
            programs: &[1],
            ended: 0,
            last: Ok(1),
            text: b"one more",
            state: State::Running,
        })
    }

    #[test]
    fn a_job_whose_programs_all_ended_already_is_done_at_once() {
        let mut jobs = Jobs::default();
        let number = jobs
            .add(NewJob {
                group: 0,
                programs: &[7],
                ended: 1,
                last: Err(Ended::Exited(4)),
                text: b"ls | false",
                state: State::Running,
            })
            .unwrap();
        assert_eq!(
            jobs.get(number).unwrap().state(),
            State::Done(Ended::Exited(4))
        );
        assert!(jobs.get(number).unwrap().last_program().is_none());
        assert_eq!(jobs.get(number).unwrap().running_programs().count(), 0);
    }

    #[test]
    fn job_numbers_must_be_positive_decimals() {
        assert_eq!(number(b"12"), Some(12));
        assert_eq!(number(b"0"), None);
        assert_eq!(number(b"99999"), None);
        assert_eq!(number(b"1a"), None);
        assert_eq!(trimmed(b" \t a b \n"), b"a b");
        assert_eq!(trimmed(b"   "), b"");
    }
}
