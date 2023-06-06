use draco_interfaces::{
    types::{
        AccountInfo, Epoch, ExecutionData, ExecutionError, Metadata, NodeInfo, ProofOfConsensus,
        ProofOfMisbehavior, ProtocolParams, QueryMethod, QueryRequest, Service, ServiceId, Staking,
        Tokens, TransactionResponse, UpdateMethod, UpdateRequest, Worker,
    },
    DeliveryAcknowledgment,
};
use fleek_crypto::{
    AccountOwnerPublicKey, NodeNetworkingPublicKey, NodePublicKey, TransactionSender,
};
use multiaddr::Multiaddr;
use serde::{Deserialize, Serialize};

use crate::table::{Backend, TableRef};

/// The state of the Application
///
/// The functions implemented on this struct are the "Smart Contracts" of the application layer
/// All state changes come from Transactions and start at execute_txn
pub struct State<B: Backend> {
    pub metadata: B::Ref<Metadata, u64>,
    pub account_info: B::Ref<AccountOwnerPublicKey, AccountInfo>,
    pub node_info: B::Ref<NodePublicKey, NodeInfo>,
    pub committee_info: B::Ref<Epoch, Committee>,
    pub bandwidth_info: B::Ref<Epoch, BandwidthInfo>,
    pub services: B::Ref<ServiceId, Service>,
    pub parameters: B::Ref<ProtocolParams, u128>,
    pub backend: B,
}

#[derive(Debug, Hash, PartialEq, PartialOrd, Ord, Eq, Serialize, Deserialize, Clone, Default)]
pub struct Committee {
    pub members: Vec<NodePublicKey>,
    pub ready_to_change: Vec<NodePublicKey>,
    pub epoch_end_timestamp: u64,
}

#[derive(Debug, Hash, PartialEq, PartialOrd, Ord, Eq, Serialize, Deserialize, Clone)]
pub struct BandwidthInfo {
    pub total_served: u128,
    pub reward_pool: u128,
}

impl<B: Backend> State<B> {
    pub fn new(backend: B) -> Self {
        Self {
            metadata: backend.get_table_reference("metadata"),
            account_info: backend.get_table_reference("account"),
            node_info: backend.get_table_reference("node"),
            committee_info: backend.get_table_reference("committee"),
            bandwidth_info: backend.get_table_reference("bandwidth"),
            services: backend.get_table_reference("service"),
            parameters: backend.get_table_reference("parameter"),
            backend,
        }
    }

    /// This function is the entry point of a transaction
    pub fn execute_txn(&self, txn: UpdateRequest) -> TransactionResponse {
        // Execute transaction
        let response = match txn.payload.method {
            UpdateMethod::SubmitDeliveryAcknowledgmentAggregation {
                commodity,
                service_id,
                proofs,
                metadata: _,
            } => self.submit_pod(txn.sender, commodity, service_id, proofs),

            UpdateMethod::Withdraw {
                amount,
                token,
                receiving_address,
            } => self.withdraw(txn.sender, receiving_address, amount, token),

            UpdateMethod::Deposit {
                proof,
                token,
                amount,
            } => self.deposit(txn.sender, proof, amount, token),

            UpdateMethod::Stake {
                amount,
                node_public_key,
                node_network_key,
                node_domain,
                worker_public_key,
                worker_domain,
                worker_mempool_address,
            } => self.stake(
                txn.sender,
                amount,
                node_public_key,
                node_network_key,
                node_domain,
                worker_public_key,
                worker_domain,
                worker_mempool_address,
            ),

            UpdateMethod::Unstake { amount, node } => self.unstake(txn.sender, amount, node),

            UpdateMethod::WithdrawUnstaked { node, recipient } => {
                self.withdrawl_unstaked(txn.sender, node, recipient)
            },

            UpdateMethod::ChangeEpoch => self.change_epoch(txn.sender),

            UpdateMethod::AddService {
                service,
                service_id,
            } => self.add_service(txn.sender, service, service_id),

            UpdateMethod::RemoveService { service_id } => {
                self.remove_service(txn.sender, service_id)
            },

            UpdateMethod::Slash {
                service_id,
                node,
                proof_of_misbehavior,
            } => self.slash(txn.sender, proof_of_misbehavior, service_id, node),
        };
        // Increment nonce of the sender
        self.increment_nonce(txn.sender);
        // Return the response
        response
    }

    pub fn execute_query(&self, txn: QueryRequest) -> TransactionResponse {
        match txn.query {
            QueryMethod::Bandwidth { public_key } => self.get_bandwidth(public_key),
            QueryMethod::FLK { public_key } => self.get_flk(public_key),
            QueryMethod::Locked { public_key } => self.get_locked(public_key),
            QueryMethod::LockedUntil { public_key } => self.get_locked_until(public_key),
            QueryMethod::Staked { node } => self.get_staked(node),
            QueryMethod::Served { epoch, node } => self.get_node_bandwidth_served(epoch, node),
            QueryMethod::TotalServed { epoch } => self.get_total_served(epoch),
            QueryMethod::RewardPool { epoch } => self.get_reward_pool(epoch),
            QueryMethod::CurrentEpochInfo => self.get_current_epoch_info(),
        }
    }
    /*********** External Update Functions ********** */
    // The following functions should only be called in the result of a query or update transaction
    // through execute_txn() If called in an update txn it will mutate state
    fn submit_pod(
        &self,
        _sender: TransactionSender,
        _commodity: u128,
        _service_id: u64,
        _acknowledgments: Vec<DeliveryAcknowledgment>,
    ) -> TransactionResponse {
        todo!()
    }

    fn withdraw(
        &self,
        _sender: TransactionSender,
        _reciever: AccountOwnerPublicKey,
        _amount: u128,
        _token: Tokens,
    ) -> TransactionResponse {
        todo!()
    }

    fn deposit(
        &self,
        sender: TransactionSender,
        proof: ProofOfConsensus,
        amount: u128,
        token: Tokens,
    ) -> TransactionResponse {
        // This transaction is only callable by AccountOwners and not nodes
        // So revert if the sender is a node public key
        let sender = match self.only_account_owner(sender) {
            Ok(account) => account,
            Err(e) => return e,
        };

        // Verify the proof from the bridge
        if !self.backend.verify_proof_of_consensus(proof) {
            return TransactionResponse::Revert(ExecutionError::InvalidProof);
        }

        let mut account = self.account_info.get(&sender).unwrap_or_default();

        // Check the token bridged and increment that amount
        match token {
            Tokens::FLK => account.flk_balance += amount,
            Tokens::USDC => account.bandwidth_balance += amount,
        }

        self.account_info.set(sender, account);
        TransactionResponse::Success(ExecutionData::None)
    }

    #[allow(clippy::too_many_arguments)]
    fn stake(
        &self,
        sender: TransactionSender,
        amount: u128,
        node_public_key: NodePublicKey,
        node_network_key: Option<NodeNetworkingPublicKey>,
        node_domain: Option<String>,
        worker_public_key: Option<NodeNetworkingPublicKey>,
        worker_domain: Option<String>,
        worker_mempool_address: Option<String>,
    ) -> TransactionResponse {
        // This transaction is only callable by AccountOwners and not nodes
        // So revert if the sender is a node public key
        let sender = match self.only_account_owner(sender) {
            Ok(account) => account,
            Err(e) => return e,
        };

        let mut owner = self.account_info.get(&sender).unwrap_or_default();

        // Make sure the sender has atleast the amount of FLK he is trying to stake
        if owner.flk_balance < amount {
            return TransactionResponse::Revert(ExecutionError::InsufficientBalance);
        }

        let node_domain = match node_domain {
            Some(address) => {
                let address = address.parse::<Multiaddr>();
                if address.is_err() {
                    return TransactionResponse::Revert(ExecutionError::InvalidInternetAddress);
                }
                Some(address.unwrap())
            },
            None => None,
        };

        let worker_domain = match worker_domain {
            Some(address) => {
                let address = address.parse::<Multiaddr>();
                if address.is_err() {
                    return TransactionResponse::Revert(ExecutionError::InvalidInternetAddress);
                }
                Some(address.unwrap())
            },
            None => None,
        };

        let worker_mempool_address = match worker_mempool_address {
            Some(address) => {
                let address = address.parse::<Multiaddr>();
                if address.is_err() {
                    return TransactionResponse::Revert(ExecutionError::InvalidInternetAddress);
                }
                Some(address.unwrap())
            },
            None => None,
        };

        let node = match self.node_info.get(&node_public_key) {
            Some(mut node) => {
                // Todo(dalton): should we stop people from staking on a node they do not own??

                // If any of the nodes fields were provided with this transaction update them on the
                // nodes state
                if let Some(network_key) = node_network_key {
                    node.network_key = network_key;
                }
                if let Some(primary_domain) = node_domain {
                    node.domain = primary_domain;
                }
                if let Some(worker_key) = worker_public_key {
                    node.workers[0].public_key = worker_key;
                }
                if let Some(worker_domain) = worker_domain {
                    node.workers[0].address = worker_domain
                }
                if let Some(mempool_address) = worker_mempool_address {
                    node.workers[0].mempool = mempool_address
                }

                // Increase the nodes stake by the amount being staked
                node.stake.staked += amount;

                node
            },
            None => {
                // If the node doesnt Exist, create it. But check if they provided all the required
                // options for a new node
                if let (
                    Some(network_key),
                    Some(primary_domain),
                    Some(worker_key),
                    Some(worker_domain),
                    Some(mempool_domain),
                ) = (
                    node_network_key,
                    node_domain,
                    worker_public_key,
                    worker_domain,
                    worker_mempool_address,
                ) {
                    NodeInfo {
                        owner: sender,
                        public_key: node_public_key,
                        network_key,
                        staked_since: self.metadata.get(&Metadata::Epoch).unwrap_or_default(),
                        stake: Staking {
                            staked: amount,
                            ..Default::default()
                        },
                        domain: primary_domain,
                        workers: [Worker {
                            public_key: worker_key,
                            address: worker_domain,
                            mempool: mempool_domain,
                        }]
                        .into(),
                        nonce: 0,
                    }
                } else {
                    return TransactionResponse::Revert(ExecutionError::InsufficientNodeDetails);
                }
            },
        };

        // decrement the owners balance
        owner.flk_balance -= amount;

        // Commit changes to the node and the owner
        self.account_info.set(sender, owner);
        self.node_info.set(node_public_key, node);

        TransactionResponse::Success(ExecutionData::None)
    }

    fn unstake(
        &self,
        sender: TransactionSender,
        amount: u128,
        node_public_key: NodePublicKey,
    ) -> TransactionResponse {
        // This transaction is only callable by AccountOwners and not nodes
        // So revert if the sender is a node public key
        let sender = match self.only_account_owner(sender) {
            Ok(account) => account,
            Err(e) => return e,
        };

        let mut node = match self.node_info.get(&node_public_key) {
            Some(node) => node,
            None => return TransactionResponse::Revert(ExecutionError::NodeDoesNotExist),
        };

        // Make sure the caller is the owner of the node
        if node.owner != sender {
            return TransactionResponse::Revert(ExecutionError::NotNodeOwner);
        }
        // Make sure the node has atleast that much staked
        if node.stake.staked < amount {
            return TransactionResponse::Revert(ExecutionError::InsufficientBalance);
        }

        let current_epoch = self.metadata.get(&Metadata::Epoch).unwrap_or_default();
        let lock_time = self
            .parameters
            .get(&ProtocolParams::LockTime)
            .unwrap_or_default();

        // decrease the stake, add to the locked amount, and set the locked time for the withdrawl
        // current epoch + lock time todo(dalton): we should be storing unstaked tokens in a
        // list so we can have multiple locked stakes with dif lock times
        node.stake.staked -= amount;
        node.stake.locked += amount;
        node.stake.locked_until = current_epoch + lock_time as u64;

        // Save the changed node state and return success
        self.node_info.set(node_public_key, node);
        TransactionResponse::Success(ExecutionData::None)
    }

    fn withdrawl_unstaked(
        &self,
        sender: TransactionSender,
        node_public_key: NodePublicKey,
        recipient: Option<AccountOwnerPublicKey>,
    ) -> TransactionResponse {
        // This transaction is only callable by AccountOwners and not nodes
        // So revert if the sender is a node public key
        let sender_public_key = match self.only_account_owner(sender) {
            Ok(account) => account,
            Err(e) => return e,
        };

        let mut node = match self.node_info.get(&node_public_key) {
            Some(node) => node,
            None => return TransactionResponse::Revert(ExecutionError::NodeDoesNotExist),
        };

        // Make sure the caller is the owner of the node
        if node.owner != sender_public_key {
            return TransactionResponse::Revert(ExecutionError::NotNodeOwner);
        }

        let current_epoch = self.metadata.get(&Metadata::Epoch).unwrap_or_default();
        // Make sure the node has locked tokens and that the lock time is passed
        if node.stake.locked == 0 {
            return TransactionResponse::Revert(ExecutionError::NoLockedTokens);
        }
        if node.stake.locked_until > current_epoch {
            return TransactionResponse::Revert(ExecutionError::TokensLocked);
        }

        // if there is no recipient the owner will recieve the withdrawl
        let recipient = recipient.unwrap_or(sender_public_key);
        let mut reciever = self.account_info.get(&recipient).unwrap_or_default();

        // add the withdrawn tokens to the recipient and reset the nodes locked stake state
        // no need to reset locked_until on the node because that will get adjusted the next time
        // the node unstakes
        reciever.flk_balance += node.stake.locked;
        node.stake.locked = 0;

        // Todo(dalton): if the nodes stake+locked are equal to 0 here should we remove him from the
        // state tables completly?

        // Save state changes and return response
        self.account_info.set(recipient, reciever);
        self.node_info.set(node_public_key, node);
        TransactionResponse::Success(ExecutionData::None)
    }

    fn change_epoch(&self, sender: TransactionSender) -> TransactionResponse {
        // Only Nodes can call this function
        let sender = match self.only_node(sender) {
            Ok(account) => account,
            Err(e) => return e,
        };

        let mut current_epoch = self.metadata.get(&Metadata::Epoch).unwrap_or_default();
        let mut current_committee = self.committee_info.get(&current_epoch).unwrap_or_default();

        // If sender is not on the current committee revert early, or if they have already signaled;
        if !current_committee.members.contains(&sender) {
            return TransactionResponse::Revert(ExecutionError::NotCommitteeMember);
        } else if current_committee.ready_to_change.contains(&sender) {
            return TransactionResponse::Revert(ExecutionError::AlreadySignaled);
        }
        current_committee.ready_to_change.push(sender);

        // If more than 2/3rds of the committee have signaled, start the epoch change process
        if current_committee.ready_to_change.len() > (current_committee.members.len() / 2) {
            // Todo: Reward nodes, calculate rep?, choose new committee, increment epoch.

            // calculate the next epoch endstamp
            let epoch_duration = self.parameters.get(&ProtocolParams::EpochTime).unwrap();
            let new_epoch_end = current_committee.epoch_end_timestamp + epoch_duration as u64;

            // Save the old committee so we can see who signaled
            self.committee_info.set(current_epoch, current_committee);
            // Get new committee
            let new_committee = self.choose_new_committee();
            // increment epoch
            current_epoch += 1;

            self.committee_info.set(
                current_epoch,
                Committee {
                    ready_to_change: Vec::with_capacity(new_committee.len()),
                    members: new_committee,
                    epoch_end_timestamp: new_epoch_end,
                },
            );
            self.metadata.set(Metadata::Epoch, current_epoch);
            TransactionResponse::Success(ExecutionData::EpochChange)
        } else {
            self.committee_info.set(current_epoch, current_committee);
            TransactionResponse::Success(ExecutionData::None)
        }
    }

    fn add_service(
        &self,
        _sender: TransactionSender,
        _service: Service,
        _service_id: ServiceId,
    ) -> TransactionResponse {
        todo!()
    }

    fn remove_service(
        &self,
        _sender: TransactionSender,
        _service_id: ServiceId,
    ) -> TransactionResponse {
        todo!()
    }

    fn slash(
        &self,
        _sender: TransactionSender,
        _proof: ProofOfMisbehavior,
        _service_id: ServiceId,
        _node: NodePublicKey,
    ) -> TransactionResponse {
        todo!()
    }

    /*******External View Functions****** */
    // The following functions should be called through execute_txn as the result of a txn
    // They will never change state even if called through update
    // Will usually only be called through query calls where msg.sender is not checked
    //      so if that is required for the function it should be made a parameter instead

    fn get_flk(&self, account: AccountOwnerPublicKey) -> TransactionResponse {
        let balance = self
            .account_info
            .get(&account)
            .unwrap_or_default()
            .flk_balance;
        TransactionResponse::Success(ExecutionData::UInt(balance))
    }
    fn get_locked(&self, node: NodePublicKey) -> TransactionResponse {
        let balance = if let Some(node) = self.node_info.get(&node) {
            node.stake.locked
        } else {
            0
        };
        TransactionResponse::Success(ExecutionData::UInt(balance))
    }

    fn get_locked_until(&self, node: NodePublicKey) -> TransactionResponse {
        let until = if let Some(node) = self.node_info.get(&node) {
            node.stake.locked_until
        } else {
            0
        };
        TransactionResponse::Success(ExecutionData::UInt(until as u128))
    }

    fn get_bandwidth(&self, account: AccountOwnerPublicKey) -> TransactionResponse {
        let balance = self
            .account_info
            .get(&account)
            .unwrap_or_default()
            .bandwidth_balance;
        TransactionResponse::Success(ExecutionData::UInt(balance))
    }
    fn get_staked(&self, node: NodePublicKey) -> TransactionResponse {
        let balance = if let Some(node) = self.node_info.get(&node) {
            node.stake.staked
        } else {
            0
        };
        TransactionResponse::Success(ExecutionData::UInt(balance))
    }
    fn get_reward_pool(&self, _epoch: Epoch) -> TransactionResponse {
        todo!()
    }
    fn get_total_served(&self, _epoch: Epoch) -> TransactionResponse {
        todo!()
    }
    fn get_node_bandwidth_served(
        &self,
        _epoch: Epoch,
        _node: NodePublicKey,
    ) -> TransactionResponse {
        todo!()
    }
    fn get_current_epoch_info(&self) -> TransactionResponse {
        let epoch = self.metadata.get(&Metadata::Epoch).unwrap_or_default();
        let committee = self.committee_info.get(&epoch).unwrap_or_default();

        let vec = committee
            .members
            .iter()
            // Safe unwrap, a node should never be added to committee unless we have all data
            // These checks should be done when adding to the committee and to the whitelist.
            .map(|node| self.node_info.get(node).unwrap())
            .collect();

        TransactionResponse::Success(ExecutionData::EpochInfo {
            committee: vec,
            epoch,
            epoch_end: committee.epoch_end_timestamp,
        })
    }

    /********Internal Application Functions******** */
    // These functions should only ever be called in the context of an external transaction function
    // They should never panic and any check that could result in that should be done in the
    // external function that calls it The functions that should call this and the required
    // checks should be documented for each function

    // This function should be called during signal_epoch_change.
    fn _distribute_rewards(&self) {
        todo!()
    }

    fn choose_new_committee(&self) -> Vec<NodePublicKey> {
        // Todo: function not done
        // we need true randomness here, for now we will return the same committee as before to be
        // able to run tests
        let epoch = self.metadata.get(&Metadata::Epoch).unwrap_or_default();
        self.committee_info.get(&epoch).unwrap_or_default().members
    }

    /// Called internally at the end of every transaction to increment the senders nonce.
    /// This happens even if the transaction reverts unless it reverts do to an invalid signature or
    /// nonce This function reverts if the sender does not exist so should be created before
    /// calling this function
    fn increment_nonce(&self, sender: TransactionSender) {
        match sender {
            TransactionSender::Node(node) => {
                self.node_info.get(&node).unwrap().nonce += 1;
            },
            TransactionSender::AccountOwner(account) => {
                self.account_info.get(&account).unwrap().nonce += 1;
            },
        }
    }
    // Useful for transaction that nodes cannot call but an account owner can
    // Does not panic
    fn only_account_owner(
        &self,
        sender: TransactionSender,
    ) -> Result<AccountOwnerPublicKey, TransactionResponse> {
        match sender {
            TransactionSender::AccountOwner(account) => Ok(account),
            _ => Err(TransactionResponse::Revert(
                ExecutionError::OnlyAccountOwner,
            )),
        }
    }
    // Useful for transaction that nodes cann call but an account owner cant
    // Does not panic
    fn only_node(&self, sender: TransactionSender) -> Result<NodePublicKey, TransactionResponse> {
        match sender {
            TransactionSender::Node(node) => Ok(node),
            _ => Err(TransactionResponse::Revert(ExecutionError::OnlyNode)),
        }
    }
}