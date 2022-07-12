// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use forest_address::Address;
use forest_cid::multihash::MultihashDigest;
use forest_cid::{Cid, Code, RAW};
use ipld_blockstore::BlockStore;
use serde::Serialize;
use vm::ActorState;

use anyhow::Context;
use ipld_blockstore::BlockStoreExt;

/// Account actor method.
pub type Method = fil_actor_account_v7::Method;

/// Account actor state.
#[derive(Serialize)]
#[serde(untagged)]
pub enum State {
    V7(fil_actor_account_v7::State),
}

pub fn account_cid_v7() -> Cid {
    Cid::new_v1(RAW, Code::Identity.digest(b"fil/7/account"))
}

impl State {
    pub fn load<BS>(store: &BS, actor: &ActorState) -> anyhow::Result<State>
    where
        BS: BlockStore,
    {
        if actor.code == account_cid_v7() {
            Ok(store
                .get_obj(&actor.state)?
                .map(State::V7)
                .context("Actor state doesn't exist in store")?)
        } else {
            Err(anyhow::anyhow!("Unknown account actor code {}", actor.code))
        }
    }

    pub fn pubkey_address(&self) -> Address {
        match self {
            State::V7(st) => st.address,
        }
    }
}
