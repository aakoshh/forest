// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::consensus::NarwhalConsensusError;
use async_std::channel::Sender;
use async_trait::async_trait;
use forest_blocks::{Error as BlockchainError, Ticket, Tipset};
use forest_chain::ChainStore;
use forest_chain::Error as ChainStoreError;
use forest_crypto::VRFProof;
use forest_encoding::tuple::*;
use forest_ipld_blockstore::BlockStore;
use fvm_ipld_encoding::Cbor;
use narwhal_consensus::ConsensusOutput;
use narwhal_executor::{
    BatchExecutionState, ExecutionState, ExecutionStateError, SerializedTransaction,
};
use narwhal_types::SequenceNumber;

pub struct NarwhalOutput {
    pub consensus_output: ConsensusOutput,
    pub transaction_batches: Vec<Vec<SerializedTransaction>>,
}

pub struct NarwhalExecutionState<BS> {
    chain_store: Arc<ChainStore<BS>>,
    output_tx: Sender<NarwhalOutput>,
    locked: AtomicBool,
}

impl<BS> NarwhalExecutionState<BS> {
    pub fn new(chain_store: Arc<ChainStore<BS>>, output_tx: Sender<NarwhalOutput>) -> Self {
        Self {
            chain_store,
            output_tx,
            locked: AtomicBool::new(false),
        }
    }
}

impl ExecutionStateError for NarwhalConsensusError {
    fn node_error(&self) -> bool {
        // For now treat all errors as fatal.
        true
    }
}

impl<BS> ExecutionState for NarwhalExecutionState<BS> {
    type Error = NarwhalConsensusError;

    fn ask_consensus_write_lock(&self) -> bool {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn release_consensus_write_lock(&self) {
        self.locked.store(false, Ordering::Relaxed)
    }
}

#[async_trait]
impl<BS> BatchExecutionState for NarwhalExecutionState<BS>
where
    BS: BlockStore + Sync + Send + 'static,
{
    /// Load the tip of the chain and get the last certificate index from the `Ticket` in the header.
    async fn load_next_certificate_index(&self) -> Result<SequenceNumber, Self::Error> {
        // Load the last certificate from the KV store.
        match self.chain_store.heaviest_tipset().await {
            None => Ok(0),
            Some(tipset) => {
                let index = ConsensusTransactionIndex::try_from(tipset.as_ref())?;
                Ok(index.next_consensus_index())
            }
        }
    }

    async fn handle_consensus(
        &self,
        consensus_output: &ConsensusOutput,
        transaction_batches: Vec<Vec<SerializedTransaction>>,
    ) -> Result<(), Self::Error> {
        // Append the transactions and the certificate to a pending queue that we can take values from
        // when we have extended the local blockchain with a block. It is tempting to try to keep the
        // last block we built here and just keep appending on top of it, or to append it directly to
        // the chainstore. But that would bypass all the regular channels in `TipsetProcessor` and
        // `ChainMuxer`, which are quite difficult to grasp, making this a risky option.

        let output = NarwhalOutput {
            consensus_output: consensus_output.clone(),
            transaction_batches,
        };
        self.output_tx
            .send(output)
            .await
            .map_err(|_| NarwhalConsensusError::Other("Cannot send output.".into()))?;
        Ok(())
    }
}

/// Support crash recovery by storing the index of processed transactions in the block tickets.
/// Looking at the these numbers we can decide if we need to replay from the current, or the next consensus index.
#[derive(Serialize_tuple, Deserialize_tuple, Clone, Debug)]
pub struct ConsensusTransactionIndex {
    /// The consensus index the transactions in the block came from.
    pub consensus_index: SequenceNumber,
    /// The number of transactions included in blocks (this and previous ones) from this consensus output so far.
    pub transactions_included: u32,
    /// The total number of transactions in this consensus output.
    pub transactions_total: u32,
}

impl ConsensusTransactionIndex {
    fn next_consensus_index(&self) -> SequenceNumber {
        if self.transactions_included < self.transactions_total {
            self.consensus_index
        } else {
            self.consensus_index + 1
        }
    }
}

impl Cbor for ConsensusTransactionIndex {}

/// Parse the certificate index from the `Ticket` in a `Tipset`.
///
/// While storing the index in the `Ticket` is a bit of a hack, this way we can be sure that storing
/// the block and the certificate is an atomic operation, and we don't need extra key-value store
/// entries for the sequence number.
impl TryFrom<&Tipset> for ConsensusTransactionIndex {
    type Error = NarwhalConsensusError;

    fn try_from(tipset: &Tipset) -> Result<Self, Self::Error> {
        match tipset.min_ticket() {
            None => Err(NarwhalConsensusError::ChainStore(
                ChainStoreError::Blockchain(BlockchainError::InvalidTipset(
                    "Missing ticket.".into(),
                )),
            )),
            Some(ticket) => Self::try_from(ticket),
        }
    }
}

impl TryFrom<&Ticket> for ConsensusTransactionIndex {
    type Error = NarwhalConsensusError;

    fn try_from(ticket: &Ticket) -> Result<Self, Self::Error> {
        let bytes = ticket.vrfproof.as_bytes();
        match Self::unmarshal_cbor(bytes) {
            Ok(x) => Ok(x),
            Err(e) => Err(NarwhalConsensusError::ChainStore(
                ChainStoreError::Encoding(format!(
                    "Unexpected bytes for sequence number in ticket: {}",
                    e
                )),
            )),
        }
    }
}

impl TryInto<Ticket> for ConsensusTransactionIndex {
    type Error = NarwhalConsensusError;

    fn try_into(self) -> Result<Ticket, Self::Error> {
        let bytes = self.marshal_cbor()?;
        let proof = VRFProof(bytes);
        Ok(Ticket::new(proof))
    }
}
