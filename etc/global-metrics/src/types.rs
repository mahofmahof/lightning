use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Prometheus HTTP service discovery chunk.
/// Targets are expected to provide a `/metrics` endpoint
#[derive(Serialize, Deserialize, Debug)]
pub struct PrometheusDiscoveryChunk {
    targets: Vec<String>,
    labels: HashMap<String, String>,
}

impl PrometheusDiscoveryChunk {
    pub(crate) fn new(targets: Vec<String>, labels: HashMap<String, String>) -> Self {
        Self { targets, labels }
    }

    pub fn get_targets(&self) -> &Vec<String> {
        &self.targets
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcResponse<T: Serialize> {
    pub jsonrpc: String,
    pub result: T,
    pub id: u16,
}
