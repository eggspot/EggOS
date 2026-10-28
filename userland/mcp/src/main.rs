//! `eggmcpd`, EggOS's built-in MCP server (D136): init starts it when the agent channel
//! (`/dev/agent`) exists, as `eggmcpd --restarted` when it starts it again.

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(target_env = "musl")]
eggos_runtime::main!(eggos_mcp::linux::run);

#[cfg(not(target_env = "musl"))]
fn main() {}
