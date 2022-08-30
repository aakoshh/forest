use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_std::channel::Sender;
use async_trait::async_trait;
use forest_blocks::{Error as BlockchainError, Ticket, Tipset};
use forest_chain::ChainStore;
use forest_chain::Error as ChainStoreError;
use forest_crypto::VRFProof;
use forest_ipld_blockstore::BlockStore;
use narwhal_consensus::ConsensusOutput;
use narwhal_executor::{
    BatchExecutionState, ExecutionState, ExecutionStateError, SerializedTransaction,
};
use narwhal_types::SequenceNumber;

use crate::consensus::NarwhalConsensusError;

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
            Some(tipset) => tipset_certificate_index(tipset.as_ref()),
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

/// Parse the certificate index from the `Ticket` in a `Tipset`.
///
/// While storing the index in the `Ticket` is a bit of a hack, this way we can be sure that storing
/// the block and the certificate is an atomic operation, and we don't need extra key-value store
/// entries for the sequence number.
fn tipset_certificate_index(tipset: &Tipset) -> Result<SequenceNumber, NarwhalConsensusError> {
    match tipset.min_ticket() {
        None => Err(NarwhalConsensusError::ChainStore(
            ChainStoreError::Blockchain(BlockchainError::InvalidTipset("Missing ticket.".into())),
        )),
        Some(ticket) => ticket_to_certificate_index(ticket),
    }
}

pub fn ticket_to_certificate_index(
    ticket: &Ticket,
) -> Result<SequenceNumber, NarwhalConsensusError> {
    let bytes = ticket.vrfproof.as_bytes();
    match bytes.try_into() {
        Ok(bytes) => Ok(u64::from_be_bytes(bytes)),
        Err(_) => Err(NarwhalConsensusError::ChainStore(
            ChainStoreError::Encoding("Unexpected bytes for sequence number in ticket.".into()),
        )),
    }
}

pub fn certificate_index_to_ticket(index: SequenceNumber) -> Ticket {
    let bytes = u64::to_be_bytes(index);
    let proof = VRFProof(Vec::from(bytes));
    Ticket::new(proof)
}
