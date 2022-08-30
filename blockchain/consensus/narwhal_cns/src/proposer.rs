use arc_swap::ArcSwap;
use async_std::path::PathBuf;
// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT
use async_trait::async_trait;
use std::sync::Arc;

use forest_chain_sync::consensus::{MessagePoolApi, Proposer, SyncGossipSubmitter};
use forest_ipld_blockstore::BlockStore;
use forest_state_manager::StateManager;

use narwhal_config::{Committee, Parameters};
use narwhal_fastcrypto::{traits::KeyPair as _, KeyPair};
use narwhal_node::{Node, NodeStorage};

use crate::state::NarwhalState;

/// `NarwhalProposer` regularly pulls transactions from the mempool, sorted by their
/// account nonces, and sends them to Narwhal for Atomic Broadcast. The built-in mempool
/// gossiping is expected to be turned off, so this node will create batches from the
/// transactions that users sent to it, while other nodes will similarly batch their
/// own transactions, keeping transaction duplication across batches to the minimum.
/// Users can send their transactions to multiple nodes, for redundancy and censorship
/// resisteance.
///
/// The proposer also acts as the execution engine for the Narwhal library, receiving
/// committed certificates and batches of transactions from the network. Each certificate
/// (which contains batches produced by a given validator), will be turned into a single,
/// deterministic Filecoin block, appended to the end of the chain, then executed.
///
/// There might be invalid transactions in the batches, which can be discarded, and
/// potentially the batching validator punished for them.
///
/// Block gossiping is expected to be turned off, so blocks don't need to be signed.
/// Instead, every validator produces exactly the same blocks, and at some point can
/// publish their signatures over them, which can be gathered to form a checkpoint.
pub struct NarwhalProposer {
    keypair: KeyPair,
    committee: Committee,
    storage_base_path: PathBuf,
    parameters: Parameters,
}

#[async_trait]
impl Proposer for NarwhalProposer {
    /// Start a Narwhal Primary and a Worker in the background.
    async fn run<DB, MP>(
        self,
        state_manager: Arc<StateManager<DB>>,
        _mpool: &MP,
        _submitter: &SyncGossipSubmitter,
    ) -> anyhow::Result<()>
    where
        DB: BlockStore + Sync + Send + 'static,
        MP: MessagePoolApi + Send + Sync + 'static,
    {
        // The proposer will need to be able to subscribe to:
        // * The certificates from Narwhal
        // * The chain extensions in the local chain
        // When a new certificate arrives from Narwhal, its content can be added to a
        // pending queue of transactions to be put in the next block.
        // When a block is appended to the local chain, we can check if there are more
        // available in the pending queue and add them next.

        // The following is based on the `NodeRestarter` in the narwhal repo.

        // Get a store for the epoch of the committee (although currently there won't be reconfiguration.)
        let mut store_path = self.storage_base_path.clone();
        store_path.push(format!("epoch{}", self.committee.epoch()));
        let store = NodeStorage::reopen(store_path);
        let registry = prometheus::Registry::default();
        let name = self.keypair.public().clone();

        let execution_state = Arc::new(NarwhalState::new(state_manager.blockstore_cloned()));

        let mut handles = Vec::new();

        let primary_handles = Node::spawn_primary(
            self.keypair,
            Arc::new(ArcSwap::new(Arc::new(self.committee.clone()))),
            &store,
            self.parameters.clone(),
            true,
            execution_state,
            &registry,
        )
        .await?;

        let worker_handles = Node::spawn_workers(
            name.clone(),
            vec![0],
            Arc::new(ArcSwap::new(Arc::new(self.committee))),
            &store,
            self.parameters,
            &registry,
        );

        handles.extend(primary_handles);
        handles.extend(worker_handles);

        // TODO: Register to system shutdown and stop the handles.

        // TODO: Now we have a primary and a worker running. They will ask the execution state where to resume from.
        // Meanwhile we have to subscribe to get the transactions we can currently propose from the mempool and
        // send them to the worker. We must remember which account/highest-nonce pairs we sent last. Then, when
        // a new block is appended to the local chain, we can ask the mempool again, passing in the hightst account
        // nonce pairs as a filter. If it returns new transactions we can publish those as well.

        todo!()
    }
}
