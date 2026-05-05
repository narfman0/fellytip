//! Homegrown particle effect system for Fellytip.
//!
//! No external dependency — particles are small `Mesh3d` entities with lifetime
//! components that move and fade over time.
//!
//! # Emitter kinds
//! - `Campfire` — continuous orange→grey smoke, 3-5 particles/tick
//! - `Lantern`  — continuous small orange sparks, 1-2 particles/tick
//! - `MeleeHit` — one-shot burst of 8 red particles on damage message
//! - `SpellImpact` — one-shot burst of 12 coloured particles on damage message
//! - `HealEffect` — one-shot burst of 6 rising green motes
//!
//! # Wiring
//! `ParticleEmitter` components are added to campfire/lantern building visuals
//! by `entity_renderer.rs`.  Combat events drive burst emitters via the
//! `ClientDamageMsg` Bevy message, written by the server's `resolve_interrupts`
//! system.

use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use fellytip_shared::protocol::ClientDamageMsg;
use rand::RngExt as _;
use super::settings::ParticlesEnabled;

// ── Components ────────────────────────────────────────────────────────────────

/// A single live particle entity.
#[derive(Component)]
pub struct Particle {
    pub velocity: Vec3,
    pub lifetime: f32,
    pub max_lifetime: f32,
    pub start_color: Color,
    pub end_color: Color,
    pub start_scale: f32,
    pub end_scale: f32,
}

/// Attaches to campfire / lantern building entities; continuously spawns particles.
#[derive(Component)]
pub struct ParticleEmitter {
    pub kind: EmitterKind,
    pub timer: Timer,
}

#[allow(dead_code)]
pub enum EmitterKind {
    Campfire,
    Lantern,
    MeleeHit,
    SpellImpact { color: Color },
    HealEffect,
}

// ── Shared mesh / material asset ──────────────────────────────────────────────

/// Shared tiny sphere mesh used for all particles.
#[derive(Resource)]
pub(crate) struct ParticleAssets {
    mesh: Handle<Mesh>,
}

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct ParticlesPlugin;

impl Plugin for ParticlesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_particle_assets)
            .add_systems(
                Update,
                (
                    tick_particles,
                    tick_emitters,
                    spawn_combat_particles,
                ),
            );
    }
}

fn setup_particle_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let mesh = meshes.add(Sphere::new(0.05));
    commands.insert_resource(ParticleAssets { mesh });
}

// ── Particle tick ─────────────────────────────────────────────────────────────

/// Advance every live particle: move, lerp colour/scale, despawn on expiry.
fn tick_particles(
    mut commands: Commands,
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut query: Query<(
        Entity,
        &mut Particle,
        &mut Transform,
        &MeshMaterial3d<StandardMaterial>,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, mut p, mut transform, mat_handle) in &mut query {
        p.lifetime += dt;
        if p.lifetime >= p.max_lifetime {
            commands.entity(entity).despawn();
            continue;
        }
        let t = (p.lifetime / p.max_lifetime).clamp(0.0, 1.0);

        // Move
        transform.translation += p.velocity * dt;

        // Scale
        let scale = p.start_scale + (p.end_scale - p.start_scale) * t;
        transform.scale = Vec3::splat(scale);

        // Colour / alpha lerp
        if let Some(mat) = materials.get_mut(&mat_handle.0) {
            let sc = p.start_color.to_linear();
            let ec = p.end_color.to_linear();
            let r = sc.red   + (ec.red   - sc.red)   * t;
            let g = sc.green + (ec.green - sc.green) * t;
            let b = sc.blue  + (ec.blue  - sc.blue)  * t;
            let a = sc.alpha + (ec.alpha - sc.alpha) * t;
            mat.base_color = Color::linear_rgba(r, g, b, a);
        }
    }
}

// ── Emitter tick ──────────────────────────────────────────────────────────────

/// Fire continuous emitters (Campfire, Lantern) on their timer.
fn tick_emitters(
    time: Res<Time>,
    mut commands: Commands,
    assets: Option<Res<ParticleAssets>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut query: Query<(&mut ParticleEmitter, &Transform)>,
    particles_enabled: Option<Res<ParticlesEnabled>>,
) {
    // Respect the global particles toggle if the resource exists.
    if let Some(ref enabled) = particles_enabled
        && !enabled.0 {
            return;
        }
    let Some(assets) = assets else { return };
    let mut rng = rand::rng();

    for (mut emitter, transform) in &mut query {
        emitter.timer.tick(time.delta());
        if !emitter.timer.just_finished() {
            continue;
        }

        let origin = transform.translation;

        match &emitter.kind {
            EmitterKind::Campfire => {
                let count = rng.random_range(3..=5);
                for _ in 0..count {
                    let drift_x = rng.random_range(-0.3_f32..0.3);
                    let drift_z = rng.random_range(-0.3_f32..0.3);
                    let vel = Vec3::new(drift_x, rng.random_range(0.8_f32..1.6), drift_z);
                    spawn_particle(
                        &mut commands,
                        &assets,
                        &mut materials,
                        origin + Vec3::new(0.0, 0.3, 0.0),
                        vel,
                        1.5,
                        Color::srgb(1.0, 0.5, 0.05),
                        Color::srgba(0.4, 0.4, 0.4, 0.0),
                        0.05,
                        0.01,
                    );
                }
            }
            EmitterKind::Lantern => {
                let count = rng.random_range(1..=2);
                for _ in 0..count {
                    let drift_x = rng.random_range(-0.1_f32..0.1);
                    let drift_z = rng.random_range(-0.1_f32..0.1);
                    let vel = Vec3::new(drift_x, rng.random_range(0.4_f32..0.8), drift_z);
                    spawn_particle(
                        &mut commands,
                        &assets,
                        &mut materials,
                        origin + Vec3::new(0.0, 0.1, 0.0),
                        vel,
                        0.3,
                        Color::srgb(1.0, 0.65, 0.1),
                        Color::srgba(1.0, 0.4, 0.0, 0.0),
                        0.03,
                        0.005,
                    );
                }
            }
            // Burst emitters are handled via `spawn_combat_particles`
            EmitterKind::MeleeHit | EmitterKind::SpellImpact { .. } | EmitterKind::HealEffect => {}
        }
    }
}

// ── Combat particle spawning ──────────────────────────────────────────────────

/// Respond to `ClientDamageMsg` by spawning a burst of hit particles.
fn spawn_combat_particles(
    mut messages: MessageReader<ClientDamageMsg>,
    mut commands: Commands,
    assets: Option<Res<ParticleAssets>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(assets) = assets else { return };
    let mut rng = rand::rng();

    for msg in messages.read() {
        if msg.is_miss { continue; }
        let origin = Vec3::new(msg.x, msg.y, msg.z);
        if msg.is_spell {
            // SpellImpact: 12 particles, colour by spell type, 0.2 s lifetime
            let color = if let Some([r, g, b, a]) = msg.spell_color {
                Color::linear_rgba(r, g, b, a)
            } else {
                Color::srgb(1.0, 0.5, 0.0) // default fire orange
            };
            let lc = color.to_linear();
            let end_color = Color::linear_rgba(lc.red, lc.green, lc.blue, 0.0);
            for _ in 0..12 {
                let dir = random_unit_vec(&mut rng);
                let vel = dir * rng.random_range(3.0_f32..6.0);
                spawn_particle(
                    &mut commands,
                    &assets,
                    &mut materials,
                    origin,
                    vel,
                    0.2,
                    color,
                    end_color,
                    0.06,
                    0.01,
                );
            }
        } else {
            // MeleeHit: 8 particles, red→transparent, 0.1 s lifetime
            for _ in 0..8 {
                let dir = random_unit_vec(&mut rng);
                let vel = dir * rng.random_range(2.0_f32..4.0);
                spawn_particle(
                    &mut commands,
                    &assets,
                    &mut materials,
                    origin,
                    vel,
                    0.1,
                    Color::srgb(0.9, 0.05, 0.05),
                    Color::srgba(0.9, 0.05, 0.05, 0.0),
                    0.07,
                    0.01,
                );
            }
        }
    }
}

// ── Heal effect helper ────────────────────────────────────────────────────────

#[allow(dead_code)]
pub(crate) fn spawn_heal_effect(
    commands: &mut Commands,
    assets: &ParticleAssets,
    materials: &mut Assets<StandardMaterial>,
    position: Vec3,
) {
    let mut rng = rand::rng();
    for _ in 0..6 {
        let drift_x = rng.random_range(-0.15_f32..0.15);
        let drift_z = rng.random_range(-0.15_f32..0.15);
        let vel = Vec3::new(drift_x, rng.random_range(0.3_f32..0.8), drift_z);
        spawn_particle(
            commands,
            assets,
            materials,
            position,
            vel,
            0.8,
            Color::srgb(0.1, 0.9, 0.2),
            Color::srgba(0.1, 0.9, 0.2, 0.0),
            0.05,
            0.01,
        );
    }
}

// ── Spawn helper ──────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn spawn_particle(
    commands: &mut Commands,
    assets: &ParticleAssets,
    materials: &mut Assets<StandardMaterial>,
    position: Vec3,
    velocity: Vec3,
    max_lifetime: f32,
    start_color: Color,
    end_color: Color,
    start_scale: f32,
    end_scale: f32,
) {
    let sc = start_color.to_linear();
    let mat = materials.add(StandardMaterial {
        base_color: start_color,
        emissive: LinearRgba::new(sc.red * 0.5, sc.green * 0.5, sc.blue * 0.5, 0.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    commands.spawn((
        Particle {
            velocity,
            lifetime: 0.0,
            max_lifetime,
            start_color,
            end_color,
            start_scale,
            end_scale,
        },
        Mesh3d(assets.mesh.clone()),
        MeshMaterial3d(mat),
        Transform::from_translation(position).with_scale(Vec3::splat(start_scale)),
    ));
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn random_unit_vec(rng: &mut impl rand::Rng) -> Vec3 {
    let theta: f32 = rng.random_range(0.0..std::f32::consts::TAU);
    let phi: f32 = rng.random_range(0.0..std::f32::consts::PI);
    Vec3::new(
        phi.sin() * theta.cos(),
        phi.cos(),
        phi.sin() * theta.sin(),
    )
}
