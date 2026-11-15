
//! Weights for pallet_star_voting
//!
//! These are hand-estimated placeholder weights sufficient for development
//! and testing. Replace them with benchmark-generated weights (see
//! `benchmarking.rs`) before production use.

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::{Weight, constants::RocksDbWeight}};
use core::marker::PhantomData;

/// Weight functions needed for pallet_star_voting.
pub trait WeightInfo {
	fn create_election(c: u32) -> Weight;
	fn vote(c: u32) -> Weight;
	fn submit_tally(c: u32, w: u32) -> Weight;
	fn submit_tally_star(c: u32, w: u32) -> Weight;
}

/// Weights for pallet_star_voting using the Substrate node and recommended hardware.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
	/// Storage: StarVoting NextElectionId (r:1 w:1)
	/// Proof: StarVoting NextElectionId (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: StarVoting Elections (r:0 w:1)
	/// Proof: StarVoting Elections (max_values: None, max_size: Some(4004), added: 4479, mode: MaxEncodedLen)
	/// The range of component `c` is `[5, 1000]`.
	fn create_election(c: u32) -> Weight {
		// Estimated per-candidate cost for hashing/dedup of the candidate list.
		Weight::from_parts(15_000_000, 0)
			.saturating_add(Weight::from_parts(8_000, 0).saturating_mul(c as u64))
			.saturating_add(T::DbWeight::get().reads(1_u64))
			.saturating_add(T::DbWeight::get().writes(2_u64))
	}
	/// Storage: StarVoting Elections (r:1 w:0)
	/// Proof: StarVoting Elections (max_values: None, max_size: Some(4004), added: 4479, mode: MaxEncodedLen)
	/// Storage: StarVoting Ballots (r:1 w:1)
	/// Proof: StarVoting Ballots (max_values: None, max_size: Some(4000), added: 4475, mode: MaxEncodedLen)
	/// Storage: StarVoting BallotCount (r:1 w:1)
	/// Proof: StarVoting BallotCount (max_values: None, max_size: Some(4), added: 2479, mode: MaxEncodedLen)
	/// The range of component `c` is `[5, 1000]`.
	fn vote(c: u32) -> Weight {
		Weight::from_parts(16_000_000, 0)
			.saturating_add(Weight::from_parts(2_500, 0).saturating_mul(c as u64))
			.saturating_add(T::DbWeight::get().reads(3_u64))
			.saturating_add(T::DbWeight::get().writes(2_u64))
	}
	/// Storage: StarVoting Elections (r:1 w:1)
	/// Proof: StarVoting Elections (max_values: None, max_size: Some(4004), added: 4479, mode: MaxEncodedLen)
	/// Storage: StarVoting Tally (r:0 w:1)
	/// Proof: StarVoting Tally (max_values: None, max_size: Some(12000), added: 14475, mode: MaxEncodedLen)
	/// The range of component `c` is `[5, 1000]`.
	/// The range of component `w` is `[5, 500]`.
	fn submit_tally(c: u32, w: u32) -> Weight {
		Weight::from_parts(25_000_000, 0)
			.saturating_add(Weight::from_parts(6_000, 0).saturating_mul(c as u64))
			.saturating_add(Weight::from_parts(3_000, 0).saturating_mul(w as u64))
			.saturating_add(T::DbWeight::get().reads(1_u64))
			.saturating_add(T::DbWeight::get().writes(2_u64))
	}
	/// Same validation and storage effects as `submit_tally`; the extra
	/// domain-tag hashing is negligible. Placeholder pending benchmarks.
	fn submit_tally_star(c: u32, w: u32) -> Weight {
		Self::submit_tally(c, w)
	}
}

// For backwards compatibility and tests
impl WeightInfo for () {
	fn create_election(_c: u32) -> Weight {
		Weight::default()
	}
	fn vote(_c: u32) -> Weight {
		Weight::default()
	}
	fn submit_tally(_c: u32, _w: u32) -> Weight {
		Weight::default()
	}
	fn submit_tally_star(_c: u32, _w: u32) -> Weight {
		Weight::default()
	}
}
