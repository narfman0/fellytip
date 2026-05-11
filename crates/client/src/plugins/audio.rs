//! Client-side audio: SFX one-shots + music loops.
//!
//! Other plugins emit `PlaySfx(SoundId::…)` messages; this plugin holds the
//! preloaded handles and spawns short-lived `AudioPlayer` entities that
//! despawn when playback finishes.
//!
//! Music is owned by a single long-lived entity reused across track swaps via
//! the `SetMusic(Option<MusicTrack>)` message.

use bevy::audio::Volume;
use bevy::ecs::message::{Message, MessageReader};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use rand::seq::IndexedRandom;

/// SFX categories. A single id may map to multiple sample files; the plugin
/// picks one at random per play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SoundId {
    UiClick,
    UiHover,
    UiConfirm,
    UiCancel,
    UiError,
    FootstepGrass,
    FootstepStone,
    FootstepWood,
    FootstepSnow,
    MeleeLight,
    MeleeHeavy,
    ImpactMetal,
    ImpactWood,
    DoorOpen,
    DoorClose,
    PickupCoin,
    PickupLeather,
    VoiceLevelUp,
    VoiceReady,
    VoiceFight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MusicTrack {
    Overworld1,
    Overworld2,
    Overworld3,
    Combat1,
    Combat2,
    Victory,
}

/// One-shot SFX request.
#[derive(Message, Debug, Clone, Copy)]
pub struct PlaySfx(pub SoundId);

/// Swap (or stop) the music loop.
#[derive(Message, Debug, Clone, Copy)]
pub struct SetMusic(pub Option<MusicTrack>);

/// Player-tunable volumes (0.0 – 1.0). Hooked up to the settings menu in a
/// later phase.
#[derive(Resource, Debug, Clone, Copy)]
pub struct AudioSettings {
    pub master: f32,
    pub sfx: f32,
    pub music: f32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self { master: 1.0, sfx: 1.0, music: 0.5 }
    }
}

#[derive(Resource, Default)]
struct SfxLibrary {
    samples: HashMap<SoundId, Vec<Handle<AudioSource>>>,
}

#[derive(Resource, Default)]
struct MusicLibrary {
    tracks: HashMap<MusicTrack, Handle<AudioSource>>,
}

/// Tag for the entity currently playing music.
#[derive(Component)]
struct MusicEntity;

pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AudioSettings>()
            .init_resource::<SfxLibrary>()
            .init_resource::<MusicLibrary>()
            .add_message::<PlaySfx>()
            .add_message::<SetMusic>()
            .add_systems(Startup, load_audio_assets)
            .add_systems(Update, (play_sfx, swap_music));
    }
}

fn load_set(server: &AssetServer, paths: &[&'static str]) -> Vec<Handle<AudioSource>> {
    paths.iter().map(|p| server.load::<AudioSource>(*p)).collect()
}

fn load_audio_assets(
    server: Res<AssetServer>,
    mut sfx: ResMut<SfxLibrary>,
    mut music: ResMut<MusicLibrary>,
) {
    // UI
    sfx.samples.insert(SoundId::UiClick, load_set(&server, &[
        "audio/ui/click_01.ogg",
        "audio/ui/click_02.ogg",
        "audio/ui/click_03.ogg",
    ]));
    sfx.samples.insert(SoundId::UiHover, load_set(&server, &[
        "audio/ui/hover_01.ogg",
        "audio/ui/hover_02.ogg",
    ]));
    sfx.samples.insert(SoundId::UiConfirm, load_set(&server, &["audio/ui/confirm.ogg"]));
    sfx.samples.insert(SoundId::UiCancel,  load_set(&server, &["audio/ui/cancel.ogg"]));
    sfx.samples.insert(SoundId::UiError,   load_set(&server, &["audio/ui/error.ogg"]));

    // Footsteps
    sfx.samples.insert(SoundId::FootstepGrass, load_set(&server, &[
        "audio/footstep/grass_01.ogg", "audio/footstep/grass_02.ogg",
        "audio/footstep/grass_03.ogg", "audio/footstep/grass_04.ogg",
    ]));
    sfx.samples.insert(SoundId::FootstepStone, load_set(&server, &[
        "audio/footstep/stone_01.ogg", "audio/footstep/stone_02.ogg",
        "audio/footstep/stone_03.ogg", "audio/footstep/stone_04.ogg",
    ]));
    sfx.samples.insert(SoundId::FootstepWood, load_set(&server, &[
        "audio/footstep/wood_01.ogg", "audio/footstep/wood_02.ogg",
        "audio/footstep/wood_03.ogg", "audio/footstep/wood_04.ogg",
    ]));
    sfx.samples.insert(SoundId::FootstepSnow, load_set(&server, &[
        "audio/footstep/snow_01.ogg", "audio/footstep/snow_02.ogg",
        "audio/footstep/snow_03.ogg", "audio/footstep/snow_04.ogg",
    ]));

    // Impacts
    sfx.samples.insert(SoundId::MeleeLight, load_set(&server, &[
        "audio/impact/melee_light_01.ogg",
        "audio/impact/melee_light_02.ogg",
        "audio/impact/melee_light_03.ogg",
    ]));
    sfx.samples.insert(SoundId::MeleeHeavy, load_set(&server, &[
        "audio/impact/melee_heavy_01.ogg",
        "audio/impact/melee_heavy_02.ogg",
        "audio/impact/melee_heavy_03.ogg",
    ]));
    sfx.samples.insert(SoundId::ImpactMetal, load_set(&server, &[
        "audio/impact/metal_01.ogg",
        "audio/impact/metal_02.ogg",
        "audio/impact/metal_03.ogg",
    ]));
    sfx.samples.insert(SoundId::ImpactWood, load_set(&server, &[
        "audio/impact/wood_01.ogg",
        "audio/impact/wood_02.ogg",
        "audio/impact/wood_03.ogg",
    ]));

    // Doors / pickups
    sfx.samples.insert(SoundId::DoorOpen, load_set(&server, &[
        "audio/door/open_01.ogg", "audio/door/open_02.ogg",
    ]));
    sfx.samples.insert(SoundId::DoorClose, load_set(&server, &[
        "audio/door/close_01.ogg", "audio/door/close_02.ogg",
    ]));
    sfx.samples.insert(SoundId::PickupCoin,    load_set(&server, &["audio/pickup/coin.ogg"]));
    sfx.samples.insert(SoundId::PickupLeather, load_set(&server, &["audio/pickup/leather.ogg"]));

    // Voice
    sfx.samples.insert(SoundId::VoiceLevelUp, load_set(&server, &["audio/voice/level_up.ogg"]));
    sfx.samples.insert(SoundId::VoiceReady,   load_set(&server, &["audio/voice/ready.ogg"]));
    sfx.samples.insert(SoundId::VoiceFight,   load_set(&server, &["audio/voice/fight.ogg"]));

    // Music
    music.tracks.insert(MusicTrack::Overworld1, server.load("audio/music/overworld_01.ogg"));
    music.tracks.insert(MusicTrack::Overworld2, server.load("audio/music/overworld_02.ogg"));
    music.tracks.insert(MusicTrack::Overworld3, server.load("audio/music/overworld_03.ogg"));
    music.tracks.insert(MusicTrack::Combat1,    server.load("audio/music/combat_01.ogg"));
    music.tracks.insert(MusicTrack::Combat2,    server.load("audio/music/combat_02.ogg"));
    music.tracks.insert(MusicTrack::Victory,    server.load("audio/music/victory.ogg"));
}

fn play_sfx(
    mut reader: MessageReader<PlaySfx>,
    library: Res<SfxLibrary>,
    settings: Res<AudioSettings>,
    mut commands: Commands,
) {
    let volume = (settings.master * settings.sfx).clamp(0.0, 1.0);
    let mut rng = rand::rng();
    for PlaySfx(id) in reader.read() {
        let Some(samples) = library.samples.get(id) else {
            warn!(?id, "no samples registered for SoundId");
            continue;
        };
        let Some(handle) = samples.choose(&mut rng) else { continue };
        commands.spawn((
            AudioPlayer(handle.clone()),
            PlaybackSettings::DESPAWN.with_volume(Volume::Linear(volume)),
        ));
    }
}

fn swap_music(
    mut reader: MessageReader<SetMusic>,
    library: Res<MusicLibrary>,
    settings: Res<AudioSettings>,
    existing: Query<Entity, With<MusicEntity>>,
    mut commands: Commands,
) {
    let Some(SetMusic(target)) = reader.read().last().copied() else { return };

    for e in &existing {
        commands.entity(e).despawn();
    }

    let Some(track) = target else { return };
    let Some(handle) = library.tracks.get(&track) else {
        warn!(?track, "no handle for music track");
        return;
    };
    let volume = (settings.master * settings.music).clamp(0.0, 1.0);
    commands.spawn((
        AudioPlayer(handle.clone()),
        PlaybackSettings::LOOP.with_volume(Volume::Linear(volume)),
        MusicEntity,
    ));
}
