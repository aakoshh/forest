// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT
use async_std::task::{self, JoinHandle};
use async_std::{channel::Sender, stream::StreamExt};
use async_trait::async_trait;
use forest_chain::Scale;
use forest_libp2p::{NetworkMessage, Topic, PUBSUB_BLOCK_STR};
use forest_message::SignedMessage;
use forest_message_pool::MessagePool;
use futures::stream::FuturesUnordered;
use fvm_ipld_encoding::Cbor;
use fvm_shared::address::Address;
use nonempty::NonEmpty;
use std::borrow::Cow;
use std::collections::HashMap;
use std::{
    fmt::{Debug, Display},
    sync::Arc,
};

use forest_blocks::{Block, GossipBlock, Tipset};
use forest_ipld_blockstore::BlockStore;
use forest_state_manager::StateManager;

use crate::WorkerState;

/// The `Consensus` trait encapsulates consensus specific rules of validation
/// and block creation. Behind the scenes they can farm out the total ordering
/// of transactions to an arbitrary consensus engine, but in the end they
/// package the transactions into Filecoin compatible blocks.
///
/// Not all fields will be made use of, however, so the validation of these
/// blocks at least partially have to be trusted to the `Consensus` component.
///
/// Common rules for message ordering will be followed, and can be validated
/// outside by the host system during chain synchronization.
#[async_trait]
pub trait Consensus: Scale + Debug + Send + Sync + Unpin + 'static {
    type Error: Debug + Display + Send + Sync;

    /// Perform block validation asynchronously and return all encountered errors if failed.
    ///
    /// Being asynchronous gives the method a chance to construct a pipeline of
    /// validations, i.e. do some common ones before branching out.
    async fn validate_block<DB>(
        &self,
        state_manager: Arc<StateManager<DB>>,
        block: Arc<Block>,
    ) -> Result<(), NonEmpty<Self::Error>>
    where
        DB: BlockStore + Sync + Send + 'static;

    /// Indicate whether this consensus requires blocks to be signed.
    ///
    /// One use case where we can't have signatures is when nodes derive blocks in a deterministic
    /// fashion from an already ordered stream of transactions. Under this scenario signatures might
    /// be gossiped around later, but they aren't part of the block.
    ///
    /// The reason different nodes can't add different signatures to their individual blocks is because
    /// 1) the would not be able to sign in the name of the miner who should get the rewards
    /// 2) the signature becomes part of the block CID (just not the `to_signing_bytes`),
    ///    so everyone would have different keys in their tipsets
    const REQUIRE_MINER_SIGNATURE: bool;

    /// Indicate whether the `epoch` has relation to time and `block_delay`.
    ///
    /// If not, then we cannot compare epoch to the wall clock time to estimate maximum chain growth.
    const ENFORCE_EPOCH_DELAY: bool;

    /// Should the block be rejected if the global `BLOCK_GAS_LIMIT` is violated.
    const ENFORCE_BLOCK_GAS_LIMIT: bool;
}

/// Helper function to collect errors from async validations.
pub async fn collect_errs<E>(
    mut handles: FuturesUnordered<task::JoinHandle<Result<(), E>>>,
) -> Result<(), NonEmpty<E>> {
    let mut errors = Vec::new();

    while let Some(result) = handles.next().await {
        if let Err(e) = result {
            errors.push(e);
        }
    }

    let mut errors = errors.into_iter();

    match errors.next() {
        None => Ok(()),
        Some(head) => Err(NonEmpty {
            head,
            tail: errors.collect(),
        }),
    }
}

/// The `Proposer` trait expresses the ability to "mine", or in more general,
/// to propose blocks to the network according to the rules of the consensus
/// protocol.
///
/// It is separate from the `Consensus` trait because it is only expected to
/// be called once, then left to run in the background and try to publish
/// blocks to the network which may or may not be adopted.
///
/// It exists mostly as a way for us to describe what kind of dependencies
/// mining processes are expected to take.
#[async_trait]
pub trait Proposer {
    /// Start proposing blocks in the background and never return, unless
    /// something went horribly wrong. Broadly, they should select messages
    /// from the `mempool`, come up with a total ordering for them, then create
    /// blocks and publish them to the network.
    ///
    /// To establish total ordering, the proposer might have to communicate
    /// with other peers using custom P2P messages, however that is its own
    /// concern, the dependencies to implement a suitable network later must
    /// come from somewhere else, because they are not common across all
    /// consensus variants.
    ///
    /// The method returns a vector of handles so that it can start unspecified
    /// number of background tasks, which can all be canceled by the main thread
    /// if the application needs to exit. The method is async so that it can
    /// use async operations to initialize itself, during which it might encounter
    /// some errors.
    ///
    /// The method can use the passed `sync_state` to wait until initial bootstrapping
    /// is finished, before proposing blocks on earlier tips, however it *must* return
    /// the spawned task handles before it looks at this state, because the syncing
    /// might not be running at the time this method is called; waiting on it could
    /// cause the node to hang.
    async fn spawn<DB, MP>(
        self,
        // NOTE: We will need access to the `ChainStore` as well, or, ideally
        // a wrapper over it that only allows us to do what we need to, but
        // for example not reset the Genesis. But since the `StateManager`
        // already exposes the `ChainStore` as is, and this is how it's
        // accessed during validation for example, I think we can defer
        // these for later refactoring and just use the same pattern.
        state_manager: Arc<StateManager<DB>>,
        mpool: Arc<MP>,
        submitter: SyncGossipSubmitter,
        sync_state: WorkerState,
    ) -> anyhow::Result<Vec<JoinHandle<()>>>
    where
        DB: BlockStore + Sync + Send + 'static,
        MP: MessagePoolApi + Sync + Send + 'static;
}

/// The `MessagePoolApi` is the window of consensus to the contents of the `MessagePool`.
///
/// It exists to narrow down the possible operations that a consensus engine can do with
/// the `MessagePool` to only those that it should reasonably exercise, which are mostly
/// read-only queries to get transactions which can be expected to be put in the next
/// block, based on their account nonce values and the current state.
///
/// The `MessagePool` is still expected to monitor the chain growth and remove messages
/// which were included in blocks on its own.
#[async_trait]
pub trait MessagePoolApi {
    /// Select the set of suitable signed messages based on a tipset we are about
    /// to build the next block on.
    ///
    /// The result is a `Cow` in case the source can avoid cloning messages and just
    /// return a reference. They will be sent to the data store for storage, but a
    /// reference is enough for that.
    ///
    /// The skip parameter allows us to set a minimum nonce for accounts from which
    /// we already sent messages to a total ordering service, so we can send more
    /// messages without duplicating the ones we sent earlier.
    async fn select_signed<DB>(
        &self,
        state_manager: &StateManager<DB>,
        base: &Tipset,
        skip: &HashMap<Address, u64>,
    ) -> anyhow::Result<Vec<Cow<SignedMessage>>>
    where
        DB: BlockStore + Sync + Send + 'static;
}

#[async_trait]
impl<P> MessagePoolApi for MessagePool<P>
where
    P: forest_message_pool::Provider + Send + Sync + 'static,
{
    async fn select_signed<DB>(
        &self,
        _: &StateManager<DB>,
        base: &Tipset,
        skip: &HashMap<Address, u64>,
    ) -> anyhow::Result<Vec<Cow<SignedMessage>>>
    where
        DB: BlockStore + Sync + Send + 'static,
    {
        self.select_messages_for_block(base, skip)
            .await
            .map_err(|e| e.into())
            .map(|v| v.into_iter().map(Cow::Owned).collect())
    }
}

/// `SyncGossipSubmitter` dispatches proposed blocks to the network and the local chain synchronizer.
///
/// Similar to `sync_api::sync_submit_block` but assumes that the block is correct and already persisted.
pub struct SyncGossipSubmitter {
    network_name: String,
    network_tx: Sender<NetworkMessage>,
    tipset_tx: Sender<Arc<Tipset>>,
}

impl SyncGossipSubmitter {
    pub fn new(
        network_name: String,
        network_tx: Sender<NetworkMessage>,
        tipset_tx: Sender<Arc<Tipset>>,
    ) -> Self {
        Self {
            network_name,
            network_tx,
            tipset_tx,
        }
    }

    /// Enqueue blocks to be appended to the local chain and also gossip it to peers.
    ///
    /// All the blocks must be in the same epoch, so they can form a tipset.
    pub async fn submit_blocks(&self, blocks: Vec<GossipBlock>) -> anyhow::Result<()> {
        let datas = blocks
            .iter()
            .map(|b| b.marshal_cbor())
            .collect::<Result<Vec<_>, _>>()?;

        self.submit_blocks_locally(blocks).await?;

        for data in datas {
            let msg = NetworkMessage::PubsubMessage {
                topic: Topic::new(format!("{}/{}", PUBSUB_BLOCK_STR, self.network_name)),
                message: data,
            };
            self.network_tx.send(msg).await?;
        }

        Ok(())
    }

    /// Enqueue the blocks to be appended to the local chain, without gossiping to peers.
    ///
    /// They all must be part of the same epoch, to form a single tipset.
    pub async fn submit_blocks_locally(&self, blocks: Vec<GossipBlock>) -> anyhow::Result<()> {
        let hs = blocks.into_iter().map(|b| b.header).collect();
        let ts = Tipset::new(hs)?;
        self.submit_tipset_locally(ts).await?;
        Ok(())
    }

    /// Enqueue a tipset to be appended to the local chain.
    pub async fn submit_tipset_locally(&self, tipset: Tipset) -> anyhow::Result<()> {
        let ts = Arc::new(tipset);
        self.tipset_tx.send(ts).await?;
        Ok(())
    }
}
