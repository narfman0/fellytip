pub mod basic_movement;
pub mod combat_resolves;

use anyhow::Result;

pub trait Scenario {
    fn name(&self) -> &str;
    fn run(&self) -> Result<()>;
}

pub fn all_scenarios() -> Vec<Box<dyn Scenario>> {
    vec![
        Box::new(basic_movement::BasicMovement),
        Box::new(combat_resolves::CombatResolves),
    ]
}

pub fn find_scenario(name: &str) -> Option<Box<dyn Scenario>> {
    all_scenarios().into_iter().find(|s| s.name() == name)
}
