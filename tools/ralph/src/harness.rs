//! Process lifecycle harness for self-contained ralph scenarios.
//!
//! `TestHarness` spawns a headless `fellytip-client` process and blocks until
//! BRP responds, then kills the process when dropped.  The 120-second timeout
//! handles cold `cargo run` builds.

use anyhow::{Result, bail};
use std::{
    process::{Child, Command},
    thread::sleep,
    time::{Duration, Instant},
};

use crate::brp::BrpClient;

const BRP_STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(500);

pub struct TestHarness {
    process: Child,
}

impl TestHarness {
    /// Spawns `cargo run -p fellytip-client -- --headless [extra_args]`
    /// and blocks until BRP responds to ping (up to 120 s for build + startup).
    pub fn start(extra_args: &[&str]) -> Result<Self> {
        let mut cmd = Command::new("cargo");
        cmd.args(["run", "-p", "fellytip-client", "--", "--headless"]);
        for arg in extra_args {
            cmd.arg(arg);
        }
        let process = cmd.spawn()?;

        let client = BrpClient::server();
        let deadline = Instant::now() + BRP_STARTUP_TIMEOUT;
        tracing::info!("TestHarness: waiting for BRP at localhost:15702 (up to 120 s) …");
        loop {
            if client.ping() {
                tracing::info!("TestHarness: server is up");
                return Ok(Self { process });
            }
            if Instant::now() > deadline {
                bail!("TestHarness: server BRP not reachable within {BRP_STARTUP_TIMEOUT:?}");
            }
            sleep(POLL);
        }
    }
}

impl Drop for TestHarness {
    fn drop(&mut self) {
        self.process.kill().ok();
        self.process.wait().ok();
    }
}
