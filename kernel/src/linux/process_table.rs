//! The process table (D105): every process's place in the tree and in job control, from its
//! creation until its parent reaps it. A process has a parent (0 for the kernel), the signal
//! its end sends that parent, a process group and a session; once it ends it is a zombie, its
//! [`ProcessExit`] kept until its parent's `wait4` or `waitid` takes it, or at once if nobody
//! can wait for it (the kernel is its parent, or the parent ignores `SIGCHLD` or set
//! `SA_NOCLDWAIT`). A process that ends hands its children to the nearest ancestor that asked
//! for orphans (`PR_SET_CHILD_SUBREAPER`), else to init, else to the kernel. The table also
//! records which session has each terminal, the console or a pseudo-terminal (D187), as its
//! controlling terminal, and the terminal's foreground process group.
//!
//! One lock holds it all, a leaf but for the heap: nothing else is locked under it, and
//! nothing it gives out is a strong reference to a process, so no process is freed under it.
//! A running process's entry holds a weak reference to its current program's process and
//! its first thread's wait slot, through which a signal sent to it from outside reaches it.
//!
//! A running process may be stopped by a stop signal and continued by `SIGCONT` (D181): its
//! entry says so until its parent's wait reports it (`WUNTRACED`, `WCONTINUED`), and the end
//! of a process that leaves a process group orphaned with stopped members hangs that group up
//! (`SIGHUP`, then `SIGCONT`), as POSIX requires.

use super::Errno;
use super::descriptors::WeakDescriptors;
use super::futex::WaitSlot;
use crate::process::ProcessExit;
use crate::sync::SpinLock;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::any::Any;

/// `SIGCHLD`, the signal a child's end sends its parent unless `clone` named another.
pub const SIGCHLD: u8 = super::signal::SIGCHLD as u8;

/// The table every process is in.
pub static TABLE: SpinLock<Table> = SpinLock::new(Table::new());

/// A process's current program, type-erased (a `Process<P>`), and its first thread's wait slot:
/// what a signal sent from outside the process needs to reach it; and that thread's descriptor
/// table, which `/proc/<pid>/fd` lists (D168).
#[derive(Debug, Clone)]
pub struct Running {
    pub process: Weak<dyn Any + Send + Sync>,
    pub first_thread: Arc<WaitSlot>,
    pub files: WeakDescriptors,
}

/// A process a signal is sent to: its id, and how to reach it while it runs (`None` once it
/// ended and before it is reaped: it is found, and takes nothing).
#[derive(Debug, Clone)]
pub struct Target {
    pub id: u32,
    pub running: Option<Running>,
}

impl Target {
    fn of(entry: &Entry) -> Self {
        Self {
            id: entry.id,
            running: match entry.exit {
                Some(_) => None,
                None => entry.running.clone(),
            },
        }
    }
}

#[derive(Debug)]
struct Entry {
    id: u32,
    parent: u32,
    exit_signal: u8,
    group: u32,
    session: u32,
    exit: Option<ProcessExit>,
    uid: u32,
    children: u32,
    child_subreaper: bool,
    no_zombie_children: bool,
    no_stop_notices: bool,
    executed: bool,
    job: JobState,
    running: Option<Running>,
    name: Name,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobState {
    Running,
    Stopped { signal: u8, reported: bool },
    Continued,
}

/// What a wait reports of children that stopped or continued, besides their ends (D181).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JobReports {
    /// Children stopped since a wait last reported it (`WUNTRACED`, `WSTOPPED`).
    pub stopped: bool,
    /// Children continued since a wait last reported it (`WCONTINUED`).
    pub continued: bool,
    /// Leaves what it reports to be reported again (`WNOWAIT`).
    pub keep: bool,
}

/// A child's stop or continuation, as its parent learns of it (D181).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobChange {
    /// This signal stopped it.
    Stopped(u8),
    /// `SIGCONT` continued it.
    Continued,
}

/// Whom a process's stop or continuation tells (D181): its parent, which wakes from a wait for
/// its children, and is sent `SIGCHLD` unless it set `SA_NOCLDSTOP` (`signal` false).
#[derive(Debug, Clone)]
pub struct JobNotice {
    pub parent: u32,
    pub running: Option<Running>,
    pub child: u32,
    pub uid: u32,
    pub change: JobChange,
    pub signal: bool,
}

/// What a stop signal's delivery did to a process (D181).
#[derive(Debug, Clone)]
pub enum Stopping {
    /// It is stopped now, and its parent is to be told, if it has one.
    Stopped(Option<JobNotice>),
    /// It was stopped already: the delivering thread stops with it.
    Already,
    /// It is not in the table, or has ended.
    Gone,
}

/// The length of a process's name, its NUL included (Linux's `TASK_COMM_LEN`).
pub const NAME_LENGTH: usize = 16;

/// A process's name: the last component of the file its program came from, at most
/// [`NAME_LENGTH`] - 1 bytes, as Linux's `comm`; NUL-padded.
pub type Name = [u8; NAME_LENGTH];

/// The name of the file at `path`'s last component, cut as a process's [`Name`] is.
pub fn name_of(file_name: &[u8]) -> Name {
    let mut name = [0; NAME_LENGTH];
    let length = file_name.len().min(NAME_LENGTH - 1);
    name[..length].copy_from_slice(&file_name[..length]);
    name
}

/// What `/proc` says of a process (D125).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listed {
    pub id: u32,
    pub parent: u32,
    pub group: u32,
    pub session: u32,
    pub ended: bool,
    pub name: Name,
    /// The signal its end sends its parent (D168, `/proc/<pid>/stat`).
    pub exit_signal: u8,
}

impl Listed {
    fn of(entry: &Entry) -> Self {
        Self {
            id: entry.id,
            parent: entry.parent,
            group: entry.group,
            session: entry.session,
            ended: entry.exit.is_some(),
            name: entry.name,
            exit_signal: entry.exit_signal,
        }
    }
}

/// Which children a wait looks at: Linux's `pid` argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selector {
    /// The child with this id (`pid > 0`).
    Process(u32),
    /// Any child in this process group (`pid < -1`, or the caller's own for 0).
    Group(u32),
    /// Any child (`-1`).
    Any,
}

/// Which children a wait looks at by the signal their end sends (`__WCLONE`, `__WALL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildKind {
    /// Those whose end sends `SIGCHLD`.
    Sigchld,
    /// The "clone" children: those whose end sends another signal or none (`__WCLONE`).
    Clone,
    /// Every child (`__WALL`).
    All,
}

/// What a wait does with a child that has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ends {
    /// Reports and reaps it.
    Reap,
    /// Reports it and leaves it a zombie (`WNOWAIT`).
    Report,
    /// Passes it over: the wait reports none (`waitid` without `WEXITED`).
    Ignore,
}

/// What a wait found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// A child that has ended, with its real user id and how; reaped unless the wait only
    /// looked.
    Ended {
        id: u32,
        uid: u32,
        exit: ProcessExit,
    },
    /// A child that a stop signal stopped (D181); reported once unless the wait only looked.
    Stopped { id: u32, uid: u32, signal: u8 },
    /// A child that `SIGCONT` continued (D181); reported once unless the wait only looked.
    Continued { id: u32, uid: u32 },
    /// Children that may be waited for, none of them ended yet.
    Running,
    /// No child the wait may wait for (`ECHILD`).
    NoChild,
}

/// Whom a process's end tells, and with which signal (0 for none): its parent, which wakes
/// from a wait for its children.
#[derive(Debug, Clone)]
pub struct Notice {
    pub parent: u32,
    pub running: Option<Running>,
    pub signal: u8,
    pub child: u32,
    pub uid: u32,
    pub exit: ProcessExit,
}

/// What the end of a process leaves to be done once the table's lock is released.
#[derive(Debug, Default)]
pub struct Ended {
    /// Its own end, for its parent.
    pub to_parent: Option<Notice>,
    /// The ended children it handed over, for their new parent.
    pub to_new_parent: Option<Notice>,
    /// The foreground group of the terminal the session's leader controlled, which loses its
    /// controlling terminal as the leader ends: each gets `SIGHUP` and `SIGCONT`.
    pub hung_up: Vec<Running>,
}

/// A terminal a session may have as its controlling terminal (D187).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminal {
    /// The console (`/dev/console`).
    Console,
    /// Pseudo-terminal `n` (`/dev/pts/<n>`), of which `/dev/ptmx` opened the other side.
    Pseudo(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Controlled {
    terminal: Terminal,
    session: u32,
    foreground: u32,
}

/// See the module documentation.
#[derive(Debug)]
pub struct Table {
    entries: Vec<Entry>,
    init: u32,
    controlled: Vec<Controlled>,
    stopped: u32,
}

impl Table {
    /// No process, no init and no terminal any session's.
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            init: 0,
            controlled: Vec::new(),
            stopped: 0,
        }
    }

    fn controlled_by(&self, terminal: Terminal) -> Option<&Controlled> {
        self.controlled
            .iter()
            .find(|controlled| controlled.terminal == terminal)
    }

    fn controlled_by_session(&self, session: u32) -> Option<usize> {
        match session {
            0 => None,
            session => self
                .controlled
                .iter()
                .position(|controlled| controlled.session == session),
        }
    }

    fn index(&self, id: u32) -> Option<usize> {
        match self.entries.last() {
            Some(last) if last.id == id => Some(self.entries.len() - 1),
            Some(last) if last.id < id => None,
            _ => self
                .entries
                .binary_search_by_key(&id, |entry| entry.id)
                .ok(),
        }
    }

    fn insertion_point(&self, id: u32) -> Result<usize, usize> {
        match self.entries.last() {
            Some(last) if last.id < id => Err(self.entries.len()),
            _ => self.entries.binary_search_by_key(&id, |entry| entry.id),
        }
    }

    fn entry(&self, id: u32) -> Option<&Entry> {
        self.index(id).map(|index| &self.entries[index])
    }

    fn entry_mut(&mut self, id: u32) -> Option<&mut Entry> {
        self.index(id).map(|index| &mut self.entries[index])
    }

    /// Adds process `id`: started by the kernel (`parent` `None`), the leader of a new session
    /// and process group; or a child of `parent`, in its group and session, which inherits
    /// whether `SIGCHLD` is ignored, its end sending no signal until [`Self::set_exit_signal`].
    /// `Err` without memory for it.
    pub fn add(&mut self, id: u32, parent: Option<u32>) -> Result<(), Errno> {
        let Err(at) = self.insertion_point(id) else {
            return Err(Errno::EAGAIN);
        };
        let parent_index = parent.and_then(|parent| self.index(parent));
        let inherited = parent_index.map(|index| &self.entries[index]);
        let entry = Entry {
            id,
            parent: inherited.map_or(0, |parent| parent.id),
            exit_signal: 0,
            group: inherited.map_or(id, |parent| parent.group),
            session: inherited.map_or(id, |parent| parent.session),
            exit: None,
            uid: 0,
            children: 0,
            child_subreaper: false,
            no_zombie_children: inherited.is_some_and(|parent| parent.no_zombie_children),
            no_stop_notices: inherited.is_some_and(|parent| parent.no_stop_notices),
            executed: false,
            job: JobState::Running,
            running: None,
            name: inherited.map_or([0; NAME_LENGTH], |parent| parent.name),
        };
        self.entries.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
        self.entries.insert(at, entry);
        if let Some(index) = parent_index {
            let index = if index >= at { index + 1 } else { index };
            self.entries[index].children += 1;
        }
        Ok(())
    }

    fn remove_at(&mut self, index: usize) {
        let parent = self.entries.remove(index).parent;
        if let Some(parent) = self.entry_mut(parent) {
            parent.children = parent.children.saturating_sub(1);
        }
    }

    /// Adds process `id` as [`Self::add`] does for one the kernel started, unless it is in the
    /// table already.
    pub fn add_if_missing(&mut self, id: u32) -> Result<(), Errno> {
        match self.index(id) {
            Some(_) => Ok(()),
            None => self.add(id, None),
        }
    }

    /// Sets the signal process `id`'s end sends its parent (0 for none).
    pub fn set_exit_signal(&mut self, id: u32, signal: u8) {
        if let Some(entry) = self.entry_mut(id) {
            entry.exit_signal = signal;
        }
    }

    /// Records the program process `id` runs now and its first thread's wait slot.
    pub fn set_running(&mut self, id: u32, running: Running) {
        if let Some(entry) = self.entry_mut(id) {
            entry.running = Some(running);
        }
    }

    /// Names process `id`.
    pub fn set_name(&mut self, id: u32, name: Name) {
        if let Some(entry) = self.entry_mut(id) {
            entry.name = name;
        }
    }

    /// Every process in the table, by id: what `/proc` lists.
    pub fn list(&self) -> impl Iterator<Item = Listed> + '_ {
        self.entries.iter().map(Listed::of)
    }

    /// Records that process `id` ran `execve`: its parent may no longer move it to another
    /// group (`setpgid`'s `EACCES`).
    pub fn mark_executed(&mut self, id: u32) {
        if let Some(entry) = self.entry_mut(id) {
            entry.executed = true;
        }
    }

    /// Records whether process `id`'s children become zombies as they end: not if it ignores
    /// `SIGCHLD` or set `SA_NOCLDWAIT` for it.
    pub fn set_no_zombie_children(&mut self, id: u32, no_zombies: bool) {
        if let Some(entry) = self.entry_mut(id) {
            entry.no_zombie_children = no_zombies;
        }
    }

    /// Records whether process `id` is sent `SIGCHLD` when a child stops or continues: not if
    /// it set `SA_NOCLDSTOP` for it (D181).
    pub fn set_no_stop_notices(&mut self, id: u32, no_notices: bool) {
        if let Some(entry) = self.entry_mut(id) {
            entry.no_stop_notices = no_notices;
        }
    }

    /// Makes process `id` the one orphans go to when no ancestor of theirs is a subreaper.
    pub fn set_init(&mut self, id: u32) {
        self.init = id;
    }

    /// Sets whether process `id` takes in its descendants' orphans (`PR_SET_CHILD_SUBREAPER`).
    pub fn set_child_subreaper(&mut self, id: u32, subreaper: bool) {
        if let Some(entry) = self.entry_mut(id) {
            entry.child_subreaper = subreaper;
        }
    }

    /// Whether process `id` takes in its descendants' orphans.
    pub fn is_child_subreaper(&self, id: u32) -> bool {
        self.entry(id).is_some_and(|entry| entry.child_subreaper)
    }

    /// Process `id`'s parent, 0 for the kernel or a process not in the table.
    pub fn parent(&self, id: u32) -> u32 {
        self.entry(id).map_or(0, |entry| entry.parent)
    }

    /// Process `id`'s process group and session, if it is in the table, ended or not.
    pub fn group_and_session(&self, id: u32) -> Option<(u32, u32)> {
        self.entry(id).map(|entry| (entry.group, entry.session))
    }

    fn is_alive(&self, id: u32) -> bool {
        self.entry(id).is_some_and(|entry| entry.exit.is_none())
    }

    fn new_parent_for_children_of(&self, id: u32) -> u32 {
        let mut ancestor = self.parent(id);
        let mut steps = self.entries.len();
        while ancestor != 0 && steps > 0 {
            if self.is_child_subreaper(ancestor) && self.is_alive(ancestor) {
                return ancestor;
            }
            ancestor = self.parent(ancestor);
            steps -= 1;
        }
        match self.init != id && self.is_alive(self.init) {
            true => self.init,
            false => 0,
        }
    }

    fn reaps_at_end(&self, parent: u32, exit_signal: u8) -> bool {
        Self::reaped_by(self.entry(parent), exit_signal)
    }

    fn reaped_by(parent: Option<&Entry>, exit_signal: u8) -> bool {
        parent.is_none_or(|parent| {
            parent.exit.is_some() || (exit_signal == SIGCHLD && parent.no_zombie_children)
        })
    }

    /// Records that process `id`, whose real user id is `uid`, ended as `exit`: it becomes a
    /// zombie for its parent, or is removed at once if nobody can wait for it; its children go
    /// to their new parent, which reaps those that ended already if it cannot wait for them;
    /// if it led a session that has a controlling terminal, the terminal belongs to none from
    /// now on and its foreground group is hung up. Returns whom to tell.
    pub fn end(&mut self, id: u32, exit: ProcessExit, uid: u32) -> Ended {
        let mut ended = Ended::default();
        let Some(index) = self.index(id) else {
            return ended;
        };
        if self.entries[index].exit.is_some() {
            return ended;
        }
        self.forget_stop(index);
        let around = match self.stopped {
            0 => Vec::new(),
            _ => self.groups_around(index),
        };
        self.entries[index].exit = Some(exit);
        self.entries[index].uid = uid;
        self.entries[index].running = None;
        let mut index = index;
        if self.entries[index].children > 0 {
            ended.to_new_parent = self.hand_over_children(id);
            let Some(moved) = self.index(id) else {
                return ended;
            };
            index = moved;
        }
        let entry = &self.entries[index];
        let (parent, session, exit_signal) = (entry.parent, entry.session, entry.exit_signal);
        let parent_entry = match parent {
            0 => None,
            parent => self.entry(parent),
        };
        if let Some(parent_entry) = parent_entry {
            ended.to_parent = Some(Notice {
                parent,
                running: parent_entry.running.clone(),
                signal: exit_signal,
                child: id,
                uid,
                exit,
            });
        }
        if Self::reaped_by(parent_entry, exit_signal) {
            self.remove_at(index);
        }
        let mut hung_up_group = 0;
        if session == id
            && let Some(index) = self.controlled_by_session(id)
        {
            let controlled = self.controlled.swap_remove(index);
            hung_up_group = controlled.foreground;
            ended.hung_up = self.members(controlled.foreground);
        }
        for group in around {
            if group != hung_up_group && self.has_stopped_member(group) && self.is_orphaned(group) {
                let mut members = self.members(group);
                if ended.hung_up.try_reserve(members.len()).is_ok() {
                    ended.hung_up.append(&mut members);
                }
            }
        }
        ended
    }

    fn forget_stop(&mut self, index: usize) {
        if let JobState::Stopped { .. } = self.entries[index].job {
            self.stopped = self.stopped.saturating_sub(1);
        }
        self.entries[index].job = JobState::Running;
    }

    fn groups_around(&self, index: usize) -> Vec<u32> {
        let ending = &self.entries[index];
        let mut groups = Vec::new();
        if groups.try_reserve(1).is_ok() {
            groups.push(ending.group);
        }
        if ending.children == 0 {
            return groups;
        }
        for entry in &self.entries {
            if entry.parent == ending.id
                && entry.session == ending.session
                && !groups.contains(&entry.group)
                && groups.try_reserve(1).is_ok()
            {
                groups.push(entry.group);
            }
        }
        groups
    }

    fn is_orphaned(&self, group: u32) -> bool {
        !self.entries.iter().any(|member| {
            member.group == group
                && member.exit.is_none()
                && self.entry(member.parent).is_some_and(|parent| {
                    parent.exit.is_none()
                        && parent.group != group
                        && parent.session == member.session
                })
        })
    }

    fn has_stopped_member(&self, group: u32) -> bool {
        self.entries.iter().any(|entry| {
            entry.group == group
                && entry.exit.is_none()
                && matches!(entry.job, JobState::Stopped { .. })
        })
    }

    /// For a stop signal delivered to process `id`, whose real user id is `uid` (D181): stops
    /// it, unless it is stopped already, and says whom to tell.
    pub fn stop(&mut self, id: u32, signal: u8, uid: u32) -> Stopping {
        let Some(index) = self.index(id) else {
            return Stopping::Gone;
        };
        let entry = &mut self.entries[index];
        if entry.exit.is_some() {
            return Stopping::Gone;
        }
        if let JobState::Stopped { .. } = entry.job {
            return Stopping::Already;
        }
        entry.job = JobState::Stopped {
            signal,
            reported: false,
        };
        entry.uid = uid;
        let parent = entry.parent;
        self.stopped += 1;
        Stopping::Stopped(self.job_notice(parent, id, uid, JobChange::Stopped(signal)))
    }

    /// For `SIGCONT` sent to process `id` (D181): continues it if it is stopped, and says whom
    /// to tell; `None` if it was not stopped.
    pub fn resume(&mut self, id: u32) -> Option<JobNotice> {
        let index = self.index(id)?;
        let entry = &mut self.entries[index];
        if entry.exit.is_some() || !matches!(entry.job, JobState::Stopped { .. }) {
            return None;
        }
        entry.job = JobState::Continued;
        let (parent, uid) = (entry.parent, entry.uid);
        self.stopped = self.stopped.saturating_sub(1);
        self.job_notice(parent, id, uid, JobChange::Continued)
    }

    fn job_notice(
        &self,
        parent: u32,
        child: u32,
        uid: u32,
        change: JobChange,
    ) -> Option<JobNotice> {
        let entry = self.entry(parent).filter(|entry| entry.exit.is_none())?;
        Some(JobNotice {
            parent,
            running: entry.running.clone(),
            child,
            uid,
            change,
            signal: !entry.no_stop_notices,
        })
    }

    /// Whether process `id` is stopped (D181).
    pub fn is_stopped(&self, id: u32) -> bool {
        self.entry(id)
            .is_some_and(|entry| matches!(entry.job, JobState::Stopped { .. }))
    }

    /// Whether process `id`'s group is orphaned, so the terminal's stop signals do not stop it
    /// (D181).
    pub fn is_in_orphaned_group(&self, id: u32) -> bool {
        self.entry(id)
            .is_some_and(|entry| self.is_orphaned(entry.group))
    }

    /// For process `caller` reading `terminal` or changing its settings (D181): its process
    /// group, and whether that group is orphaned, if `terminal` is its controlling terminal
    /// and its group is not the terminal's foreground; `None` otherwise (it may go on).
    pub fn background_on(&self, terminal: Terminal, caller: u32) -> Option<(u32, bool)> {
        let entry = self.entry(caller)?;
        let controlled = self.controlled_by(terminal)?;
        (controlled.session == entry.session && entry.group != controlled.foreground)
            .then(|| (entry.group, self.is_orphaned(entry.group)))
    }

    /// [`Self::background_on`] the console.
    pub fn background_on_console(&self, caller: u32) -> Option<(u32, bool)> {
        self.background_on(Terminal::Console, caller)
    }

    /// Removes process `id`, which never ended as a Linux process does (it never ran, or its
    /// program never reached its end), handing its children over as [`Self::end`] does. Its
    /// parent is told with no signal, so a wait for it looks again (and may find no child).
    pub fn remove_unended(&mut self, id: u32) -> Ended {
        let mut ended = Ended::default();
        let Some(index) = self.index(id) else {
            return ended;
        };
        if self.entries[index].exit.is_some() {
            return ended;
        }
        self.forget_stop(index);
        ended.to_new_parent = self.hand_over_children(id);
        if let Some(index) = self.index(id) {
            let parent = self.entries[index].parent;
            if self.index(parent).is_some() {
                ended.to_parent = Some(Notice {
                    parent,
                    running: None,
                    signal: 0,
                    child: id,
                    uid: 0,
                    exit: ProcessExit::Exited(0),
                });
            }
            self.remove_at(index);
        }
        if let Some(index) = self.controlled_by_session(id) {
            self.controlled.swap_remove(index);
        }
        ended
    }

    fn hand_over_children(&mut self, id: u32) -> Option<Notice> {
        let entry = self.entry_mut(id)?;
        if entry.children == 0 {
            return None;
        }
        entry.children = 0;
        let new_parent = self.new_parent_for_children_of(id);
        let reaped = self.reaps_at_end(new_parent, SIGCHLD);
        let mut first_ended = None;
        let mut kept = 0;
        self.entries.retain_mut(|entry| {
            if entry.parent != id {
                return true;
            }
            entry.parent = new_parent;
            entry.exit_signal = SIGCHLD;
            if let (None, Some(exit)) = (first_ended, entry.exit) {
                first_ended = Some((entry.id, entry.uid, exit));
            }
            let keep = !(reaped && entry.exit.is_some());
            kept += u32::from(keep);
            keep
        });
        if let Some(entry) = self.entry_mut(new_parent) {
            entry.children += kept;
        }
        let (child, uid, exit) = first_ended?;
        (new_parent != 0).then(|| Notice {
            parent: new_parent,
            running: self
                .entry(new_parent)
                .and_then(|entry| entry.running.clone()),
            signal: SIGCHLD,
            child,
            uid,
            exit,
        })
    }

    /// [`Self::wait_for_change`] for ends alone.
    #[cfg(test)]
    pub fn wait(&mut self, parent: u32, selector: Selector, kind: ChildKind, ends: Ends) -> Found {
        self.wait_for_change(parent, selector, kind, ends, JobReports::default())
    }

    /// The wait of process `parent` for one of its children that `selector` and `kind` name:
    /// the first of them that has ended, reaped or not as `ends` says, or that stopped or
    /// continued since a wait last said so, if `reports` asks for those (D181); otherwise
    /// whether any of them is running.
    pub fn wait_for_change(
        &mut self,
        parent: u32,
        selector: Selector,
        kind: ChildKind,
        ends: Ends,
        reports: JobReports,
    ) -> Found {
        let matches = |entry: &Entry| {
            entry.parent == parent
                && match selector {
                    Selector::Process(id) => entry.id == id,
                    Selector::Group(group) => entry.group == group,
                    Selector::Any => true,
                }
                && match kind {
                    ChildKind::Sigchld => entry.exit_signal == SIGCHLD,
                    ChildKind::Clone => entry.exit_signal != SIGCHLD,
                    ChildKind::All => true,
                }
        };
        if self.entry(parent).is_none_or(|entry| entry.children == 0) {
            return Found::NoChild;
        }
        let mut running = false;
        let mut found = None;
        for (index, entry) in self.entries.iter().enumerate() {
            if !matches(entry) {
                continue;
            }
            let (id, uid) = (entry.id, entry.uid);
            match (entry.exit, entry.job) {
                (Some(_), _) if ends == Ends::Ignore => {}
                (Some(exit), _) => {
                    found = Some((index, Found::Ended { id, uid, exit }));
                    break;
                }
                (
                    None,
                    JobState::Stopped {
                        signal,
                        reported: false,
                    },
                ) if reports.stopped => {
                    found = Some((index, Found::Stopped { id, uid, signal }));
                    break;
                }
                (None, JobState::Continued) if reports.continued => {
                    found = Some((index, Found::Continued { id, uid }));
                    break;
                }
                (None, _) => running = true,
            }
        }
        match found {
            Some((index, found)) => {
                match found {
                    Found::Ended { .. } if ends == Ends::Reap => self.remove_at(index),
                    Found::Stopped { signal, .. } if !reports.keep => {
                        self.entries[index].job = JobState::Stopped {
                            signal,
                            reported: true,
                        };
                    }
                    Found::Continued { .. } if !reports.keep => {
                        self.entries[index].job = JobState::Running;
                    }
                    _ => {}
                }
                found
            }
            None if running => Found::Running,
            None => Found::NoChild,
        }
    }

    fn group_exists_in(&self, group: u32, session: u32) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.group == group && entry.session == session)
    }

    /// `setpgid(pid, pgid)` by process `caller`, the arguments as `pid_t`s: moves process `pid`
    /// (the caller for 0), the caller or one of its children, to group `pgid` (its own id for
    /// 0), a new one or one in the same session. As Linux: `EINVAL` for a negative `pgid`;
    /// `ESRCH` for a `pid` that is neither; `EACCES` for a child that ran `execve`; `EPERM`
    /// for a child in another session, a session leader, or a group of another session.
    pub fn set_group(&mut self, caller: u32, pid: i32, pgid: i32) -> Result<(), Errno> {
        if pgid < 0 {
            return Err(Errno::EINVAL);
        }
        let target = match pid {
            0 => caller,
            pid if pid < 0 => return Err(Errno::ESRCH),
            pid => pid as u32,
        };
        let group = match pgid {
            0 => target,
            pgid => pgid as u32,
        };
        let (caller_session, _) = self.session_of(caller)?;
        let entry = self.entry(target).ok_or(Errno::ESRCH)?;
        if target != caller && (entry.parent != caller || entry.exit.is_some()) {
            return Err(Errno::ESRCH);
        }
        if entry.session != caller_session {
            return Err(Errno::EPERM);
        }
        if target != caller && entry.executed {
            return Err(Errno::EACCES);
        }
        if entry.session == target {
            return Err(Errno::EPERM);
        }
        if group != target && !self.group_exists_in(group, caller_session) {
            return Err(Errno::EPERM);
        }
        if let Some(entry) = self.entry_mut(target) {
            entry.group = group;
        }
        Ok(())
    }

    fn session_of(&self, id: u32) -> Result<(u32, u32), Errno> {
        self.entry(id)
            .map(|entry| (entry.session, entry.group))
            .ok_or(Errno::ESRCH)
    }

    /// `setsid()` by process `caller`: a new session and group it leads, with no controlling
    /// terminal; returns its id. `EPERM` if a group with its id exists already (it leads one).
    pub fn new_session(&mut self, caller: u32) -> Result<u32, Errno> {
        if self.entries.iter().any(|entry| entry.group == caller) {
            return Err(Errno::EPERM);
        }
        let entry = self.entry_mut(caller).ok_or(Errno::ESRCH)?;
        entry.group = caller;
        entry.session = caller;
        Ok(caller)
    }

    /// For a session leader the kernel starts, `id`: the console becomes its session's
    /// controlling terminal, its group the foreground, if the console has no session yet.
    pub fn give_console_if_free(&mut self, id: u32) {
        self.give_if_free(Terminal::Console, id);
    }

    /// For process `id` opening `terminal` without `O_NOCTTY`, as Linux's open of a terminal
    /// does: if it leads a session that has no controlling terminal and `terminal` is no
    /// session's, `terminal` becomes the session's, its group the foreground.
    pub fn give_if_free(&mut self, terminal: Terminal, id: u32) {
        if self.controlled_by(terminal).is_some() || self.controlled_by_session(id).is_some() {
            return;
        }
        let Some(entry) = self.entry(id).filter(|entry| entry.session == id) else {
            return;
        };
        let controlled = Controlled {
            terminal,
            session: id,
            foreground: entry.group,
        };
        if self.controlled.try_reserve(1).is_ok() {
            self.controlled.push(controlled);
        }
    }

    /// `terminal` becomes no session's controlling terminal.
    pub fn release(&mut self, terminal: Terminal) {
        self.controlled
            .retain(|controlled| controlled.terminal != terminal);
    }

    /// The console becomes no session's controlling terminal.
    pub fn release_console(&mut self) {
        self.release(Terminal::Console);
    }

    /// The terminal process `caller`'s session controls, if it has one.
    pub fn controlling_terminal_of(&self, caller: u32) -> Option<Terminal> {
        let (session, _) = self.session_of(caller).ok()?;
        let index = self.controlled_by_session(session)?;
        Some(self.controlled[index].terminal)
    }

    /// `terminal`'s session and foreground group, if `terminal` is process `caller`'s
    /// controlling terminal; `ENOTTY` otherwise.
    pub fn controlling(&self, terminal: Terminal, caller: u32) -> Result<(u32, u32), Errno> {
        let (session, _) = self.session_of(caller).map_err(|_| Errno::ENOTTY)?;
        match self.controlled_by(terminal) {
            Some(controlled) if controlled.session == session && session != 0 => {
                Ok((controlled.session, controlled.foreground))
            }
            _ => Err(Errno::ENOTTY),
        }
    }

    /// [`Self::controlling`] the console.
    pub fn controlling_terminal(&self, caller: u32) -> Result<(u32, u32), Errno> {
        self.controlling(Terminal::Console, caller)
    }

    /// `terminal`'s session and foreground group, whoever asks, if a session controls it: what
    /// a pseudo-terminal's other side reads (`TIOCGPGRP`, `TIOCGSID` on `/dev/ptmx`'s side).
    pub fn session_and_foreground(&self, terminal: Terminal) -> Option<(u32, u32)> {
        self.controlled_by(terminal)
            .map(|controlled| (controlled.session, controlled.foreground))
    }

    /// `TIOCSPGRP` by process `caller`: makes group `group` (a `pid_t`) `terminal`'s
    /// foreground. `ENOTTY` unless `terminal` is the caller's controlling terminal; `EINVAL`
    /// for a negative group; `ESRCH` for a group no process is in; `EPERM` for a group of
    /// another session.
    pub fn set_foreground(
        &mut self,
        terminal: Terminal,
        caller: u32,
        group: i32,
    ) -> Result<(), Errno> {
        let (session, _) = self.controlling(terminal, caller)?;
        if group < 0 {
            return Err(Errno::EINVAL);
        }
        if !self.entries.iter().any(|entry| entry.group == group as u32) {
            return Err(Errno::ESRCH);
        }
        if !self.group_exists_in(group as u32, session) {
            return Err(Errno::EPERM);
        }
        if let Some(controlled) = self
            .controlled
            .iter_mut()
            .find(|controlled| controlled.terminal == terminal)
        {
            controlled.foreground = group as u32;
        }
        Ok(())
    }

    /// [`Self::set_foreground`] of the console.
    pub fn set_console_foreground(&mut self, caller: u32, group: i32) -> Result<(), Errno> {
        self.set_foreground(Terminal::Console, caller, group)
    }

    /// `TIOCSCTTY` by process `caller`: makes `terminal` its session's controlling terminal,
    /// its group the foreground. As Linux: `EPERM` unless the caller leads its session; then
    /// nothing to do if it is already; `EPERM` if the session controls another terminal, or if
    /// `terminal` is another session's and the caller may not `steal` it (an argument of 1 and
    /// privileges). `ENOMEM` without memory.
    pub fn take(&mut self, terminal: Terminal, caller: u32, steal: bool) -> Result<(), Errno> {
        let (session, group) = self.session_of(caller)?;
        if session != caller {
            return Err(Errno::EPERM);
        }
        let current = self.controlled_by(terminal).map(|owner| owner.session);
        if current == Some(session) {
            return Ok(());
        }
        if self.controlled_by_session(session).is_some() || (current.is_some() && !steal) {
            return Err(Errno::EPERM);
        }
        self.release(terminal);
        self.controlled.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
        self.controlled.push(Controlled {
            terminal,
            session,
            foreground: group,
        });
        Ok(())
    }

    /// [`Self::take`] the console.
    pub fn take_console(&mut self, caller: u32, steal: bool) -> Result<(), Errno> {
        self.take(Terminal::Console, caller, steal)
    }

    /// Hangs `terminal` up, as the last close of a pseudo-terminal's `/dev/ptmx` side does
    /// (D187): it becomes no session's, and the running processes of its foreground group and
    /// its session's leader are returned, to be sent `SIGHUP` and `SIGCONT`.
    pub fn hang_up(&mut self, terminal: Terminal) -> Vec<Running> {
        let Some(controlled) = self.controlled_by(terminal).copied() else {
            return Vec::new();
        };
        self.release(terminal);
        let mut hung_up = self.members(controlled.foreground);
        let leader = self
            .entry(controlled.session)
            .filter(|leader| leader.group != controlled.foreground && leader.exit.is_none())
            .and_then(|leader| leader.running.clone());
        if let Some(leader) = leader
            && hung_up.try_reserve(1).is_ok()
        {
            hung_up.push(leader);
        }
        hung_up
    }

    /// The running processes of group `group`.
    pub fn members(&self, group: u32) -> Vec<Running> {
        let mut members = Vec::new();
        if group == 0 {
            return members;
        }
        for entry in &self.entries {
            if entry.group == group
                && let Some(running) = &entry.running
                && members.try_reserve(1).is_ok()
            {
                members.push(running.clone());
            }
        }
        members
    }

    /// Process `id` as a signal sent to it finds it, ended or not; `None` if it is not in the
    /// table.
    pub fn target(&self, id: u32) -> Option<Target> {
        self.entry(id).map(Target::of)
    }

    /// What `/proc/<id>` says of process `id` and how to reach its program while it runs, if
    /// the process is in the table (D168).
    pub fn describe(&self, id: u32) -> Option<(Listed, Option<Running>)> {
        let entry = self.entry(id)?;
        Some((Listed::of(entry), Target::of(entry).running))
    }

    /// Every process of group `group` as a signal sent to the group finds it, ended or not.
    pub fn group_targets(&self, group: u32) -> Vec<Target> {
        let mut targets = Vec::new();
        if group == 0 {
            return targets;
        }
        for entry in &self.entries {
            if entry.group == group && targets.try_reserve(1).is_ok() {
                targets.push(Target::of(entry));
            }
        }
        targets
    }

    /// Every process a signal sent to `-1` finds (`kill(-1, ...)`, D183): all but `caller` and
    /// init, ended or not.
    pub fn every_target_but(&self, caller: u32) -> Vec<Target> {
        let mut targets = Vec::new();
        for entry in &self.entries {
            if entry.id != caller && entry.id != self.init && targets.try_reserve(1).is_ok() {
                targets.push(Target::of(entry));
            }
        }
        targets
    }

    /// Adds to `ids` every process id the table holds in use (D183): each process's, ended or
    /// not, every process group's and session's, each controlling terminal's session and
    /// foreground group, and init's; and returns how to reach each running process, whose other threads hold
    /// ids too. Ids are pushed unsorted, possibly more than once. `Err` without memory.
    pub fn ids_in_use(&self, ids: &mut Vec<u32>) -> Result<Vec<Running>, Errno> {
        ids.try_reserve(self.entries.len() * 3 + self.controlled.len() * 2 + 1)
            .map_err(|_| Errno::ENOMEM)?;
        let mut running = Vec::new();
        running
            .try_reserve(self.entries.len())
            .map_err(|_| Errno::ENOMEM)?;
        for entry in &self.entries {
            ids.extend([entry.id, entry.group, entry.session]);
            if entry.exit.is_none()
                && let Some(reach) = &entry.running
            {
                running.push(reach.clone());
            }
        }
        ids.push(self.init);
        for controlled in &self.controlled {
            ids.extend([controlled.session, controlled.foreground]);
        }
        Ok(running)
    }

    /// The process orphans go to, which cannot be killed; 0 before there is one.
    pub fn init(&self) -> u32 {
        self.init
    }

    /// The running processes of `terminal`'s foreground group; none while no session controls
    /// it.
    pub fn foreground_members(&self, terminal: Terminal) -> Vec<Running> {
        match self.controlled_by(terminal) {
            Some(controlled) => self.members(controlled.foreground),
            None => Vec::new(),
        }
    }

    /// [`Self::foreground_members`] of the console.
    pub fn console_foreground_members(&self) -> Vec<Running> {
        self.foreground_members(Terminal::Console)
    }

    /// How many processes the table holds, zombies among them, in session `session`.
    pub fn in_session(&self, session: u32) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.session == session)
            .count()
    }
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXITED: ProcessExit = ProcessExit::Exited(3);

    fn table_with(ids: &[(u32, Option<u32>)]) -> Table {
        let mut table = Table::new();
        for &(id, parent) in ids {
            table.add(id, parent).unwrap();
            table.set_exit_signal(id, SIGCHLD);
        }
        table
    }

    #[test]
    fn a_child_ends_as_a_zombie_until_its_parent_reaps_it_once() {
        let mut table = table_with(&[(10, None), (11, Some(10)), (12, Some(10))]);
        assert_eq!(
            table.wait(10, Selector::Any, ChildKind::Sigchld, Ends::Reap),
            Found::Running
        );
        let ended = table.end(11, EXITED, 0);
        let notice = ended.to_parent.unwrap();
        assert_eq!(
            (notice.parent, notice.signal, notice.child),
            (10, SIGCHLD, 11)
        );
        assert_eq!(
            table.wait(10, Selector::Process(11), ChildKind::Sigchld, Ends::Report),
            Found::Ended {
                id: 11,
                uid: 0,
                exit: EXITED
            }
        );
        assert_eq!(
            table.wait(10, Selector::Process(11), ChildKind::Sigchld, Ends::Ignore),
            Found::NoChild
        );
        assert_eq!(
            table.wait(10, Selector::Any, ChildKind::Sigchld, Ends::Ignore),
            Found::Running
        );
        assert_eq!(
            table.wait(10, Selector::Any, ChildKind::Sigchld, Ends::Reap),
            Found::Ended {
                id: 11,
                uid: 0,
                exit: EXITED
            }
        );
        assert_eq!(
            table.wait(10, Selector::Process(11), ChildKind::Sigchld, Ends::Reap),
            Found::NoChild
        );
        assert_eq!(
            table.wait(10, Selector::Any, ChildKind::Sigchld, Ends::Reap),
            Found::Running
        );
        assert_eq!(table.end(12, EXITED, 0).to_parent.unwrap().child, 12);
        assert!(matches!(
            table.wait(10, Selector::Any, ChildKind::Sigchld, Ends::Reap),
            Found::Ended { id: 12, .. }
        ));
        assert_eq!(table.in_session(10), 1);
    }

    #[test]
    fn waits_select_children_by_id_group_and_exit_signal() {
        let mut table = table_with(&[(20, None), (21, Some(20)), (22, Some(20)), (23, Some(20))]);
        table.set_exit_signal(23, 0);
        table.set_group(20, 22, 0).unwrap();
        for id in [21, 22, 23] {
            table.end(id, ProcessExit::Exited(id), 0);
        }
        assert_eq!(
            table.wait(20, Selector::Group(22), ChildKind::Sigchld, Ends::Reap),
            Found::Ended {
                id: 22,
                uid: 0,
                exit: ProcessExit::Exited(22)
            }
        );
        assert_eq!(
            table.wait(20, Selector::Group(22), ChildKind::Sigchld, Ends::Reap),
            Found::NoChild
        );
        assert_eq!(
            table.wait(20, Selector::Any, ChildKind::Clone, Ends::Reap),
            Found::Ended {
                id: 23,
                uid: 0,
                exit: ProcessExit::Exited(23)
            }
        );
        assert_eq!(
            table.wait(20, Selector::Any, ChildKind::Clone, Ends::Reap),
            Found::NoChild
        );
        assert_eq!(
            table.wait(20, Selector::Group(20), ChildKind::All, Ends::Reap),
            Found::Ended {
                id: 21,
                uid: 0,
                exit: ProcessExit::Exited(21)
            }
        );
        assert_eq!(
            table.wait(21, Selector::Any, ChildKind::All, Ends::Reap),
            Found::NoChild
        );
    }

    #[test]
    fn nobody_waits_for_a_child_of_the_kernel_or_of_a_parent_ignoring_sigchld() {
        let mut table = table_with(&[(30, None), (31, Some(30)), (32, Some(30))]);
        assert!(table.end(30, EXITED, 0).to_parent.is_none());
        assert_eq!(table.parent(31), 0);
        assert!(table.end(31, EXITED, 0).to_parent.is_none());
        assert!(table.group_and_session(31).is_none());
        let mut table = table_with(&[(40, None)]);
        table.set_no_zombie_children(40, true);
        table.add(41, Some(40)).unwrap();
        table.set_exit_signal(41, SIGCHLD);
        assert!(table.end(41, EXITED, 0).to_parent.is_some());
        assert_eq!(
            table.wait(40, Selector::Any, ChildKind::All, Ends::Reap),
            Found::NoChild
        );
        table.add(42, Some(40)).unwrap();
        table.set_exit_signal(42, 10);
        table.end(42, EXITED, 0);
        assert!(matches!(
            table.wait(40, Selector::Any, ChildKind::All, Ends::Reap),
            Found::Ended { id: 42, .. }
        ));
    }

    #[test]
    fn orphans_go_to_the_nearest_living_subreaper_then_init_then_the_kernel() {
        let mut table = table_with(&[
            (50, None),
            (51, Some(50)),
            (52, Some(51)),
            (53, Some(52)),
            (54, Some(52)),
        ]);
        table.set_child_subreaper(50, true);
        table.set_exit_signal(54, 0);
        table.end(54, EXITED, 0);
        let ended = table.end(52, EXITED, 0);
        assert_eq!(table.parent(53), 50);
        assert_eq!(table.parent(54), 50);
        let notice = ended.to_new_parent.unwrap();
        assert_eq!(
            (notice.parent, notice.child, notice.signal),
            (50, 54, SIGCHLD)
        );
        assert!(matches!(
            table.wait(50, Selector::Any, ChildKind::Sigchld, Ends::Reap),
            Found::Ended { id: 54, .. }
        ));
        let mut table = table_with(&[(60, None), (61, None), (62, Some(61)), (63, Some(62))]);
        table.set_init(60);
        table.end(62, EXITED, 0);
        assert_eq!(table.parent(63), 60);
        table.end(60, EXITED, 0);
        assert_eq!(table.parent(63), 0);
    }

    #[test]
    fn an_unended_process_that_goes_hands_its_children_over_and_leaves_no_entry() {
        let mut table = table_with(&[(70, None), (71, Some(70)), (72, Some(71))]);
        table.set_child_subreaper(70, true);
        table.end(72, EXITED, 0);
        let removed = table.remove_unended(71);
        let notice = removed.to_new_parent.unwrap();
        assert_eq!((notice.parent, notice.child), (70, 72));
        let told = removed.to_parent.unwrap();
        assert_eq!((told.parent, told.child, told.signal), (70, 71, 0));
        assert!(table.group_and_session(71).is_none());
        let removed = table.remove_unended(70);
        assert!(removed.to_parent.is_none() && removed.to_new_parent.is_none());
        assert!(table.group_and_session(72).is_none());
        assert_eq!(table.in_session(70), 0);
    }

    #[test]
    fn groups_and_sessions_follow_setpgid_and_setsid_rules() {
        let mut table = table_with(&[(80, None), (81, Some(80)), (82, Some(80)), (90, None)]);
        assert_eq!(table.group_and_session(81), Some((80, 80)));
        assert_eq!(table.set_group(80, 81, -1), Err(Errno::EINVAL));
        assert_eq!(table.set_group(80, 90, 0), Err(Errno::ESRCH));
        assert_eq!(table.set_group(80, 0, 0), Err(Errno::EPERM));
        assert_eq!(table.set_group(80, 81, 90), Err(Errno::EPERM));
        table.set_group(80, 81, 0).unwrap();
        table.set_group(80, 82, 81).unwrap();
        assert_eq!(table.group_and_session(82), Some((81, 80)));
        assert_eq!(table.new_session(81), Err(Errno::EPERM));
        assert_eq!(table.new_session(82), Ok(82));
        assert_eq!(table.group_and_session(82), Some((82, 82)));
        assert_eq!(table.set_group(80, 82, 80), Err(Errno::EPERM));
        table.mark_executed(82);
        assert_eq!(table.set_group(80, 82, 80), Err(Errno::EPERM));
        assert_eq!(table.set_group(82, 0, 81), Err(Errno::EPERM));
        table.mark_executed(81);
        assert_eq!(table.set_group(80, 81, 80), Err(Errno::EACCES));
        assert_eq!(table.set_group(81, 0, 80), Ok(()));
    }

    #[test]
    fn the_console_belongs_to_one_session_whose_leader_s_end_hangs_up_its_foreground() {
        let mut table = table_with(&[(100, None), (101, Some(100)), (110, None)]);
        assert_eq!(table.controlling_terminal(100), Err(Errno::ENOTTY));
        table.give_console_if_free(100);
        table.give_console_if_free(110);
        assert_eq!(table.controlling_terminal(101), Ok((100, 100)));
        assert_eq!(table.controlling_terminal(110), Err(Errno::ENOTTY));
        assert_eq!(table.set_console_foreground(110, 110), Err(Errno::ENOTTY));
        assert_eq!(table.set_console_foreground(101, 110), Err(Errno::EPERM));
        assert_eq!(table.set_console_foreground(101, 999), Err(Errno::ESRCH));
        table.set_group(100, 101, 0).unwrap();
        table.set_console_foreground(101, 101).unwrap();
        assert_eq!(table.controlling_terminal(100), Ok((100, 101)));
        assert_eq!(table.take_console(110, false), Err(Errno::EPERM));
        assert_eq!(table.take_console(101, true), Err(Errno::EPERM));
        assert_eq!(table.take_console(100, false), Ok(()));
        let slot = Arc::new(WaitSlot::new());
        table.set_running(
            101,
            Running {
                process: Weak::<()>::new(),
                first_thread: Arc::clone(&slot),
                files: WeakDescriptors::default(),
            },
        );
        assert_eq!(table.console_foreground_members().len(), 1);
        let ended = table.end(100, EXITED, 0);
        assert_eq!(ended.hung_up.len(), 1);
        assert_eq!(table.controlling_terminal(101), Err(Errno::ENOTTY));
        assert_eq!(table.take_console(110, false), Ok(()));
        assert_eq!(table.controlling_terminal(110), Ok((110, 110)));
    }

    #[test]
    fn each_session_controls_one_terminal_and_a_pseudo_terminal_hangs_up_its_session() {
        let mut table = table_with(&[(120, None), (121, Some(120)), (130, None), (131, Some(130))]);
        let pseudo = Terminal::Pseudo(3);
        table.give_console_if_free(120);
        table.give_if_free(pseudo, 120);
        assert_eq!(table.controlling_terminal_of(121), Some(Terminal::Console));
        assert_eq!(table.controlling(pseudo, 120), Err(Errno::ENOTTY));
        table.give_if_free(pseudo, 131);
        assert_eq!(
            table.controlling_terminal_of(131),
            None,
            "not a session leader"
        );
        table.give_if_free(pseudo, 130);
        assert_eq!(
            table.controlling_terminal_of(131),
            Some(Terminal::Pseudo(3))
        );
        assert_eq!(table.controlling(pseudo, 131), Ok((130, 130)));
        assert_eq!(table.session_and_foreground(pseudo), Some((130, 130)));
        assert_eq!(table.take(Terminal::Console, 130, false), Err(Errno::EPERM));
        assert_eq!(table.take(pseudo, 120, true), Err(Errno::EPERM));
        table.set_group(130, 131, 0).unwrap();
        table.set_foreground(pseudo, 130, 131).unwrap();
        assert_eq!(
            table.set_foreground(Terminal::Console, 130, 131),
            Err(Errno::ENOTTY)
        );
        assert_eq!(table.background_on(pseudo, 130), Some((130, true)));
        assert_eq!(table.background_on(pseudo, 131), None);
        assert_eq!(table.background_on(Terminal::Console, 131), None);
        table.set_running(130, running());
        table.set_running(131, running());
        assert_eq!(table.foreground_members(pseudo).len(), 1);
        assert_eq!(
            table.hang_up(pseudo).len(),
            2,
            "the foreground group and the leader"
        );
        assert_eq!(table.controlling_terminal_of(130), None);
        assert!(table.hang_up(pseudo).is_empty());
        assert_eq!(table.controlling_terminal_of(120), Some(Terminal::Console));
        let mut ids = Vec::new();
        table.ids_in_use(&mut ids).unwrap();
        assert!(ids.contains(&120));
        table.release_console();
        assert_eq!(table.controlling_terminal_of(121), None);
    }

    fn running() -> Running {
        Running {
            process: Weak::<()>::new(),
            first_thread: Arc::new(WaitSlot::new()),
            files: WeakDescriptors::default(),
        }
    }

    const STOPPED_AND_CONTINUED: JobReports = JobReports {
        stopped: true,
        continued: true,
        keep: false,
    };

    #[test]
    fn a_stopped_child_is_reported_once_to_a_wait_that_asks_then_its_continuation() {
        let mut table = table_with(&[(140, None), (141, Some(140))]);
        let Stopping::Stopped(Some(notice)) = table.stop(141, 20, 5) else {
            panic!("the child did not stop");
        };
        assert_eq!(
            (notice.parent, notice.child, notice.uid, notice.change),
            (140, 141, 5, JobChange::Stopped(20))
        );
        assert!(notice.signal);
        assert!(table.is_stopped(141));
        assert!(matches!(table.stop(141, 19, 5), Stopping::Already));
        assert!(matches!(table.stop(999, 19, 5), Stopping::Gone));
        assert_eq!(
            table.wait(140, Selector::Any, ChildKind::Sigchld, Ends::Reap),
            Found::Running,
            "a wait without WUNTRACED does not see the stop"
        );
        let looked = JobReports {
            keep: true,
            ..STOPPED_AND_CONTINUED
        };
        for reports in [looked, STOPPED_AND_CONTINUED] {
            assert_eq!(
                table.wait_for_change(140, Selector::Any, ChildKind::Sigchld, Ends::Reap, reports),
                Found::Stopped {
                    id: 141,
                    uid: 5,
                    signal: 20
                }
            );
        }
        assert_eq!(
            table.wait_for_change(
                140,
                Selector::Any,
                ChildKind::Sigchld,
                Ends::Reap,
                STOPPED_AND_CONTINUED
            ),
            Found::Running,
            "a stop is reported once"
        );
        let notice = table.resume(141).unwrap();
        assert_eq!(notice.change, JobChange::Continued);
        assert!(!table.is_stopped(141));
        assert!(table.resume(141).is_none());
        assert_eq!(
            table.wait_for_change(
                140,
                Selector::Process(141),
                ChildKind::Sigchld,
                Ends::Reap,
                STOPPED_AND_CONTINUED
            ),
            Found::Continued { id: 141, uid: 5 }
        );
        table.set_no_stop_notices(140, true);
        let Stopping::Stopped(Some(quiet)) = table.stop(141, 19, 5) else {
            panic!("the child did not stop again");
        };
        assert!(!quiet.signal, "SA_NOCLDSTOP: woken, but no SIGCHLD");
        table.end(141, ProcessExit::Killed { signal: 9 }, 5);
        assert_eq!(table.stopped, 0);
        assert!(matches!(
            table.wait_for_change(
                140,
                Selector::Any,
                ChildKind::Sigchld,
                Ends::Reap,
                STOPPED_AND_CONTINUED
            ),
            Found::Ended { id: 141, .. }
        ));
    }

    #[test]
    fn a_group_is_orphaned_when_no_member_has_a_parent_elsewhere_in_its_session() {
        let mut table = table_with(&[(150, None), (151, Some(150))]);
        table.set_group(150, 151, 0).unwrap();
        assert!(
            !table.is_in_orphaned_group(151),
            "151's parent is in another group of the session"
        );
        assert!(
            table.is_in_orphaned_group(150),
            "the session leader's group has no parent in the session"
        );
        table.end(150, EXITED, 0);
        assert!(table.is_in_orphaned_group(151), "its parent ended");
        assert!(!table.is_in_orphaned_group(999));
    }

    #[test]
    fn the_end_of_a_process_hangs_up_a_group_it_leaves_orphaned_with_stopped_members() {
        let mut table = table_with(&[(160, None), (161, Some(160)), (162, Some(160))]);
        table.set_group(160, 161, 0).unwrap();
        table.set_group(160, 162, 0).unwrap();
        table.set_running(161, running());
        table.set_running(162, running());
        assert!(matches!(table.stop(161, 20, 0), Stopping::Stopped(_)));
        let ended = table.end(160, EXITED, 0);
        assert_eq!(
            ended.hung_up.len(),
            1,
            "only the group with a stopped member: 161's, not 162's"
        );
        let mut quiet = table_with(&[(170, None), (171, Some(170))]);
        quiet.set_group(170, 171, 0).unwrap();
        quiet.set_running(171, running());
        assert!(quiet.end(170, EXITED, 0).hung_up.is_empty());
    }

    #[test]
    fn a_background_group_of_the_console_s_session_is_told_apart() {
        let mut table = table_with(&[(180, None), (181, Some(180)), (190, None)]);
        table.give_console_if_free(180);
        assert_eq!(table.background_on_console(180), None);
        table.set_group(180, 181, 0).unwrap();
        assert_eq!(table.background_on_console(181), Some((181, false)));
        assert_eq!(table.background_on_console(190), None, "another session");
        table.set_console_foreground(180, 181).unwrap();
        assert_eq!(table.background_on_console(181), None);
        assert_eq!(table.background_on_console(180), Some((180, true)));
        table.release_console();
    }

    #[test]
    fn signals_find_processes_and_groups_zombies_included() {
        let mut table = table_with(&[(120, None), (121, Some(120)), (122, Some(120)), (130, None)]);
        for id in [121, 122] {
            table.set_running(
                id,
                Running {
                    process: Weak::<()>::new(),
                    first_thread: Arc::new(WaitSlot::new()),
                    files: WeakDescriptors::default(),
                },
            );
        }
        table.set_init(120);
        assert_eq!(table.init(), 120);
        assert!(table.target(121).unwrap().running.is_some());
        assert!(table.target(999).is_none());
        let ids = |targets: Vec<Target>| -> Vec<u32> { targets.iter().map(|t| t.id).collect() };
        assert_eq!(ids(table.group_targets(120)), [120, 121, 122]);
        assert_eq!(ids(table.group_targets(130)), [130]);
        assert!(table.group_targets(0).is_empty());
        assert!(table.group_targets(999).is_empty());
        table.set_group(120, 122, 0).unwrap();
        assert_eq!(ids(table.group_targets(122)), [122]);
        table.end(122, EXITED, 0);
        let zombie = table.target(122).unwrap();
        assert!(zombie.running.is_none());
        let zombies = table.group_targets(122);
        assert_eq!(ids(zombies.clone()), [122]);
        assert!(zombies.iter().all(|target| target.running.is_none()));
    }

    #[test]
    fn minus_one_finds_every_process_but_the_caller_and_init() {
        let mut table = table_with(&[(1, None), (5, Some(1)), (6, Some(5)), (7, Some(1))]);
        table.set_init(1);
        table.end(7, EXITED, 0);
        let ids = |targets: Vec<Target>| -> Vec<u32> { targets.iter().map(|t| t.id).collect() };
        assert_eq!(ids(table.every_target_but(5)), [6, 7]);
        assert_eq!(ids(table.every_target_but(1)), [5, 6, 7]);
        let mut alone = table_with(&[(1, None)]);
        alone.set_init(1);
        assert!(alone.every_target_but(1).is_empty());
    }

    #[test]
    fn a_group_and_session_hold_their_id_after_their_leader_is_reaped() {
        let mut table = table_with(&[(1, None), (60, Some(1))]);
        table.set_init(1);
        assert_eq!(table.new_session(60), Ok(60));
        table.add(61, Some(60)).unwrap();
        table.set_exit_signal(61, SIGCHLD);
        table.set_running(61, running());
        table.end(60, EXITED, 0);
        assert_eq!(
            table.wait(1, Selector::Process(60), ChildKind::Sigchld, Ends::Reap),
            Found::Ended {
                id: 60,
                uid: 0,
                exit: EXITED
            }
        );
        assert!(table.target(60).is_none());
        let mut ids = Vec::new();
        let running = table.ids_in_use(&mut ids).unwrap();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids, [0, 1, 60, 61]);
        assert_eq!(running.len(), 1);
    }

    #[test]
    fn a_reused_id_below_the_newest_takes_its_sorted_place_once() {
        let mut table = table_with(&[(1, None), (500, Some(1))]);
        table.add(300, Some(1)).unwrap();
        assert_eq!(table.parent(300), 1);
        let listed: Vec<u32> = table.list().map(|listed| listed.id).collect();
        assert_eq!(listed, [1, 300, 500]);
        assert_eq!(table.add(300, Some(1)), Err(Errno::EAGAIN));
        assert_eq!(table.add(500, None), Err(Errno::EAGAIN));
    }
}
