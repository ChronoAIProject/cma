//! Layer 4 — sandboxed agent runtime, running in a bot pod.
//!
//! Supervises the agent kernel process and relays its terminal to the control
//! plane. Deployed separately; never linked into the control plane binary.

#[tokio::main]
async fn main() {
    println!("layer4-runtime: bot pod agent runtime");
}
