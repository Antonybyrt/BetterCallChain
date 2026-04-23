use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct VisualizerConfig {
    pub bind:             SocketAddr,
    pub container_prefix: String,
    pub node_count:       usize,
    pub node_ports:       Vec<u16>,
}

impl VisualizerConfig {
    pub fn node_url(&self, index: usize) -> String {
        let port = self.node_ports.get(index).copied().unwrap_or(8081 + index as u16);
        format!("http://127.0.0.1:{}", port)
    }

    pub fn container_name(&self, index: usize) -> String {
        format!("{}{}", self.container_prefix, index + 1)
    }

    pub fn node_name(&self, index: usize) -> String {
        format!("node{}", index + 1)
    }
}
