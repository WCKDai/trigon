//! Exporters. Both are written from the public specifications (Khronos glTF 2.0, buildingSMART
//! IFC4 ADD2 TC1 and ISO 10303-21); no other implementation is linked or read.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod glb;
pub mod ifc;
mod masonry_texture;
