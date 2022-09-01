// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT
use anyhow::anyhow;
use arc_swap::ArcSwap;
use async_std::channel::Receiver;
use async_std::path::PathBuf;
use async_std::stream::StreamExt;
use async_std::task;
use async_std::{channel, task::JoinHandle};
use async_trait::async_trait;
use log::{error, warn};
use narwhal_types::SequenceNumber;
use std::collections::HashMap;
use std::sync::Arc;

use forest_blocks::{BlockHeader, GossipBlock, Ticket, Tipset};
use forest_chain::{HeadChange, Scale};
use forest_chain_sync::consensus::{MessagePoolApi, Proposer, SyncGossipSubmitter};
use forest_ipld_blockstore::BlockStore;
use forest_message::SignedMessage;
use forest_networks::Height;
use forest_state_manager::StateManager;
use fvm_ipld_encoding::Cbor;
use fvm_shared::address::Address;

use narwhal_config::{Committee, Parameters};
use narwhal_fastcrypto::{traits::KeyPair as _, KeyPair, PublicKey};
use narwhal_node::{Node, NodeStorage};
use narwhal_types::{TransactionProto, TransactionsClient};

use crate::consensus::{NarwhalConsensus, NarwhalConsensusError};
use crate::exec::{ConsensusTransactionIndex, NarwhalExecutionState, NarwhalOutput};

/// `NarwhalProposer` regularly pulls transactions from the mempool, sorted by their
/// account nonces, and sends them to Narwhal for Atomic Broadcast. The built-in mempool
/// gossiping is expected to be turned off, so this node will create batches from the
/// transactions that users sent to it, while other nodes will similarly batch their
/// own transactions, keeping transaction duplication across batches to the minimum.
/// Users can send their transactions to multiple nodes, for redundancy and censorship
/// resistance.
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
    validator_addresses: HashMap<PublicKey, Address>,
    storage_base_path: PathBuf,
    parameters: Parameters,
}

#[async_trait]
impl Proposer for NarwhalProposer {
    /// Start a Narwhal Primary and a Worker in the background and subscribe to
    /// local chain extensions. Whenever there is a new tip: (1) check for new
    /// transactions that can be sent to Narwhal and (2) take the next output
    /// from Narwhal and turn it into the next block.
    async fn spawn<DB, MP>(
        self,
        state_manager: Arc<StateManager<DB>>,
        mpool: Arc<MP>,
        submitter: SyncGossipSubmitter,
    ) -> anyhow::Result<Vec<JoinHandle<()>>>
    where
        DB: BlockStore + Sync + Send + 'static,
        MP: MessagePoolApi + Send + Sync + 'static,
    {
        let chain_store = state_manager.chain_store();
        let (output_tx, output_rx) = channel::bounded(self.committee.size());
        let (head_tx, head_rx) = tokio::sync::watch::channel(None);
        let execution_state = Arc::new(NarwhalExecutionState::new(chain_store.clone(), output_tx));

        // The following is based on the `NodeRestarter` in the narwhal repo.

        // Get a store for the epoch of the committee (although currently there won't be reconfiguration.)
        let mut store_path = self.storage_base_path.clone();
        store_path.push(format!("epoch{}", self.committee.epoch()));
        let store = NodeStorage::reopen(store_path);

        let registry = prometheus::Registry::default();
        let name = self.keypair.public().clone();

        // Lazily connect to the worker which isn't running yet.
        let transactions_client = make_transactions_client(&name, &self.committee)?;

        // Start the Primary and a Worker. They will ask the execution state where to resume from.
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
            name,
            vec![0],
            Arc::new(ArcSwap::new(Arc::new(self.committee))),
            &store,
            self.parameters,
            &registry,
        );

        handles.extend(tokio_to_async_std(primary_handles));
        handles.extend(tokio_to_async_std(worker_handles));

        // Subscribe to local chain extensions.
        let head_change_rx = chain_store.publisher().subscribe();

        // Get the current head of the chain.
        let current_head = chain_store.heaviest_tipset().await;

        // NOTE: Instead of passing in the current head, we could use `chain_store.sub_header_changes` and `chain_store.next_header_change`,
        // which has extra machinery to replay the current head, but it involves extra bookkeeping, channels and tasks, and also sets a
        // lower limit for lagging, although that in our case wouldn't matter since we create blocks at the pace we can consume them.
        let state_manager_clone = state_manager.clone();
        handles.push(task::spawn(async move {
            if let Err(e) = handle_head_changes(
                state_manager_clone,
                submitter,
                self.validator_addresses,
                current_head,
                head_change_rx,
                head_tx,
                output_rx,
            )
            .await
            {
                error!("Error handling head changes: {}", e)
            }
        }));

        handles.push(task::spawn(async move {
            if let Err(e) =
                handle_head_watch(state_manager, mpool, head_rx, transactions_client).await
            {
                error!("Error handling head watch: {}", e)
            }
        }));

        Ok(handles)
    }
}

fn tokio_to_async_std(
    handles: Vec<tokio::task::JoinHandle<()>>,
) -> Vec<async_std::task::JoinHandle<()>> {
    handles
        .into_iter()
        .map(|h| task::spawn(async { h.await.expect("The tokio task has panicked.") }))
        .collect()
}

/// Handle local chain extensions by projecting blocks on top of them from the Narwhal batches.
/// Also ping the mempool reader process that there might be some change that makes it worth running another check.
async fn handle_head_changes<DB>(
    state_manager: Arc<StateManager<DB>>,
    submitter: SyncGossipSubmitter,
    validator_addresses: HashMap<PublicKey, Address>,
    mut current_head: Option<Arc<Tipset>>,
    mut head_change_rx: tokio::sync::broadcast::Receiver<HeadChange>,
    head_tx: tokio::sync::watch::Sender<Option<Arc<Tipset>>>,
    output_rx: Receiver<NarwhalOutput>,
) -> anyhow::Result<()>
where
    DB: BlockStore + Sync + Send + 'static,
{
    loop {
        // Wait for the next chain extension.
        let next_head = if let Some(tipset) = current_head.take() {
            tipset
        } else {
            match head_change_rx.recv().await {
                Err(_) => {
                    return Err(anyhow!("Cannot receive head changes!"));
                }
                Ok(HeadChange::Current(_)) => panic!("Did not expect to receive the current head."),
                Ok(HeadChange::Revert(_)) => {
                    panic!("This consensus is supposed to be forward only!")
                }
                Ok(HeadChange::Apply(tipset)) => tipset,
            }
        };

        // Notify the mempool polling process that there's a new tip.
        head_tx.send_replace(Some(next_head.clone()));

        // Take the next certificate from the queue and turn it into a block, then submit.
        let next_output = match output_rx.recv().await {
            Err(_) => return Err(anyhow!("Cannot receive Narwhal output!")),
            Ok(output) => output,
        };

        // Check that the next certificate can actually be appended to the next head.
        let head_index = ConsensusTransactionIndex::try_from(next_head.as_ref())?;
        if head_index.next_consensus_index() > next_output.consensus_output.consensus_index {
            // It looks like maybe we got a block from a peer during historical catch up that already
            // includes this output, and we should not turn it into a duplicate block.
            continue;
        }

        // TODO: We might have to create multiple blocks from the same batch to respect limits!
        // For that, we should bite off just enough messages to be within limits, then keep the
        // partially consumed output in a buffer until we manage to append the block.
        let block = create_block_from_output(
            &state_manager,
            &validator_addresses,
            &next_head,
            next_output,
        )
        .await?;

        // Enqueue appending to the local blockchain.
        submitter.submit_block_locally(block).await?;
    }
}

/// Create a block from a Narwhal certificate and the included batches of transactions.
async fn create_block_from_output<DB>(
    state_manager: &Arc<StateManager<DB>>,
    validator_addresses: &HashMap<PublicKey, Address>,
    base: &Arc<Tipset>,
    output: NarwhalOutput,
) -> anyhow::Result<GossipBlock>
where
    DB: BlockStore + Sync + Send + 'static,
{
    let validator_key = output.consensus_output.certificate.origin();

    let miner_addr = validator_addresses
        .get(&validator_key)
        .copied()
        .ok_or(NarwhalConsensusError::NoMinerAddress(validator_key))?;

    let consensus_index = output.consensus_output.consensus_index;

    let mut messages = Vec::new();

    for batch in output.transaction_batches {
        for bytes in batch {
            match SignedMessage::unmarshal_cbor(bytes.as_ref()) {
                Ok(msg) => messages.push(msg),
                Err(e) => {
                    // Narwhal will let anything in, so a Byzantine validator can send us garbage.
                    // There's no way for us to decide if the fault is with them, or our deseralizer.
                    warn!("Error unmarshaling message: {}", e);
                    continue;
                }
            };
        }
    }

    create_block(state_manager, base, miner_addr, consensus_index, messages).await
}

/// Create a block from a Narwhal certificate and the included batches of transactions.
async fn create_block<DB>(
    state_manager: &Arc<StateManager<DB>>,
    base: &Arc<Tipset>,
    miner_addr: Address,
    consensus_index: SequenceNumber,
    messages: Vec<SignedMessage>,
) -> anyhow::Result<GossipBlock>
where
    DB: BlockStore + Sync + Send + 'static,
{
    let smoke_height = state_manager.chain_config().epoch(Height::Smoke);
    let (parent_state_root, parent_receipts) = state_manager.tipset_state(base).await?;
    let parent_base_fee =
        forest_chain::compute_base_fee(state_manager.blockstore(), base, smoke_height)?;

    let parent_weight = NarwhalConsensus::weight(state_manager.blockstore(), base)?;
    let persisted = forest_chain::persist_block_messages(
        state_manager.blockstore(),
        messages.iter().collect(),
    )?;

    // TODO: If we make a partial block then we have to keep track of the
    // index of the last transaction we managed to include as well.
    let index = ConsensusTransactionIndex {
        consensus_index,
        transactions_included: messages.len() as u32,
        transactions_total: messages.len() as u32,
    };

    // Use the ticket to persist the consensus index, for crash recovery.
    let ticket: Ticket = index.try_into()?;

    // There is nothing in the certificate to suggest what time it was made, so we can't assign a timestamp.
    let mut header = BlockHeader::builder()
        .messages(persisted.msg_cid)
        .bls_aggregate(Some(persisted.bls_agg))
        .miner_address(miner_addr)
        .ticket(Some(ticket))
        .weight(parent_weight)
        .parent_base_fee(parent_base_fee)
        .parents(base.key().clone())
        .epoch(base.epoch() + 1)
        .state_root(parent_state_root)
        .message_receipts(parent_receipts)
        .build()?;

    // We cannot sign the header because it has to be derived deterministically and identically on all nodes.
    header.signature = None;

    Ok(GossipBlock {
        header,
        bls_messages: persisted.bls_cids,
        secpk_messages: persisted.secp_cids,
    })
}

/// Upon the extension of the local chain, check the mempool for messages that could be executed according
/// to their account ID and nonce. Using a `watch` channel because it's okay to skip this step if it's
/// already running, we don't have to react to every block.
async fn handle_head_watch<DB, MP>(
    state_manager: Arc<StateManager<DB>>,
    mpool: Arc<MP>,
    mut head_rx: tokio::sync::watch::Receiver<Option<Arc<Tipset>>>,
    mut transactions_client: TransactionsClient<tonic::transport::channel::Channel>,
) -> anyhow::Result<()>
where
    DB: BlockStore + Sync + Send + 'static,
    MP: MessagePoolApi + Send + Sync + 'static,
{
    // Keep track of the last nonce for each account for which we sent a message to Narwhal.
    let mut submitted_account_sequences = HashMap::new();

    loop {
        // Wait for a block to be appended to the local chain.
        // Narwhal will produce blocks regularly, so as long as we append non-empty
        // blocks this event will fire. We might opt not to add empty blocks, but
        // it's not clear for how long; if we don't add blocks unless there are
        // messages inside, then we'd need another timer here. But that would lead
        // to a very dead looking blockchain if there are no transactions at all.
        head_rx.changed().await?;

        // Hold the borrow for as little time as possible to avoid blocking the writes.
        let maybe_head = {
            let r = head_rx.borrow_and_update();
            r.clone()
        };

        if let Some(current_head) = maybe_head {
            // Select any message that can now be included on the chain. These might be
            // because the account has a positive balance where it didn't previously,
            // or because there are new messages that we didn't have last time.
            // What it will not do is try to project a `base` block into the future
            // and guess what the account balance might be as if we have already
            // executed the messages we sent to Narwhal. Instead we'll just skip
            // those messages so they aren't double sent.
            // A side effect of this skipping is that if the user replaces their message
            // in the mempool with a different one having the same nonce, we will just
            // skip if it's already been sent to Narwhal.
            // At some point we also have to make sure that if Narwhal didn't include
            // a batch that contains these messages, we try to send them again.
            // TODO: Check how Narwhal behaves with regards to ignoring batches,
            // and that it preserves insertion order.
            let msgs = mpool
                .select_signed(
                    state_manager.as_ref(),
                    current_head.as_ref(),
                    &submitted_account_sequences,
                )
                .await?;

            if msgs.is_empty() {
                continue;
            }

            for msg in msgs.iter() {
                submitted_account_sequences.insert(msg.message.from, msg.message.sequence);
            }

            let msgs = msgs
                .into_iter()
                .map(|msg| msg.marshal_cbor())
                .collect::<Result<Vec<_>, _>>()?;

            // Send the transactions to the worker.
            // Based on benchmark_client.rs
            let stream = tokio_stream::iter(msgs.into_iter()).map(|msg| TransactionProto {
                transaction: msg.into(),
            });

            // TODO: For now let this task fail if we can't send. But we need better error handling, maybe it's not running yet.
            transactions_client
                .submit_transaction_stream(stream)
                .await?;
        }
    }
}

/// Open a gRPC client through which we can send transactions.
fn make_transactions_client(
    own_public_key: &PublicKey,
    committee: &Committee,
) -> anyhow::Result<TransactionsClient<tonic::transport::channel::Channel>> {
    // Based on reconfigure.rs in the Narwhal repo.
    let target = committee
        .worker(own_public_key, /* id */ &0)
        .map_err(|e| anyhow!("Our key or worker id is not in the committee: {}", e))?
        .transactions;

    let config = mysten_network::config::Config::new();

    let channel = config
        .connect_lazy(&target)
        .map_err(|e| anyhow!("Could not connect to target {}: {}", target, e))?;

    let client = TransactionsClient::new(channel);

    Ok(client)
}
