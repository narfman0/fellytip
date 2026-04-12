//! Custom 1 Hz world simulation schedule.
//!
//! `WorldSimSchedule` runs all slow world-state systems: faction AI,
//! ecology, NPC schedule transitions, and story event generation.
//! It is driven by a real-time 1-second `Timer` so it stays decoupled
//! from the 62.5 Hz `FixedUpdate` combat/movement tick.

use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;
use bevy::reflect::Reflect;

/// The custom schedule that world-sim systems are added to.
#[derive(ScheduleLabel, Clone, Debug, Hash, PartialEq, Eq)]
pub struct WorldSimSchedule;

/// Bevy resource that controls when WorldSimSchedule fires.
#[derive(Resource)]
struct WorldSimTimer(Timer);

/// Number of world sim ticks elapsed since the server started.
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct WorldSimTick(pub u64);

pub struct WorldSimPlugin;

impl Plugin for WorldSimPlugin {
    fn build(&self, app: &mut App) {
        app.init_schedule(WorldSimSchedule);
        app.insert_resource(WorldSimTimer(Timer::from_seconds(
            1.0,
            TimerMode::Repeating,
        )));
        app.init_resource::<WorldSimTick>();
        app.register_type::<WorldSimTick>();
        app.add_systems(Update, tick_world_sim);
    }
}

/// Each frame: advance the timer; when it fires, run WorldSimSchedule once.
fn tick_world_sim(world: &mut World) {
    let delta = world.resource::<Time>().delta();
    let fired = {
        let mut timer = world.resource_mut::<WorldSimTimer>();
        timer.0.tick(delta);
        timer.0.just_finished()
    };
    if fired {
        world.resource_mut::<WorldSimTick>().0 += 1;
        world.run_schedule(WorldSimSchedule);
    }
}
