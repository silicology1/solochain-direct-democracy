//! Benchmarking setup for pallet-star-voting

use super::*;

#[allow(unused)]
use crate::Pallet as StarVoting;
use frame_benchmarking::v2::*;
use frame_system::pallet_prelude::BlockNumberFor;
use frame_system::RawOrigin;

#[benchmarks]
mod benchmarks {
	use super::*;

	#[benchmark]
	fn create_election(c: Linear<5, 1000>) -> Result<(), BenchmarkError> {
		let candidates: Vec<CandidateId> = (1..=c).collect();
		let winner_count = c as u16;
		let caller: T::AccountId = whitelisted_caller();
		let voting_period: BlockNumberFor<T> = 10u32.into();

		#[extrinsic_call]
		_(RawOrigin::Signed(caller), winner_count, candidates, voting_period);

		assert_eq!(NextElectionId::<T>::get(), 1);
		Ok(())
	}

	#[benchmark]
	fn vote(c: Linear<5, 1000>) -> Result<(), BenchmarkError> {
		let candidates: Vec<CandidateId> = (1..=c).collect();
		let creator: T::AccountId = whitelisted_caller();
		Pallet::<T>::create_election(
			RawOrigin::Signed(creator.clone()).into(),
			5u16,
			candidates,
			10u32.into(),
		)?;

		// One score offset per candidate: all +5 (offset 10).
		let scores: Vec<u8> = vec![10u8; c as usize];
		let caller: T::AccountId = whitelisted_caller();

		#[extrinsic_call]
		_(RawOrigin::Signed(caller), 0u32, scores);

		assert_eq!(BallotCount::<T>::get(0u32), 1);
		Ok(())
	}

	#[benchmark]
	fn submit_tally(c: Linear<5, 1000>) -> Result<(), BenchmarkError> {
		// Winner count is capped at MAX_WINNERS (500); candidates cover the
		// whole range of `c` so worst-case sizes are benchmarked.
		let winner_count = c.min(crate::MAX_WINNERS as u32);
		let candidates: Vec<CandidateId> = (1..=c).collect();
		let creator: T::AccountId = whitelisted_caller();
		Pallet::<T>::create_election(
			RawOrigin::Signed(creator).into(),
			winner_count as u16,
			candidates,
			10u32.into(),
		)?;

		// Skip past the voting period so the tally can be submitted.
		frame_system::Pallet::<T>::set_block_number(20u32.into());

		let winners: Vec<CandidateId> = (1..=winner_count).collect();
		let totals: Vec<(CandidateId, u32)> = (1..=c).map(|i| (i, 0u32)).collect();

		#[extrinsic_call]
		_(RawOrigin::Root, 0u32, winners, totals);

		assert!(Tally::<T>::get(0u32).is_some());
		Ok(())
	}

	impl_benchmark_test_suite!(StarVoting, crate::mock::new_test_ext(), crate::mock::Test);
}
