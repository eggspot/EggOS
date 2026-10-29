//! EggOS's native GUI toolkit (D132, docs/ux/toolkit-v0.3.md): a retained tree of widgets with
//! layout, labels, push buttons, checkboxes, radio buttons, card pickers, lists, text fields and
//! scroll views, focus and keyboard navigation, hover and pressed states, the design's tokens,
//! damage tracking and scale 1x or 2x, drawn in Egg Sans 16 into a window of the window server
//! (`/dev/window`, docs/windows.md). A program that draws its content itself (Files) uses the
//! same pieces through a view: the canvas, the widgets' painting and text editing, context
//! menus and dialog panels. The widgets live in a fixed array and nothing allocates, so
//! a program pays only for the window it shows; a program links the toolkit as a library. The
//! tree renders into any pixel buffer, so host tests compare whole frames.

#![cfg_attr(not(test), no_std)]

#[cfg(target_env = "musl")]
pub mod application;
pub mod canvas;
pub mod colour;
pub mod damage;
pub mod geometry;
pub mod input;
pub mod menu;
pub mod panel;
pub mod text;
pub mod tree;
pub mod widget;
#[cfg(target_env = "musl")]
pub mod window;

pub use eggos_element_tree as elements;

#[cfg(test)]
mod tests;
