//! Library surface for the sprite_gen CLI.  Exposed as a library so unit
//! tests can exercise layout / assembler / manifest in isolation.

pub mod assembler;
pub mod generator;
pub mod incremental;
pub mod layout;
pub mod manifest;
