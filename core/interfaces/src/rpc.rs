use infusion::{c};

use crate::{
    common::WithStartAndShutdown, config::ConfigConsumer, consensus::MempoolSocket,
    infu_collection::Collection, ApplicationInterface, ConfigProviderInterface, ConsensusInterface,
};

/// The interface for the *RPC* server. Which is supposed to be opening a public
/// port (possibly an HTTP server) and accepts queries or updates from the user.
#[infusion::service]
pub trait RpcInterface<C: Collection>: Sized + Send + Sync + ConfigConsumer + WithStartAndShutdown {
    fn _init(
        config: ::ConfigProviderInterface,
        consensus: ::ConsensusInterface,
        app: ::ApplicationInterface,
    ) {
        Self::init(config.get::<Self>(), consensus.mempool(), app.sync_query())
    }

    /// Initialize the RPC-server, with the given parameters.
    fn init(
        config: Self::Config,
        mempool: MempoolSocket,
        query_runner: c!(C::ApplicationInterface::SyncExecutor),
    ) -> anyhow::Result<Self>;

    #[cfg(feature = "e2e-test")]
    fn provide_dht_socket(&self, dht_socket: crate::dht::DhtSocket);
}
