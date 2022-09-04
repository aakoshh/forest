// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT
use async_trait::async_trait;
use narwhal_fastcrypto::PublicKey;
use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;

use forest_blocks::{Block, Tipset};
use forest_chain::Error as ChainStoreError;
use forest_chain::Scale;
use forest_chain::Weight;
use forest_chain_sync::consensus::Consensus;
use forest_ipld_blockstore::BlockStore;
use forest_state_manager::Error as StateManagerError;
use forest_state_manager::StateManager;
use fvm_ipld_encoding::Error as ForestEncodingError;
use fvm_shared::bigint::BigInt;
use nonempty::NonEmpty;

#[derive(Debug, Error)]
pub enum NarwhalConsensusError {
    #[error("Chain store error: {0}")]
    ChainStore(#[from] ChainStoreError),
    #[error("StateManager error: {0}")]
    StateManager(#[from] StateManagerError),
    #[error("Encoding error: {0}")]
    ForestEncoding(#[from] ForestEncodingError),
    #[error("No miner address for validator key: {0}")]
    NoMinerAddress(PublicKey),
    #[error("{0}")]
    Other(String),
}

/// In Narwhal Consensus we don't gossip blocks, we create deterministic
/// blocks locally from the certificates we receive from Narwhal, and
/// append them to our local chain.
///
/// The catch is that Narwhal only allows committee members to participate,
/// so we basically cannot have non-validator nodes in the subnet.
#[derive(Debug)]
pub struct NarwhalConsensus;

impl Scale for NarwhalConsensus {
    fn weight<DB>(_: &DB, ts: &Tipset) -> anyhow::Result<Weight>
    where
        DB: BlockStore,
    {
        let header = ts.blocks().first().expect("Tipset is never empty.");
        // We are building a single chain.
        Ok(BigInt::from(header.epoch()))
    }
}

#[async_trait]
impl Consensus for NarwhalConsensus {
    type Error = NarwhalConsensusError;

    async fn validate_block<DB>(
        &self,
        _state_manager: Arc<StateManager<DB>>,
        _block: Arc<Block>,
    ) -> Result<(), NonEmpty<Self::Error>>
    where
        DB: BlockStore + Sync + Send + 'static,
    {
        // There is nothing to validate because we are only supposed to see blocks we created ourselves.
        // The problem is that we will have to use historical syncing from the Forest stack because
        // Narwhal will garbage collect the older blocks. At that point we have the following options:
        // (1) use some kind of checkpointing mechanism, signed by all validators, so we can trust the
        // chain up to the checkpoint without validation; (2) collect a sample from the rest of the
        // peers and hope they would not be agreeing on them if the blocks weren't correct, then just
        // apply them, as long as our chain ends up with the block we sampled.
        Ok(())
    }

    /// We cannot sign because each validator has to derive the same blocks from the total ordering
    /// provided by Narwhal and Bullshark.
    const REQUIRE_MINER_SIGNATURE: bool = false;

    /// Narwhal doesn't indicate the passage of time. While we could estimate the maximum chain growth,
    /// it can only be used if we know how many validators were present in each committee. And the actual
    /// growth can be much less than that, so it's not possible to compare against the wall clock time.
    const ENFORCE_EPOCH_DELAY: bool = false;

    /// We cannot enforce the block gas limit because we put all batches in a certificate into a single block,
    /// using the certificate authority as the miner ID, and we put all certificates at the same round into
    /// the same Tipset. Since a Tipset requires unique miners, we can't split blocks of the same miner.
    const ENFORCE_BLOCK_GAS_LIMIT: bool = false;
}
