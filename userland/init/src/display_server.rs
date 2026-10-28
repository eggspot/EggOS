//! The Wayland server's place in init (D140): init starts `eggway` when the server and a
//! display (`/dev/window`, which does not open without one) both exist, so a headless machine
//! runs nothing for it (D18), and starts it again when it ends, as it does the MCP server.

/// The window server's device, whose opening says a display exists, with its terminating NUL.
pub const DISPLAY_DEVICE: &[u8] = b"/dev/window\0";

/// The Wayland server, with its terminating NUL.
pub const DISPLAY_SERVER: &[u8] = b"/usr/libexec/eggway\0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_paths_are_c_strings() {
        for text in [DISPLAY_DEVICE, DISPLAY_SERVER] {
            assert_eq!(
                text.iter().position(|&byte| byte == 0),
                Some(text.len() - 1)
            );
        }
    }
}
