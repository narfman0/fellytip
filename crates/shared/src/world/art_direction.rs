//! Per-world art direction: terrain tints, emissive atmosphere, and building colour palettes.
//!
//! `WorldArtDirection` is a Bevy resource (inserted at startup) that maps each
//! `WorldId` inner value to an `ArtStyle`.  The client reads it in
//! `entity_renderer::spawn_hollow_tower` (building colours) and
//! `terrain/material::biome_color_tinted` (terrain tint).

use bevy::prelude::Resource;
use std::collections::HashMap;

/// Per-world visual style parameters.
#[derive(Clone, Debug)]
pub struct ArtStyle {
    /// RGB multiplier applied on top of the raw biome colour (1,1,1 = no change).
    pub terrain_tint: [f32; 3],
    /// Emissive intensity (0.0 = off, 1.0 = full).
    pub emissive_strength: f32,
    /// RGB colour of emissive glow.
    pub emissive_color: [f32; 3],
    /// Base colour for procedural tower walls.
    pub building_wall_color: [f32; 3],
    /// Base colour for procedural tower roofs / floor slabs.
    pub building_roof_color: [f32; 3],
}

/// Bevy resource — keyed by `WorldId` inner `u32`.
#[derive(Resource, Clone, Debug)]
pub struct WorldArtDirection(pub HashMap<u32, ArtStyle>);

impl Default for WorldArtDirection {
    fn default() -> Self {
        let mut map = HashMap::new();

        // WorldId(0) — Surface: neutral, realistic 2.5-D
        map.insert(
            0,
            ArtStyle {
                terrain_tint:       [1.0, 1.0, 1.0],
                emissive_strength:  0.0,
                emissive_color:     [0.0, 0.0, 0.0],
                building_wall_color: [0.55, 0.50, 0.45],
                building_roof_color: [0.30, 0.28, 0.25],
            },
        );

        // WorldId(1) — Sunken Realm: neon cyberpunk / bioluminescent
        map.insert(
            1,
            ArtStyle {
                terrain_tint:       [0.6, 0.6, 0.8],
                emissive_strength:  0.3,
                emissive_color:     [0.0, 0.8, 1.0],
                building_wall_color: [0.15, 0.15, 0.25],
                building_roof_color: [0.05, 0.05, 0.15],
            },
        );

        // WorldId(2) — Mycelium: spore-green painterly
        map.insert(
            2,
            ArtStyle {
                terrain_tint:       [0.7, 1.0, 0.6],
                emissive_strength:  0.15,
                emissive_color:     [0.3, 0.8, 0.1],
                building_wall_color: [0.35, 0.55, 0.25],
                building_roof_color: [0.20, 0.40, 0.10],
            },
        );

        Self(map)
    }
}

impl WorldArtDirection {
    /// Return the art style for `world_id`, falling back to the Surface style.
    pub fn get(&self, world_id: u32) -> &ArtStyle {
        self.0.get(&world_id)
            .or_else(|| self.0.get(&0))
            .expect("WorldArtDirection must have at least a Surface (0) entry")
    }
}
