// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use address::Address;
use cid::multihash::MultihashDigest;
use cid::Cid;
use ipld_blockstore::BlockStore;
use serde::Serialize;
use vm::ActorState;

use anyhow::Context;

/// Init actor address.
pub static ADDRESS: &fil_actors_runtime_v8::builtin::singletons::INIT_ACTOR_ADDR =
    &fil_actors_runtime_v8::builtin::singletons::INIT_ACTOR_ADDR;

/// Init actor method.
pub type Method = fil_actor_init_v8::Method;

pub fn is_v8_init_cid(cid: &Cid) -> bool {
    let known_cids = vec![
        // calibnet
        Cid::try_from("bafk2bzaceadyfilb22bcvzvnpzbg2lyg6npmperyq6es2brvzjdh5rmywc4ry").unwrap(),
        // mainnet
        Cid::try_from("bafk2bzaceaipvjhoxmtofsnv3aj6gj5ida4afdrxa4ewku2hfipdlxpaektlw").unwrap(),
    ];
    known_cids.contains(cid)
}

/// Init actor state.
#[derive(Serialize)]
#[serde(untagged)]
pub enum State {
    V8(fil_actor_init_v8::State),
}

impl State {
    pub fn load<BS>(store: &BS, actor: &ActorState) -> anyhow::Result<State>
    where
        BS: BlockStore,
    {
        if is_v8_init_cid(&actor.code) {
            return Ok(store
                .get_anyhow(&actor.state)?
                .map(State::V8)
                .context("Actor state doesn't exist in store")?);
        }
        if actor.code == Cid::new_v1(cid::RAW, cid::Code::Identity.digest(b"fil/7/init")) {
            return Ok(store
                .get_anyhow(&actor.state)?
                .map(State::V8)
                .context("Actor state doesn't exist in store")?);
        }
        Err(anyhow::anyhow!("Unknown init actor code {}", actor.code))
    }

    /// Allocates a new ID address and stores a mapping of the argument address to it.
    /// Returns the newly-allocated address.
    pub fn map_address_to_new_id<BS: BlockStore>(
        &mut self,
        store: &BS,
        addr: &Address,
    ) -> anyhow::Result<Address> {
        match self {
            State::V8(st) => {
                let fvm_store = ipld_blockstore::FvmRefStore::new(store);
                Ok(Address::new_id(st.map_address_to_new_id(&fvm_store, addr)?))
            }
        }
    }

    /// ResolveAddress resolves an address to an ID-address, if possible.
    /// If the provided address is an ID address, it is returned as-is.
    /// This means that mapped ID-addresses (which should only appear as values, not keys) and
    /// singleton actor addresses (which are not in the map) pass through unchanged.
    ///
    /// Returns an ID-address and `true` if the address was already an ID-address or was resolved
    /// in the mapping.
    /// Returns an undefined address and `false` if the address was not an ID-address and not found
    /// in the mapping.
    /// Returns an error only if state was inconsistent.
    pub fn resolve_address<BS: BlockStore>(
        &self,
        store: &BS,
        addr: &Address,
    ) -> anyhow::Result<Option<Address>> {
        match self {
            State::V8(st) => {
                let fvm_store = ipld_blockstore::FvmRefStore::new(store);
                st.resolve_address(&fvm_store, addr)
            }
        }
    }

    pub fn into_network_name(self) -> String {
        match self {
            State::V8(st) => st.network_name,
        }
    }
}
