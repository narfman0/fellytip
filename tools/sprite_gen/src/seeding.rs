//! Deterministic seed derivation for sprite generation.
//!
//! DALL-E 3 doesn't accept an explicit seed parameter, but folding a stable
//! seed into the prompt string nudges the backend toward repeatable output
//! and gives us a reproducible identifier to cite in the atlas manifest.
//! Stable-Diffusion backends accept the seed directly.

pub fn frame_seed(entity_id: &str, direction: u32, frame: u32) -> u64 {
    // FNV-1a (64-bit) of `entity_id|direction|frame`.
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME:  u64 = 0x100000001b3;
    let mut h = OFFSET;
    for b in entity_id.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h ^= direction as u64;
    h = h.wrapping_mul(PRIME);
    h ^= frame as u64;
    h.wrapping_mul(PRIME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_are_stable_and_distinct() {
        assert_eq!(frame_seed("x", 0, 0), frame_seed("x", 0, 0));
        assert_ne!(frame_seed("x", 0, 0), frame_seed("x", 1, 0));
        assert_ne!(frame_seed("x", 0, 0), frame_seed("x", 0, 1));
        assert_ne!(frame_seed("x", 0, 0), frame_seed("y", 0, 0));
    }
}
