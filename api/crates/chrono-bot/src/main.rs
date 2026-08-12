//! chrono-bot control plane. Wires Layer 1 interfaces onto the Layer 2 core,
//! which drives kernels through Layer 3 adapters.

use layer1_public::{
    a2a::A2aInterface, agui::AguiInterface, mcp::McpInterface, pty::PtyInterface,
    rest::RestInterface, InterfaceProfile,
};

fn interfaces() -> Vec<Box<dyn InterfaceProfile>> {
    vec![
        Box::new(AguiInterface),
        Box::new(RestInterface),
        Box::new(PtyInterface),
        Box::new(McpInterface),
        Box::new(A2aInterface),
    ]
}

#[tokio::main]
async fn main() {
    for iface in interfaces() {
        println!(
            "interface {} projects {} operations",
            iface.interface_id(),
            iface.operations().len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_interface_has_a_unique_id() {
        let ids: Vec<_> = interfaces().iter().map(|i| i.interface_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "interface ids must be unique");
    }

    #[test]
    fn every_interface_projects_at_least_one_operation() {
        for iface in interfaces() {
            assert!(
                !iface.operations().is_empty(),
                "{} projects no operations",
                iface.interface_id()
            );
        }
    }
}
