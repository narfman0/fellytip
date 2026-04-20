mod brp;
mod harness;
mod scenarios;

use scenarios::{Scenario, all_scenarios, find_scenario};
use anyhow::{Result, bail};
use std::env;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = env::args().collect();
    let scenario_arg = args
        .windows(2)
        .find(|w| w[0] == "--scenario")
        .map(|w| w[1].as_str())
        .unwrap_or("all");

    if scenario_arg == "all" {
        run_all()
    } else {
        run_one(scenario_arg)
    }
}

fn run_all() -> Result<()> {
    let scenarios = all_scenarios();
    let mut failed = 0usize;
    for scenario in &scenarios {
        tracing::info!("▶ Running scenario: {}", scenario.name());
        match scenario.run() {
            Ok(()) => tracing::info!("  ✓ PASS"),
            Err(e) => {
                tracing::error!("  ✗ FAIL: {e:#}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        bail!("{failed}/{} scenarios failed", scenarios.len());
    }
    Ok(())
}

fn run_one(name: &str) -> Result<()> {
    match find_scenario(name) {
        Some(scenario) => {
            tracing::info!("▶ Running scenario: {}", scenario.name());
            scenario.run()?;
            tracing::info!("  ✓ PASS");
            Ok(())
        }
        None => bail!("Unknown scenario: '{name}'. Available: {}", list_names()),
    }
}

fn list_names() -> String {
    all_scenarios()
        .iter()
        .map(|s| s.name())
        .collect::<Vec<_>>()
        .join(", ")
}
