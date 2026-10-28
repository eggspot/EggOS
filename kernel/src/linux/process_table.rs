//! The process table (D105): every process's place in the tree and in job control, from its
//! creation until its parent reaps it. A process has a parent (0 for the kernel), the signal
//! its end sends that parent, a process group and a session; once it ends it is a zombie, its
//! [`ProcessExit`] kept until its parent's `wait4` or `waitid` takes it, or at once if nobody
//! can wait for it (the kernel is its parent, or the parent ignores `SIGCHLD` or set
//! `SA_NOCLDWAIT`). A process that ends hands its children to the nearest ancestor that asked
//! for orphans (`PR_SET_CHILD_SUBREAPER`), else to init, else to the kernel. The table also
//! records which session has the console as its controlling terminal and the console's
//! foreground process group.
//!
//! One lock holds it all, a leaf but for the heap: nothing else is locked under it, and
//! nothing it gives out is a strong reference to a process, so no process is freed under it.
//! A running process's entry holds a weak reference to its current program's process and
//! its first thread's wait slot, through which a signal sent to it from outside reaches it.

use super::Errno;
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
/// what a signal sent from outside the process needs to reach it.
#[derive(Debug, Clone)]
pub struct Running {
    pub process: Weak<dyn Any + Send + Sync>,
    pub first_thread: Arc<WaitSlot>,
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
    executed: bool,
    running: Option<Running>,
    name: Name,
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
    /// The console's foreground group, which loses its controlling terminal as the session's
    /// leader ends: each gets `SIGHUP` and `SIGCONT`.
    pub hung_up: Vec<Running>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Console {
    session: u32,
    foreground: u32,
}

/// See the module documentation.
#[derive(Debug)]
pub struct Table {
    entries: Vec<Entry>,
    init: u32,
    console: Console,
}

impl Table {
    /// No process, no init and the console no session's.
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            init: 0,
            console: Console {
                session: 0,
                foreground: 0,
            },
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
            executed: false,
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
        self.entries.iter().map(|entry| Listed {
            id: entry.id,
            parent: entry.parent,
            group: entry.group,
            session: entry.session,
            ended: entry.exit.is_some(),
            name: entry.name,
        })
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
    /// if it led the session the console belongs to, the console belongs to none from now on.
    /// Returns whom to tell.
    pub fn end(&mut self, id: u32, exit: ProcessExit, uid: u32) -> Ended {
        let mut ended = Ended::default();
        let Some(index) = self.index(id) else {
            return ended;
        };
        if self.entries[index].exit.is_some() {
            return ended;
        }
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
        if session == id && self.console.session == id {
            ended.hung_up = self.members(self.console.foreground);
            self.release_console();
        }
        ended
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
        if self.console.session == id {
            self.release_console();
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

    /// The wait of process `parent` for one of its children that `selector` and `kind` name:
    /// the first of them that has ended, reaped or not as `ends` says; otherwise whether any of
    /// them is running.
    pub fn wait(&mut self, parent: u32, selector: Selector, kind: ChildKind, ends: Ends) -> Found {
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
        let mut ended = None;
        for (index, entry) in self.entries.iter().enumerate() {
            if !matches(entry) {
                continue;
            }
            match entry.exit {
                Some(_) if ends == Ends::Ignore => {}
                Some(exit) => {
                    ended = Some((index, entry.id, entry.uid, exit));
                    break;
                }
                None => running = true,
            }
        }
        match ended {
            Some((index, id, uid, exit)) => {
                if ends == Ends::Reap {
                    self.remove_at(index);
                }
                Found::Ended { id, uid, exit }
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
        if self.console.session != 0 {
            return;
        }
        if let Some(entry) = self.entry(id)
            && entry.session == id
        {
            self.console = Console {
                session: id,
                foreground: entry.group,
            };
        }
    }

    /// The console becomes no session's controlling terminal.
    pub fn release_console(&mut self) {
        self.console = Console {
            session: 0,
            foreground: 0,
        };
    }

    /// The console's session and foreground group, if the console is process `caller`'s
    /// controlling terminal; `ENOTTY` otherwise.
    pub fn controlling_terminal(&self, caller: u32) -> Result<(u32, u32), Errno> {
        let (session, _) = self.session_of(caller).map_err(|_| Errno::ENOTTY)?;
        match self.console.session == session && session != 0 {
            true => Ok((self.console.session, self.console.foreground)),
            false => Err(Errno::ENOTTY),
        }
    }

    /// `TIOCSPGRP` by process `caller`: makes group `group` (a `pid_t`) the console's
    /// foreground. `ENOTTY` unless the console is the caller's controlling terminal; `EINVAL`
    /// for a negative group; `ESRCH` for a group no process is in; `EPERM` for a group of
    /// another session.
    pub fn set_console_foreground(&mut self, caller: u32, group: i32) -> Result<(), Errno> {
        let (session, _) = self.controlling_terminal(caller)?;
        if group < 0 {
            return Err(Errno::EINVAL);
        }
        if !self.entries.iter().any(|entry| entry.group == group as u32) {
            return Err(Errno::ESRCH);
        }
        if !self.group_exists_in(group as u32, session) {
            return Err(Errno::EPERM);
        }
        self.console.foreground = group as u32;
        Ok(())
    }

    /// `TIOCSCTTY` by process `caller`: makes the console its session's controlling terminal,
    /// its group the foreground. As Linux: `EPERM` unless the caller leads its session; then
    /// nothing to do if it is already; `EPERM` if the console is another session's and the
    /// caller may not `steal` it (an argument of 1 and privileges).
    pub fn take_console(&mut self, caller: u32, steal: bool) -> Result<(), Errno> {
        let (session, group) = self.session_of(caller)?;
        if session != caller {
            return Err(Errno::EPERM);
        }
        if self.console.session == session {
            return Ok(());
        }
        if self.console.session != 0 && !steal {
            return Err(Errno::EPERM);
        }
        self.console = Console {
            session,
            foreground: group,
        };
        Ok(())
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

    /// The process orphans go to, which cannot be killed; 0 before there is one.
    pub fn init(&self) -> u32 {
        self.init
    }

    /// The running processes of the console's foreground group.
    pub fn console_foreground_members(&self) -> Vec<Running> {
        self.members(self.console.foreground)
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
    fn signals_find_processes_and_groups_zombies_included() {
        let mut table = table_with(&[(120, None), (121, Some(120)), (122, Some(120)), (130, None)]);
        for id in [121, 122] {
            table.set_running(
                id,
                Running {
                    process: Weak::<()>::new(),
                    first_thread: Arc::new(WaitSlot::new()),
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
}
