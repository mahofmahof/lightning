mod config;
pub mod utils;
use std::{
    collections::VecDeque,
    fs::read_to_string,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
#[cfg(test)]
pub mod tests;

use affair::{Socket, Task};
use anyhow::anyhow;
use async_trait::async_trait;
pub use config::Config;
use fleek_crypto::{
    ConsensusPublicKey, ConsensusSecretKey, NodePublicKey, NodeSecretKey, NodeSignature, SecretKey,
    TransactionSender,
};
use lightning_interfaces::{
    common::{ToDigest, WithStartAndShutdown},
    config::ConfigConsumer,
    infu_collection::{c, Collection},
    signer::{SignerInterface, SubmitTxSocket},
    types::{TransactionResponse, UpdateMethod, UpdatePayload, UpdateRequest},
    ApplicationInterface, MempoolSocket, SyncQueryRunnerInterface,
};
use tokio::sync::{mpsc, Notify};

// If a transaction does not get ordered, the signer will try to resend it.
// `TIMEOUT` specifies the duration the signer will wait before resending transactions to the
// mempool.
#[cfg(not(test))]
const TIMEOUT: Duration = Duration::from_secs(300);
#[cfg(test)]
const TIMEOUT: Duration = Duration::from_secs(3);

#[allow(clippy::type_complexity)]
pub struct Signer<C: Collection> {
    inner: Arc<SignerInner>,
    socket: Socket<UpdateMethod, u64>,
    is_running: Arc<Mutex<bool>>,
    // `rx` is only parked here for the time from the call to `ìnit` to the call to `start`,
    // when it is moved into the SignerInner. The only reason it is behind a Arc<Mutex<>> is to
    // ensure that `Signer` is Send and Sync.
    rx: Arc<Mutex<Option<mpsc::Receiver<Task<UpdateMethod, u64>>>>>,
    // `mempool_socket` is only parked here for the time from the call to `provide_mempool` to the
    // call to `start`, when it is moved into SignerInner.
    mempool_socket: Arc<Mutex<Option<MempoolSocket>>>,
    // `mempool_socket` is only parked here for the time from the call to `provide_query_runner` to
    // the call to `start`, when it is moved into SignerInner.
    query_runner: Arc<Mutex<Option<c![C::ApplicationInterface::SyncExecutor]>>>,
    // `new_block_notify` is only parked here for the time from the call to
    // `provide_new_block_notify` to the call to `start`, when it is moved into SignerInner.
    new_block_notify: Arc<Mutex<Option<Arc<Notify>>>>,
    shutdown_notify: Arc<Notify>,
}

#[async_trait]
impl<C: Collection> WithStartAndShutdown for Signer<C> {
    /// Returns true if this system is running or not.
    fn is_running(&self) -> bool {
        *self.is_running.lock().unwrap()
    }

    /// Start the system, should not do anything if the system is already
    /// started.
    async fn start(&self) {
        if !*self.is_running.lock().unwrap() {
            let inner = self.inner.clone();
            let rx = self.rx.lock().unwrap().take().unwrap();
            let mempool_socket = self.get_mempool_socket();
            let query_runner = self.get_query_runner();
            let new_block_notify = self.get_new_block_notify();
            let shutdown_notify = self.shutdown_notify.clone();
            tokio::spawn(async move {
                inner
                    .handle(
                        rx,
                        shutdown_notify,
                        mempool_socket,
                        query_runner,
                        new_block_notify,
                    )
                    .await
            });
            *self.is_running.lock().unwrap() = true;
        }
    }

    /// Send the shutdown signal to the system.
    async fn shutdown(&self) {
        self.shutdown_notify.notify_one();
        *self.is_running.lock().unwrap() = false;
    }
}

#[async_trait]
impl<C: Collection> SignerInterface<C> for Signer<C> {
    /// Initialize the signature service.
    fn init(
        config: Config,
        query_runner: c![C::ApplicationInterface::SyncExecutor],
    ) -> anyhow::Result<Self> {
        let inner = SignerInner::init(config)?;
        let (socket, rx) = Socket::raw_bounded(2048);
        Ok(Self {
            inner: Arc::new(inner),
            socket,
            is_running: Arc::new(Mutex::new(false)),
            rx: Arc::new(Mutex::new(Some(rx))),
            mempool_socket: Arc::new(Mutex::new(None)),
            query_runner: Arc::new(Mutex::new(Some(query_runner))),
            new_block_notify: Arc::new(Mutex::new(None)),
            shutdown_notify: Arc::new(Notify::new()),
        })
    }

    /// Provide the signer service with the mempool socket after initialization, this function
    /// should only be called once.
    fn provide_mempool(&mut self, mempool: MempoolSocket) {
        // TODO(matthias): I think the receiver can be &self here.
        *self.mempool_socket.lock().unwrap() = Some(mempool);
    }

    // Provide the signer service with a block notifier to get notified when a block of
    // transactions has been processed at the application.
    fn provide_new_block_notify(&self, new_block_notify: Arc<Notify>) {
        *self.new_block_notify.lock().unwrap() = Some(new_block_notify);
    }

    /// Returns the `BLS` public key of the current node.
    fn get_bls_pk(&self) -> ConsensusPublicKey {
        self.inner.consensus_public_key
    }

    /// Returns the `Ed25519` (network) public key of the current node.
    fn get_ed25519_pk(&self) -> NodePublicKey {
        self.inner.node_public_key
    }

    /// Returns the loaded secret key material.
    ///
    /// # Safety
    ///
    /// Just like any other function which deals with secret material this function should
    /// be used with the greatest caution.
    fn get_sk(&self) -> (ConsensusSecretKey, NodeSecretKey) {
        (
            self.inner.consensus_secret_key.clone(),
            self.inner.node_secret_key.clone(),
        )
    }

    /// Returns a socket that can be used to submit transactions to the mempool, these
    /// transactions are signed by the node and a proper nonce is assigned by the
    /// implementation.
    ///
    /// # Panics
    ///
    /// This function can panic if there has not been a prior call to `provide_mempool`.
    fn get_socket(&self) -> SubmitTxSocket {
        self.socket.clone()
    }

    /// Sign the provided raw digest and return a signature.
    ///
    /// # Safety
    ///
    /// This function is unsafe to use without proper reasoning, which is trivial since
    /// this function is responsible for signing arbitrary messages from other parts of
    /// the system.
    fn sign_raw_digest(&self, digest: &[u8; 32]) -> NodeSignature {
        self.inner.node_secret_key.sign(digest)
    }

    /// Generates the node secret key.
    ///
    /// # Safety
    ///
    /// This function will return an error if the key already exists.
    fn generate_node_key(path: &Path) -> anyhow::Result<()> {
        if path.exists() {
            return Err(anyhow!("Node secret key already exists"));
        } else {
            let node_secret_key = NodeSecretKey::generate();
            utils::save(path, node_secret_key.encode_pem())?;
        }
        Ok(())
    }

    /// Generates the network secret keys.
    ///
    /// # Safety
    ///
    /// This function will return an error if the key already exists.
    fn generate_consensus_key(path: &Path) -> anyhow::Result<()> {
        if path.exists() {
            return Err(anyhow!("Consensus secret key already exists"));
        } else {
            let consensus_secret_key = ConsensusSecretKey::generate();
            utils::save(path, consensus_secret_key.encode_pem())?;
        }
        Ok(())
    }
}

impl<C: Collection> Signer<C> {
    fn get_mempool_socket(&self) -> MempoolSocket {
        self.mempool_socket
            .lock()
            .unwrap()
            .take()
            .expect("Mempool socket must be provided before starting the signer service.")
    }

    // TODO(qti3e): Why are we locking the query runner? It's both `Clone`
    // and only takes `&self` on every single method.
    //
    // We can simply clone here and return.
    fn get_query_runner(&self) -> c![C::ApplicationInterface::SyncExecutor] {
        self.query_runner
            .lock()
            .unwrap()
            .take()
            .expect("Query runner must be provided before starting the signer serivce.")
    }

    fn get_new_block_notify(&self) -> Arc<Notify> {
        self.new_block_notify
            .lock()
            .unwrap()
            .take()
            .expect("New block notify must be provided before starting the signer serivce.")
    }
}

struct SignerInner {
    node_secret_key: NodeSecretKey,
    node_public_key: NodePublicKey,
    consensus_secret_key: ConsensusSecretKey,
    consensus_public_key: ConsensusPublicKey,
}

impl SignerInner {
    fn init(config: Config) -> anyhow::Result<Self> {
        let node_secret_key = if config.node_key_path.exists() {
            // read pem file, if we cant read the pem file we should panic
            let encoded =
                read_to_string(&config.node_key_path).expect("Failed to read node pem file");
            // todo(dalton): We should panic if we cannot decode pem file. But we should try to
            // identify the encoding and try a few different ways first. Also we should
            // support passworded pems
            NodeSecretKey::decode_pem(&encoded).expect("Failed to decode node pem file")
        } else {
            return Err(anyhow!(
                "Node secret key does not exist. Use the CLI to generate keys."
            ));
        };

        let consensus_secret_key = if config.consensus_key_path.exists() {
            // read pem file, if we cant read the pem file we should panic
            let encoded = read_to_string(&config.consensus_key_path)
                .expect("Failed to read consensus pem file");
            // todo(dalton): We should panic if we cannot decode pem file. But we should try to
            // identify the encoding and try a few different ways first. Also we should
            // support passworded pems
            ConsensusSecretKey::decode_pem(&encoded).expect("Failed to decode consensus pem file")
        } else {
            return Err(anyhow!(
                "Consensus secret key does not exist. Use the CLI to generate keys."
            ));
        };

        let node_public_key = node_secret_key.to_pk();
        let consensus_public_key = consensus_secret_key.to_pk();
        Ok(Self {
            node_secret_key,
            node_public_key,
            consensus_secret_key,
            consensus_public_key,
        })
    }

    async fn handle<Q: SyncQueryRunnerInterface>(
        self: Arc<Self>,
        mut rx: mpsc::Receiver<Task<UpdateMethod, u64>>,
        shutdown_notify: Arc<Notify>,
        mempool_socket: MempoolSocket,
        query_runner: Q,
        new_block_notify: Arc<Notify>,
    ) {
        let mut pending_transactions = VecDeque::new();
        let mut base_timestamp = None;
        let application_nonce =
            if let Some(node_info) = query_runner.get_node_info(&self.node_public_key) {
                node_info.nonce
            } else {
                0
            };
        let mut base_nonce = application_nonce;
        let mut next_nonce = application_nonce + 1;
        loop {
            tokio::select! {
                task = rx.recv() => {
                    let task = task.expect("Failed to receive UpdateMethod.");
                    let update_method = task.request.clone();
                    task.respond(next_nonce);
                    let update_payload = UpdatePayload { method: update_method, nonce: next_nonce };
                    let digest = update_payload.to_digest();
                    let signature = self.node_secret_key.sign(&digest);
                    let update_request = UpdateRequest {
                        sender:  TransactionSender::NodeMain(self.node_public_key),
                        signature: signature.into(),
                        payload: update_payload,
                    };
                    mempool_socket.run(update_request.clone())
                        .await
                        .map_err(|r| anyhow::anyhow!(format!("{r:?}")))
                        .expect("Failed to send transaction to mempool.");

                    // Optimistically increment nonce
                    next_nonce += 1;
                    let timestamp = SystemTime::now();
                    pending_transactions.push_back(PendingTransaction {
                        update_request,
                        timestamp,
                    });
                    // Set timer
                    if base_timestamp.is_none() {
                        base_timestamp = Some(timestamp);
                    }
                }
                _ = new_block_notify.notified() => {
                    SignerInner::sync_with_application(
                        self.node_public_key,
                        &query_runner,
                        &mempool_socket,
                        &mut base_nonce,
                        &mut next_nonce,
                        &mut base_timestamp,
                        &mut pending_transactions
                    ).await;
                }
                _ = shutdown_notify.notified() => break,
            }
        }
    }

    async fn sync_with_application<Q: SyncQueryRunnerInterface>(
        node_public_key: NodePublicKey,
        query_runner: &Q,
        mempool_socket: &MempoolSocket,
        base_nonce: &mut u64,
        next_nonce: &mut u64,
        base_timestamp: &mut Option<SystemTime>,
        pending_transactions: &mut VecDeque<PendingTransaction>,
    ) {
        // If node_info does not exist for the node, there is no point in sending a transaction
        // because it will revert. However, this can still be useful for testing.
        let application_nonce =
            if let Some(node_info) = query_runner.get_node_info(&node_public_key) {
                node_info.nonce
            } else {
                0
            };
        if *base_nonce == application_nonce && *next_nonce > *base_nonce + 1 {
            // Application nonce has not been incremented even though we sent out
            // a transaction
            if let Some(base_timestamp_) = base_timestamp {
                if base_timestamp_.elapsed().unwrap() >= TIMEOUT {
                    // At this point we assume that the transaction with nonce `base_nonce` will
                    // never arrive at the mempool
                    *base_timestamp = None;
                    // Reset `next_nonce` to the nonce the application is expecting.
                    *next_nonce = *base_nonce + 1;
                    // Resend all transactions with nonce >= base_nonce.
                    for pending_tx in pending_transactions.iter_mut() {
                        if let TransactionResponse::Revert(_) =
                            query_runner.validate_txn(pending_tx.update_request.clone())
                        {
                            // If transaction reverts, don't retry.
                            continue;
                        }
                        *next_nonce += 1;
                        mempool_socket
                            .run(pending_tx.update_request.clone())
                            .await
                            .map_err(|r| anyhow::anyhow!(format!("{r:?}")))
                            .expect("Failed to send transaction to mempool.");
                        // Update timestamp to resending time.
                        pending_tx.timestamp = SystemTime::now();
                        if base_timestamp.is_none() {
                            *base_timestamp = Some(pending_tx.timestamp);
                        }
                    }
                }
            }
        } else if application_nonce > *base_nonce {
            *base_nonce = application_nonce;
            // All transactions in range [base_nonce, application_nonce] have
            // been ordered, so we can remove them from `pending_transactions`.
            while !pending_transactions.is_empty()
                && pending_transactions[0].update_request.payload.nonce <= application_nonce
            {
                pending_transactions.pop_front();
            }
            if pending_transactions.is_empty() {
                *base_timestamp = None;
            } else {
                *base_timestamp = Some(pending_transactions[0].timestamp);
            }
        }
    }
}

#[derive(Clone)]
struct PendingTransaction {
    pub update_request: UpdateRequest,
    pub timestamp: SystemTime,
}

impl<C: Collection> ConfigConsumer for Signer<C> {
    const KEY: &'static str = "signer";

    type Config = Config;
}
