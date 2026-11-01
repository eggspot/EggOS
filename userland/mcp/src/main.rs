//! `eggmcpd`, EggOS's built-in MCP server (D136): init starts it when the agent channel
//! (`/dev/agent`) exists, as `eggmcpd --restarted` when it starts it again.

#![cfg_attr(eggos_user, no_std, no_main)]

#[cfg(eggos_user)]
eggos_runtime::main!(eggos_mcp::linux::run);

#[cfg(not(eggos_user))]
fn main() {}
