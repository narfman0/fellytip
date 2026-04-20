//! Animation system for player and NPC character models.
//!
//! # Flow
//!
//! 1. `setup_character_assets` loads the four character GLBs and all 14 animation
//!    clip handles at startup.
//! 2. `entity_renderer` spawns `SceneRoot` + `CharacterAnimState` on each
//!    player/NPC entity.
//! 3. When Bevy finishes loading the GLB scene, it adds `AnimationPlayer` to a
//!    child entity (the armature root).  `link_anim_players` detects this via
//!    `Added<AnimationPlayer>`, walks up to find the owning `CharacterAnimState`
//!    entity, builds an `AnimationGraph`, and marks the entity ready.
//! 4. `drive_character_anims` initialises idle on the first frame the link is
//!    established, then blends to walk when the entity moves.
//!
//! # Animation indices
//!
//! Match the order of `ANIMATION_FILES` in `tools/convert_characters.py`:
//! 0 = idle, 1 = walk, 2 = run, 3 = attack, 4 = punch, 5 = kick,
//! 6 = jump, 7 = death, 8 = crouch, 9 = crouchIdle, 10 = crouchWalk,
//! 11 = interactStanding, 12 = interactGround, 13 = shoot.

use std::time::Duration;

use bevy::prelude::*;

pub struct CharacterAnimationPlugin;

impl Plugin for CharacterAnimationPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_character_assets)
            .add_systems(Update, (link_anim_players, drive_character_anims).chain());
    }
}

// ── Animation index constants ─────────────────────────────────────────────────

pub const ANIM_IDLE:   usize = 0;
pub const ANIM_WALK:   usize = 1;
pub const ANIM_RUN:    usize = 2;
#[allow(dead_code)] pub const ANIM_ATTACK: usize = 3;
#[allow(dead_code)] pub const ANIM_DEATH:  usize = 7;

/// Base world-space scale applied to all character models.
pub const CHARACTER_SCALE: f32 = 0.125;

/// Movement speed (world units / second) above which walk replaces idle.
const WALK_THRESHOLD: f32 = 0.3;
/// Movement speed above which run replaces walk.
const RUN_THRESHOLD: f32 = 4.0;

// ── Resources ─────────────────────────────────────────────────────────────────

/// Handles for character GLB scenes and their shared animation clips.
/// Inserted at `Startup`; safe to read in `Update`.
#[derive(Resource)]
pub struct CharacterAssets {
    pub medium_scene:       Handle<Scene>,
    pub large_male_scene:   Handle<Scene>,
    pub large_female_scene: Handle<Scene>,
    /// 14 clips in `ANIM_*` index order.  All models share the same skeleton.
    pub clips: Vec<Handle<AnimationClip>>,
}

// ── Component ─────────────────────────────────────────────────────────────────

/// Per-entity animation state added alongside `SceneRoot` at visual spawn.
#[derive(Component, Default)]
pub struct CharacterAnimState {
    /// The child entity that owns `AnimationPlayer` once the scene finishes loading.
    pub anim_player_entity: Option<Entity>,
    /// Graph node index for each animation, in `ANIM_*` order.
    pub graph_nodes: Vec<AnimationNodeIndex>,
    /// Currently requested animation index.
    pub current_anim: usize,
    /// `false` until the first animation has been started (idle init).
    pub initialized: bool,
    /// Translation last frame — used to compute movement speed.
    pub last_translation: Vec3,
}

// ── Startup ───────────────────────────────────────────────────────────────────

fn setup_character_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
) {
    let scene = |name: &str| {
        asset_server.load(format!("characters/{name}.glb#Scene0"))
    };
    // All animation clips are loaded from the medium GLB; the skeleton is
    // identical across all four character variants.
    let clip = |i: usize| {
        asset_server.load(format!("characters/characterMedium.glb#Animation{i}"))
    };

    commands.insert_resource(CharacterAssets {
        medium_scene:       scene("characterMedium"),
        large_male_scene:   scene("characterLargeMale"),
        large_female_scene: scene("characterLargeFemale"),
        clips: (0..14).map(clip).collect(),
    });
}

// ── Systems ───────────────────────────────────────────────────────────────────

/// Fires when Bevy adds `AnimationPlayer` to a scene hierarchy child.
/// Walks up the parent chain to find the `CharacterAnimState` entity, builds
/// an `AnimationGraph` with all 14 clips, and stores the link.
fn link_anim_players(
    added_players:  Query<Entity, Added<AnimationPlayer>>,
    parents:        Query<&ChildOf>,
    mut states:     Query<&mut CharacterAnimState>,
    mut commands:   Commands,
    char_assets:    Res<CharacterAssets>,
    mut graphs:     ResMut<Assets<AnimationGraph>>,
) {
    'outer: for player_entity in &added_players {
        // Walk up the hierarchy to find the character entity.
        let mut cur = player_entity;
        let char_entity = loop {
            if states.contains(cur) { break cur; }
            match parents.get(cur) {
                Ok(p)  => cur = p.parent(),
                Err(_) => continue 'outer,
            }
        };

        let Ok(mut state) = states.get_mut(char_entity) else { continue };

        // Build a graph with one node per animation clip.
        let mut graph  = AnimationGraph::new();
        let root       = graph.root;
        let nodes: Vec<AnimationNodeIndex> = char_assets
            .clips
            .iter()
            .map(|clip| graph.add_clip(clip.clone(), 1.0, root))
            .collect();

        let graph_handle = graphs.add(graph);

        state.anim_player_entity = Some(player_entity);
        state.graph_nodes        = nodes;
        // Reset so drive_character_anims starts idle on next tick.
        state.initialized        = false;

        commands.entity(player_entity)
            .insert(AnimationGraphHandle(graph_handle))
            .insert(AnimationTransitions::new());
    }
}

/// Drives idle / walk transitions based on character movement speed.
/// Also handles the one-time idle initialisation on first link.
fn drive_character_anims(
    time:           Res<Time>,
    mut char_query: Query<(&mut Transform, &mut CharacterAnimState)>,
    mut transitions: Query<&mut AnimationTransitions>,
    mut players:    Query<&mut AnimationPlayer>,
) {
    let dt = time.delta_secs();

    for (mut transform, mut state) in &mut char_query {
        let Some(player_entity) = state.anim_player_entity else { continue };
        if state.graph_nodes.is_empty() { continue; }

        let delta = transform.translation - state.last_translation;

        // Compute movement speed; avoid division by near-zero dt on first frame.
        let speed = if dt > 0.0 {
            delta.length() / dt
        } else {
            0.0
        };
        state.last_translation = transform.translation;

        // Rotate to face movement direction (horizontal plane only).
        if speed > WALK_THRESHOLD {
            let horizontal = Vec3::new(delta.x, 0.0, delta.z);
            if horizontal.length_squared() > 1e-6 {
                transform.rotation = Quat::from_rotation_arc(Vec3::Z, horizontal.normalize());
            }
        }

        let target = if speed > RUN_THRESHOLD {
            ANIM_RUN
        } else if speed > WALK_THRESHOLD {
            ANIM_WALK
        } else {
            ANIM_IDLE
        };

        if state.initialized && target == state.current_anim { continue; }

        let Some(&node) = state.graph_nodes.get(target) else { continue };

        let Ok(mut trans)  = transitions.get_mut(player_entity) else { continue };
        let Ok(mut player) = players.get_mut(player_entity)     else { continue };

        // No blend on first play (instant), 150 ms blend for subsequent switches.
        let blend_ms = if state.initialized { 150 } else { 0 };
        trans.play(&mut player, node, Duration::from_millis(blend_ms))
            .repeat();

        state.current_anim = target;
        state.initialized  = true;
    }
}

#[cfg(test)]
mod tests {
    use bevy::math::{Quat, Vec3};

    fn facing_rotation(delta: Vec3) -> Option<Quat> {
        let horizontal = Vec3::new(delta.x, 0.0, delta.z);
        if horizontal.length_squared() > 1e-6 {
            Some(Quat::from_rotation_arc(Vec3::Z, horizontal.normalize()))
        } else {
            None
        }
    }

    #[test]
    fn faces_movement_direction() {
        for (delta, expected_forward) in [
            (Vec3::new(1.0, 0.0, 0.0), Vec3::X),
            (Vec3::new(0.0, 0.0, 1.0), Vec3::Z),
            (Vec3::new(-1.0, 0.0, 0.0), Vec3::NEG_X),
            (Vec3::new(0.0, 5.0, 1.0), Vec3::Z), // vertical ignored
        ] {
            let rot = facing_rotation(delta).expect("should produce rotation");
            let forward = rot * Vec3::Z;
            assert!(
                forward.dot(expected_forward) > 0.999,
                "delta {delta:?}: expected forward {expected_forward:?}, got {forward:?}"
            );
        }
    }

    #[test]
    fn no_rotation_when_stationary() {
        assert!(facing_rotation(Vec3::ZERO).is_none());
    }
}
