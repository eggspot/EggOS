//! Android packages, read (M15, D450): an APK's binary `AndroidManifest.xml` ([`manifest`])
//! and its `classes*.dex` files ([`dex`]), from the formats the Android Open Source Project
//! documents, and its methods' instructions decoded ([`dalvik`], D471). What the Android layer
//! needs to install an app and compile its code ahead of time; nothing here runs it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod dalvik;
pub mod dex;
pub mod manifest;
