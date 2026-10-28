//! EggOS's built-in MCP server (D21, D136): `eggmcpd` speaks the Model Context Protocol
//! (JSON-RPC 2.0, one message per line) on the agent channel, `/dev/agent`, which a host's MCP
//! client reaches through `run-eggos.sh --mcp`. Each tool runs its CLI twin, the command a user
//! types for the same thing (D22), and every call goes to the audit log, which `agentlog`
//! shows. Everything but the system calls is host-tested against a fake machine.

#![cfg_attr(not(test), no_std)]

pub mod audit;
#[cfg(test)]
mod fake;
pub mod framing;
#[cfg(any(test, feature = "fuzz"))]
pub mod fuzz;
pub mod json;
#[cfg(target_env = "musl")]
pub mod linux;
pub mod paths;
pub mod rpc;
pub mod server;
pub mod session;
pub mod system;
pub mod tools;
