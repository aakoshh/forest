// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use forest_address::Address;
use fvm_shared::crypto::signature::Signature;

/// Signer is a trait which allows a key implementation to sign data for an address
pub trait Signer {
    /// Function signs any arbitrary data given the [Address].
    fn sign_bytes(&self, data: &[u8], address: &Address) -> Result<Signature, anyhow::Error>;
}
