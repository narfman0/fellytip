//! Ecology plugin: runs `tick_ecology` on all regions each WorldSimSchedule tick.

use bevy::prelude::*;
use fellytip_shared::world::ecology::{EcologyEvent, RegionEcology, tick_ecology};

use crate::plugins::world_sim::WorldSimSchedule;

/// Bevy resource holding all region ecologies.
#[derive(Resource, Default)]
pub struct EcologyState {
    pub regions: Vec<RegionEcology>,
}

pub struct EcologyPlugin;

impl Plugin for EcologyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EcologyState>();
        app.add_systems(WorldSimSchedule, run_ecology_tick);
    }
}

fn run_ecology_tick(mut state: ResMut<EcologyState>, tick: Res<crate::plugins::world_sim::WorldSimTick>) {
    let regions = std::mem::take(&mut state.regions);
    state.regions = regions
        .into_iter()
        .flat_map(|region| {
            let (next, events) = tick_ecology(region);
            for ev in events {
                match ev {
                    EcologyEvent::Collapse { species, region } => {
                        tracing::warn!(
                            tick = tick.0,
                            "Ecology collapse: {:?} in {:?}",
                            species.0,
                            region.0
                        );
                    }
                    EcologyEvent::Recovery { species, region } => {
                        tracing::info!(
                            tick = tick.0,
                            "Ecology recovery: {:?} in {:?}",
                            species.0,
                            region.0
                        );
                    }
                }
            }
            Some(next)
        })
        .collect();
}
