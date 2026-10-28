//! The built-in MCP server's place in init (D136): init starts `eggmcpd` only when the agent
//! channel (`/dev/agent`) and the server both exist, so a machine without the channel runs and
//! holds nothing for it (D18). When the server ends, init starts it again with `--restarted`,
//! unless it ended [`MAX_QUICK_ENDS`] times in a row within [`QUICK_END_MS`] of starting.

/// The device whose presence starts the server, with its terminating NUL.
pub const AGENT_CHANNEL: &[u8] = b"/dev/agent\0";

/// The server, with its terminating NUL.
pub const AGENT_SERVER: &[u8] = b"/usr/libexec/eggmcpd\0";

/// The argument a server started again gets, with its terminating NUL.
pub const RESTARTED_ARGUMENT: &[u8] = b"--restarted\0";

/// A run shorter than this counts as a quick end.
pub const QUICK_END_MS: u64 = 5000;

/// How many quick ends in a row make init give up.
pub const MAX_QUICK_ENDS: u32 = 3;

/// What init does after the server ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterEnd {
    /// Start it again, with `--restarted`.
    Restart,
    /// It keeps ending: leave the channel closed.
    GiveUp,
}

/// The server's process, as init follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AgentServer {
    process: Option<usize>,
    started_ms: u64,
    quick_ends: u32,
}

impl AgentServer {
    /// Notes that the server started as `process` at `now_ms`.
    pub fn started(&mut self, process: usize, now_ms: u64) {
        self.process = Some(process);
        self.started_ms = now_ms;
    }

    /// What to do now that `process` ended at `now_ms`; `None` if it is not the server.
    pub fn ended(&mut self, process: usize, now_ms: u64) -> Option<AfterEnd> {
        if self.process != Some(process) {
            return None;
        }
        self.process = None;
        match now_ms.saturating_sub(self.started_ms) < QUICK_END_MS {
            true => self.quick_ends += 1,
            false => self.quick_ends = 0,
        }
        Some(match self.quick_ends >= MAX_QUICK_ENDS {
            true => AfterEnd::GiveUp,
            false => AfterEnd::Restart,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_paths_and_the_argument_are_c_strings() {
        for text in [AGENT_CHANNEL, AGENT_SERVER, RESTARTED_ARGUMENT] {
            assert_eq!(
                text.iter().position(|&byte| byte == 0),
                Some(text.len() - 1)
            );
        }
    }

    #[test]
    fn other_processes_ending_change_nothing() {
        let mut server = AgentServer::default();
        assert_eq!(server.ended(7, 0), None);
        server.started(7, 0);
        assert_eq!(server.ended(8, 10), None);
        assert_eq!(server.ended(7, 10_000), Some(AfterEnd::Restart));
        assert_eq!(server.ended(7, 10_001), None);
    }

    #[test]
    fn a_server_that_ran_a_while_is_always_started_again() {
        let mut server = AgentServer::default();
        let mut now = 0;
        for process in 1..10 {
            server.started(process, now);
            now += QUICK_END_MS;
            assert_eq!(server.ended(process, now), Some(AfterEnd::Restart));
        }
    }

    #[test]
    fn three_quick_ends_in_a_row_give_up_and_a_long_run_forgives() {
        let mut server = AgentServer::default();
        server.started(1, 0);
        assert_eq!(server.ended(1, 10), Some(AfterEnd::Restart));
        server.started(2, 10);
        assert_eq!(server.ended(2, 20), Some(AfterEnd::Restart));
        server.started(3, 20);
        assert_eq!(server.ended(3, 20 + QUICK_END_MS), Some(AfterEnd::Restart));
        server.started(4, 100_000);
        assert_eq!(server.ended(4, 100_001), Some(AfterEnd::Restart));
        server.started(5, 100_001);
        assert_eq!(server.ended(5, 100_002), Some(AfterEnd::Restart));
        server.started(6, 100_002);
        assert_eq!(server.ended(6, 100_003), Some(AfterEnd::GiveUp));
        assert_eq!(server.ended(6, 100_004), None);
    }
}
