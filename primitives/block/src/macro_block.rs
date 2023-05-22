use std::fmt;

use beserial::{Deserialize, Serialize};
use nimiq_collections::bitset::BitSet;
use nimiq_hash::{Blake2bHash, Blake2sHash, Hash, SerializeContent};
use nimiq_hash_derive::SerializeContent;
use nimiq_primitives::{policy::Policy, slots::Validators};
use nimiq_transaction::reward::RewardTransaction;
use nimiq_vrf::VrfSeed;
use nimiq_zkp_primitives::MacroBlock as ZKPMacroBlock;
use thiserror::Error;

use crate::{
    signed::{Message, PREFIX_TENDERMINT_PROPOSAL},
    tendermint::TendermintProof,
    BlockError,
};

/// The struct representing a Macro block (can be either checkpoint or election).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MacroBlock {
    /// The header, contains some basic information and commitments to the body and the state.
    pub header: MacroHeader,
    /// The body of the block.
    pub body: Option<MacroBody>,
    /// The justification, contains all the information needed to verify that the header was signed
    /// by the correct producers.
    pub justification: Option<TendermintProof>,
}

impl MacroBlock {
    /// Returns the Blake2b hash of the block header.
    pub fn hash(&self) -> Blake2bHash {
        self.header.hash()
    }

    /// Returns the Blake2s hash of the block header.
    pub fn hash_blake2s(&self) -> Blake2sHash {
        self.header.hash()
    }

    /// Computes the next interlink from self.header.interlink
    pub fn get_next_interlink(&self) -> Result<Vec<Blake2bHash>, BlockError> {
        if !self.is_election_block() {
            return Err(BlockError::InvalidBlockType);
        }
        let mut interlink = self
            .header
            .interlink
            .clone()
            .expect("Election blocks have interlinks");
        let number_hashes_to_update = if self.block_number() == 0 {
            // 0.trailing_zeros() would be 32, thus we need an exception for it
            0
        } else {
            (self.block_number() / Policy::blocks_per_epoch()).trailing_zeros() as usize
        };
        if number_hashes_to_update > interlink.len() {
            interlink.push(self.hash());
        }
        assert!(
            interlink.len() >= number_hashes_to_update,
            "{} {}",
            interlink.len(),
            number_hashes_to_update,
        );
        #[allow(clippy::needless_range_loop)]
        for i in 0..number_hashes_to_update {
            interlink[i] = self.hash();
        }
        Ok(interlink)
    }

    /// Returns whether or not this macro block is an election block.
    pub fn is_election_block(&self) -> bool {
        Policy::is_election_block_at(self.header.block_number)
    }

    /// Returns a copy of the validator slots. Only returns Some if it is an election block.
    pub fn get_validators(&self) -> Option<Validators> {
        self.body.as_ref()?.validators.clone()
    }

    /// Returns the block number of this macro block.
    pub fn block_number(&self) -> u32 {
        self.header.block_number
    }

    /// Return the round of this macro block.
    pub fn round(&self) -> u32 {
        self.header.round
    }

    /// Returns the epoch number of this macro block.
    pub fn epoch_number(&self) -> u32 {
        Policy::epoch_at(self.header.block_number)
    }

    /// Verifies that the block is valid for the given validators.
    pub(crate) fn verify_validators(&self, validators: &Validators) -> Result<(), BlockError> {
        // Verify the Tendermint proof.
        if !TendermintProof::verify(self, validators) {
            warn!(
                %self,
                reason = "Macro block with bad justification",
                "Rejecting block"
            );
            return Err(BlockError::InvalidJustification);
        }

        Ok(())
    }
}

impl<'a> TryFrom<&'a MacroBlock> for ZKPMacroBlock {
    type Error = ();

    fn try_from(block: &'a MacroBlock) -> Result<ZKPMacroBlock, Self::Error> {
        if let Some(proof) = block.justification.as_ref() {
            Ok(ZKPMacroBlock {
                block_number: block.block_number(),
                round_number: proof.round,
                header_hash: block.hash_blake2s(),
                signature: proof.sig.signature.get_point(),
                signer_bitmap: proof
                    .sig
                    .signers
                    .iter_bits()
                    .take(Policy::SLOTS as usize)
                    .collect(),
            })
        } else {
            Ok(ZKPMacroBlock::without_signatures(
                block.block_number(),
                0,
                block.hash_blake2s(),
            ))
        }
    }
}

impl fmt::Display for MacroBlock {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt::Display::fmt(&self.header, f)
    }
}

/// The struct representing the header of a Macro block (can be either checkpoint or election).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, SerializeContent)]
pub struct MacroHeader {
    /// The version number of the block. Changing this always results in a hard fork.
    pub version: u16,
    /// The number of the block.
    pub block_number: u32,
    /// The round number this block was proposed in.
    pub round: u32,
    /// The timestamp of the block. It follows the Unix time and has millisecond precision.
    pub timestamp: u64,
    /// The hash of the header of the immediately preceding block (either micro or macro).
    pub parent_hash: Blake2bHash,
    /// The hash of the header of the preceding election macro block.
    pub parent_election_hash: Blake2bHash,
    /// Hashes of the last blocks dividable by 2^x
    #[beserial(len_type(u8))]
    pub interlink: Option<Vec<Blake2bHash>>,
    /// The seed of the block. This is the BLS signature of the seed of the immediately preceding
    /// block (either micro or macro) using the validator key of the block proposer.
    pub seed: VrfSeed,
    /// The extra data of the block. It is simply up to 32 raw bytes.
    ///
    /// It encodes the initial supply in the genesis block, as a big-endian `u64`.
    ///
    /// No planned use otherwise.
    #[beserial(len_type(u8, limit = 32))]
    pub extra_data: Vec<u8>,
    /// The root of the Merkle tree of the blockchain state. It just acts as a commitment to the
    /// state.
    pub state_root: Blake2bHash,
    /// The root of the Merkle tree of the body. It just acts as a commitment to the body.
    pub body_root: Blake2bHash,
    /// A merkle root over all of the transactions that happened in the current epoch.
    pub history_root: Blake2bHash,
}

impl Message for MacroHeader {
    const PREFIX: u8 = PREFIX_TENDERMINT_PROPOSAL;
}

impl Hash for MacroHeader {}

impl fmt::Display for MacroHeader {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "#{}:MA:{}",
            self.block_number,
            self.hash::<Blake2bHash>().to_short_str(),
        )
    }
}

/// The struct representing the body of a Macro block (can be either checkpoint or election).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, SerializeContent)]
pub struct MacroBody {
    /// Contains all the information regarding the next validator set, i.e. their validator
    /// public key, their reward address and their assigned validator slots.
    /// Is only Some when the macro block is an election block.
    pub validators: Option<Validators>,
    /// A bitset representing which validator slots had their reward slashed at the time when this
    /// block was produced. It is used later on for reward distribution.
    pub lost_reward_set: BitSet,
    /// A bitset representing which validator slots were prohibited from producing micro blocks or
    /// proposing macro blocks at the time when this block was produced. It is used later on for
    /// reward distribution.
    pub disabled_set: BitSet,
    /// The reward related transactions of this block.
    #[beserial(len_type(u16))]
    pub transactions: Vec<RewardTransaction>,
}

impl MacroBody {
    pub(crate) fn verify(&self, is_election: bool) -> Result<(), BlockError> {
        if is_election != self.validators.is_some() {
            return Err(BlockError::InvalidValidators);
        }

        Ok(())
    }
}

impl Hash for MacroBody {}

#[derive(Error, Debug)]
pub enum IntoSlotsError {
    #[error("Body missing in macro block")]
    MissingBody,
    #[error("Not an election macro block")]
    NoElection,
}
