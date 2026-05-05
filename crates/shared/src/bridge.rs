use std::collections::VecDeque;

use bevy::prelude::*;

use crate::combat::interrupt::InterruptStack;
use crate::combat::types::{CharacterClass, CombatantId};
use crate::inputs::ActionIntent;

#[derive(Resource, Default)]
pub struct LocalPlayerInput {
    pub actions: Vec<(Option<ActionIntent>, Option<uuid::Uuid>)>,
}

#[derive(Component)]
pub struct CombatParticipant {
    pub id: CombatantId,
    pub interrupt_stack: InterruptStack,
    pub class: CharacterClass,
    pub level: u32,
    pub armor_class: i32,
    pub strength: i32,
    pub dexterity: i32,
    pub constitution: i32,
    pub intelligence: i32,
    pub wisdom: i32,
    pub charisma: i32,
}

#[derive(Component)]
pub struct ExperienceReward(pub u32);

pub const HOST_FRAME_FLOOR_SECS: f32 = 1.0 / 30.0;

#[derive(Resource, Default)]
pub struct ClientFrameTimings {
    samples: VecDeque<f32>,
    pub under_pressure: bool,
}

impl ClientFrameTimings {
    pub fn push(&mut self, delta_secs: f32) {
        if self.samples.len() >= 60 {
            self.samples.pop_front();
        }
        self.samples.push_back(delta_secs);
        let avg = self.samples.iter().sum::<f32>() / self.samples.len() as f32;
        self.under_pressure = avg > HOST_FRAME_FLOOR_SECS;
    }

    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
}
