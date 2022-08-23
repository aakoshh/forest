use std::sync::Arc;

// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT
use async_trait::async_trait;

use forest_chain_sync::consensus::{MessagePoolApi, Proposer, SyncGossipSubmitter};
use forest_ipld_blockstore::BlockStore;
use forest_state_manager::StateManager;

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
pub struct NarwhalProposer {}

#[async_trait]
impl Proposer for NarwhalProposer {
    async fn run<DB, MP>(
        self,
        _state_manager: Arc<StateManager<DB>>,
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
        todo!()
    }
}
