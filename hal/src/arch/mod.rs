//! One module per CPU architecture. Each is compiled only for its own bare-metal target
//! (`target_os = "none"`), so host builds and tests never see architecture code.
//!
//! An architecture module provides the boot entry point, its linker script and a type
//! implementing [`crate::Platform`], re-exported as `CurrentPlatform`.
