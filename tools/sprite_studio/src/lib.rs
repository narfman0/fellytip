//! Library surface for the sprite_studio.  Exposed as a library so unit
//! tests can exercise layout / assembler / manifest in isolation.

pub mod assembler;
pub mod generator;
pub mod incremental;
pub mod layout;
pub mod manifest;
pub mod openai;
pub mod palette;
pub mod postprocess;
pub mod stability;
pub mod parallel;
pub mod seeding;
pub mod studio;
