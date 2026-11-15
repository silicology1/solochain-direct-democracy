//! # Star Voting Pallet
//!
//! Multi-winner STAR voting (Score Then Automatic Runoff) for direct democracy.
//!
//! ## Overview
//!
//! This pallet stores elections and per-voter ballots on-chain, while the
//! actual tallying is performed off-chain by a trusted tally service. The
//! service submits the winning candidates and per-candidate score totals,
//! and the pallet recomputes a **result hash** on-chain from the submitted
//! data, binding the on-chain result to the exact payload that was accepted.
//!
//! Anyone can fetch the ballots from storage and re-verify the tally
//! off-chain (see the `tally-client` crate), because the ballots are the
//! single source of truth stored on-chain.
//!
//! ## Score encoding
//!
//! Raw scores live in [-5, +5]. On-chain ballots store
//! `score_offset = score + 5` in `[0, 10]`, one byte per candidate.
//! `ABSTAIN_OFFSET (255)` marks a deliberate abstention for that candidate.
//!
//! ## Phases
//!
//! An election moves through three phases:
//! 1. `Voting`    — voters may cast (or not cast) ballots while `block < voting_end`.
//! 2. `Tallying`  — voting has ended; waiting for the tally service.
//! 3. `Finalized` — the tally result has been submitted and bound by its hash.
//!
//! ## Trust model (Phase 1)
//!
//! The chain cannot verify the tally itself without re-running it on-chain.
//! Instead it provides **transparent verifiability**: `submit_tally` requires
//! a configured [`Config::TallyOrigin`], and the pallet recomputes
//! `result_hash = blake2_256(scale-encoded digest)` from the submitted
//! winners + totals, so the stored hash always binds the stored result.
//! The result digest is defined by [`compute_result_hash`]; the off-chain
//! client must replicate its exact SCALE encoding.

#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

extern crate alloc;
use alloc::vec::Vec;

use codec::Encode;
use sp_io::hashing::blake2_256;

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
pub mod weights;
pub use weights::*;

/// Minimum number of winners an election can declare.
pub const MIN_WINNERS: u16 = 5;
/// Maximum number of winners an election can declare.
pub const MAX_WINNERS: u16 = 500;
/// Score offset value marking a deliberate abstention for one candidate.
pub const ABSTAIN_OFFSET: u8 = 255;
/// Maximum valid score offset (raw score +5).
pub const MAX_SCORE_OFFSET: u8 = 10;

/// Identifier of an election.
pub type ElectionId = u32;
/// Identifier of a candidate inside an election.
pub type CandidateId = u32;

// All pallet logic is defined in its own module and must be annotated by the `pallet` attribute.
#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	/// The current lifecycle phase of an election.
	#[derive(
		Clone, Copy, PartialEq, Eq, Encode, Decode, MaxEncodedLen, TypeInfo, RuntimeDebug,
	)]
	pub enum Phase {
		/// Ballots are being accepted.
		Voting,
		/// Voting has ended; waiting for the tally result.
		Tallying,
		/// The tally result has been submitted.
		Finalized,
	}

	/// The tally method a submitted result was computed with.
///
/// Stored alongside every tally so off-chain verifiers know which algorithm
/// (and which result-hash digest) to recompute. The SCALE variant order is
/// part of the digest encoding — do not reorder.
#[derive(
	Clone, Copy, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
	RuntimeDebug,
)]
pub enum TallyMethod {
	/// Plain multi-winner score voting: top-W by real signed total.
	Score,
	/// Bloc STAR: per seat, top-2 by score, then an automatic runoff.
	BlocStar,
	/// STAR-PR (Allocated Score): proportional, sequential ballot allocation.
	StarPr,
}

/// An election: configuration plus lifecycle state.
	///
	/// The ballot format is one `u8` score offset (0..=10, or 255 = abstain)
	/// per candidate, in the order of `candidates`.
	#[derive(
		Clone, PartialEq, Eq, Encode, Decode, MaxEncodedLen, TypeInfo, RuntimeDebugNoBound,
	)]
	#[scale_info(skip_type_params(T))]
	pub struct Election<T: Config> {
		/// Number of winners to elect (MIN_WINNERS..=MAX_WINNERS).
		pub winner_count: u16,
		/// Candidate ids, in ballot-order.
		pub candidates: BoundedVec<CandidateId, T::MaxCandidates>,
		/// First block number at which voting is closed.
		pub voting_end: BlockNumberFor<T>,
		/// Lifecycle phase.
		pub phase: Phase,
	}

	/// The submitted tally result, bound to `result_hash` computed on-chain.
	#[derive(
		Clone, PartialEq, Eq, Encode, Decode, MaxEncodedLen, TypeInfo, RuntimeDebugNoBound,
	)]
	#[scale_info(skip_type_params(T))]
	pub struct TallyResult<T: Config> {
		/// `blake2_256` over the canonical digest of this result (see
		/// [`compute_result_hash`] / [`compute_star_result_hash`]); allows
		/// off-chain tools to detect any tampering with the stored
		/// winners/totals.
		pub result_hash: [u8; 32],
		/// The tally method this result was computed with. Determines which
		/// digest function produced `result_hash` and which off-chain
		/// algorithm verifiers must re-run.
		pub method: TallyMethod,
		/// Winning candidate ids, in ranked order.
		pub winners: BoundedVec<CandidateId, T::MaxWinners>,
		/// Total score per candidate (offset-based, abstentions excluded),
		/// covering every candidate of the election exactly once.
		pub totals: BoundedVec<(CandidateId, u32), T::MaxCandidates>,
		/// The account that submitted the result, if the TallyOrigin was a
		/// signed account (absent for root origins).
		pub submitted_by: Option<T::AccountId>,
		/// Block number of the submission.
		pub submitted_at: BlockNumberFor<T>,
	}

	/// The pallet's configuration trait.
	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The overarching runtime event type.
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
		/// A type representing the weights required by the dispatchables of this pallet.
		type WeightInfo: WeightInfo;
		/// Origin allowed to submit tally results (e.g. the tally service
		/// account, a council, or root for early phases).
		type TallyOrigin: EnsureOrigin<<Self as frame_system::Config>::RuntimeOrigin>;
		/// Maximum number of candidates in one election.
		type MaxCandidates: Get<u32>;
		/// Maximum number of winners in one election (500).
		type MaxWinners: Get<u32>;
	}

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	/// Id the next created election will receive.
	#[pallet::storage]
	pub type NextElectionId<T> = StorageValue<_, ElectionId, ValueQuery>;

	/// Elections by id.
	#[pallet::storage]
	pub type Elections<T: Config> =
		StorageMap<_, Blake2_128Concat, ElectionId, Election<T>, OptionQuery>;

	/// Ballot of a voter in an election: one score offset per candidate,
	/// in `Election::candidates` order.
	#[pallet::storage]
	pub type Ballots<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		(ElectionId, T::AccountId),
		BoundedVec<u8, T::MaxCandidates>,
		OptionQuery,
	>;

	/// How many distinct voters have cast a ballot in an election.
	#[pallet::storage]
	pub type BallotCount<T: Config> = StorageMap<_, Blake2_128Concat, ElectionId, u32, ValueQuery>;

	/// Submitted tally result per election.
	#[pallet::storage]
	pub type Tally<T: Config> =
		StorageMap<_, Blake2_128Concat, ElectionId, TallyResult<T>, OptionQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A new election has been created.
		ElectionCreated {
			election_id: ElectionId,
			winner_count: u16,
			candidate_count: u32,
			voting_end: BlockNumberFor<T>,
		},
		/// A voter has cast a ballot.
		VoteCast {
			election_id: ElectionId,
			who: T::AccountId,
		},
		/// A tally result has been submitted and bound by its hash.
		TallySubmitted {
			election_id: ElectionId,
			/// The tally method the result was computed with.
			method: TallyMethod,
			result_hash: [u8; 32],
			winners: Vec<CandidateId>,
		},
	}

	#[pallet::error]
	pub enum Error<T> {
		/// The election does not exist.
		ElectionNotFound,
		/// The winner count is outside MIN_WINNERS..=MAX_WINNERS.
		InvalidWinnerCount,
		/// The candidate list is longer than MaxCandidates.
		TooManyCandidates,
		/// The candidate list is shorter than the winner count.
		TooFewCandidates,
		/// The candidate list contains duplicates.
		DuplicateCandidate,
		/// The computed voting end overflows the block number type.
		VotingEndOverflow,
		/// Ballot attempted on an election that is not in the Voting phase.
		VotingClosed,
		/// Voting has not ended yet, so no tally may be submitted.
		VotingNotEnded,
		/// The election has already been finalized.
		AlreadyFinalized,
		/// The voter already cast a ballot in this election.
		AlreadyVoted,
		/// A score offset is not in 0..=10 and not the abstain marker.
		InvalidScore,
		/// The ballot does not have exactly one score per candidate.
		BallotLengthMismatch,
		/// The submitted winner count does not equal the election's winner count.
		WinnerCountMismatch,
		/// A winner is not a candidate of this election, or is duplicated.
		InvalidWinners,
		/// The submitted totals do not cover every candidate exactly once.
		InvalidTotals,
		/// `submit_tally_star` was called with `TallyMethod::Score`; score
		/// results must go through `submit_tally`.
		InvalidTallyMethod,
		/// Arithmetic overflow while computing a total.
		ArithmeticOverflow,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Create a new election.
		///
		/// - `winner_count` must be within MIN_WINNERS..=MAX_WINNERS.
		/// - `candidates` must be unique, at least `winner_count` long, and at
		///   most `MaxCandidates` long.
		/// - `voting_period` is the number of blocks the voting phase lasts.
		#[pallet::call_index(0)]
		#[pallet::weight(T::WeightInfo::create_election(candidates.len() as u32))]
		pub fn create_election(
			origin: OriginFor<T>,
			winner_count: u16,
			candidates: Vec<CandidateId>,
			voting_period: BlockNumberFor<T>,
		) -> DispatchResult {
			let _who = ensure_signed(origin)?;

			ensure!(
				winner_count >= MIN_WINNERS && winner_count <= MAX_WINNERS,
				Error::<T>::InvalidWinnerCount
			);
			ensure!(
				(candidates.len() as u32) <= T::MaxCandidates::get(),
				Error::<T>::TooManyCandidates
			);
			ensure!(
				candidates.len() >= winner_count as usize,
				Error::<T>::TooFewCandidates
			);
			// Duplicates would silently break one-byte-per-candidate ballots.
			let candidates =
				BoundedVec::try_from(candidates).map_err(|_| Error::<T>::TooManyCandidates)?;
			let mut sorted = candidates.clone().into_inner();
			sorted.sort_unstable();
			sorted.dedup();
			ensure!(sorted.len() == candidates.len(), Error::<T>::DuplicateCandidate);

			let election_id = NextElectionId::<T>::get();
			NextElectionId::<T>::put(
				election_id.checked_add(1).ok_or(Error::<T>::ArithmeticOverflow)?,
			);

			let current = frame_system::Pallet::<T>::block_number();
			let voting_end = current
				.checked_add(&voting_period)
				.ok_or(Error::<T>::VotingEndOverflow)?;

			Elections::<T>::insert(
				election_id,
				Election { winner_count, candidates, voting_end, phase: Phase::Voting },
			);

			let candidate_count = sorted.len() as u32;
			Self::deposit_event(Event::ElectionCreated { election_id, winner_count, candidate_count, voting_end });

			Ok(())
		}

		/// Cast a ballot in an election.
		///
		/// `scores` holds one score offset per candidate (in candidate order):
		/// a value in `0..=10` (= raw score + 5), or `ABSTAIN_OFFSET` (255) to
		/// abstain on that candidate. One ballot per voter per election.
		#[pallet::call_index(1)]
		#[pallet::weight(T::WeightInfo::vote(scores.len() as u32))]
		// Gasless voting: casting a ballot never charges a fee. The runtime
		// must wrap `ChargeTransactionPayment` with
		// `pallet_skip_feeless_payment::SkipCheckIfFeeless` for this to take
		// effect.
		#[pallet::feeless_if(|_origin: &OriginFor<T>, _election_id: &ElectionId, _scores: &Vec<u8>| -> bool {
			true
		})]
		pub fn vote(
			origin: OriginFor<T>,
			election_id: ElectionId,
			scores: Vec<u8>,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;

			let election = Elections::<T>::get(election_id).ok_or(Error::<T>::ElectionNotFound)?;
			ensure!(election.phase == Phase::Voting, Error::<T>::VotingClosed);
			ensure!(
				frame_system::Pallet::<T>::block_number() < election.voting_end,
				Error::<T>::VotingClosed
			);
			ensure!(
				scores.len() == election.candidates.len(),
				Error::<T>::BallotLengthMismatch
			);
			ensure!(
				scores.iter().all(|s| *s <= MAX_SCORE_OFFSET || *s == ABSTAIN_OFFSET),
				Error::<T>::InvalidScore
			);

			let scores = BoundedVec::try_from(scores)
				.map_err(|_| Error::<T>::BallotLengthMismatch)?;

			// One ballot per voter: reject an attempt to vote twice.
			ensure!(
				!Ballots::<T>::contains_key((election_id, &who)),
				Error::<T>::AlreadyVoted
			);

			let count = BallotCount::<T>::get(election_id);
			let next = count.checked_add(1).ok_or(Error::<T>::ArithmeticOverflow)?;
			BallotCount::<T>::insert(election_id, next);
			Ballots::<T>::insert((election_id, &who), scores);

			Self::deposit_event(Event::VoteCast { election_id, who });
			Ok(())
		}

		/// Submit the tally result of an election whose voting phase ended,
		/// computed with plain multi-winner score voting.
		///
		/// Requires [`Config::TallyOrigin`]. The pallet recomputes
		/// `result_hash` from the submitted data (see [`compute_result_hash`])
		/// and stores it together with the winners and totals, so the stored
		/// hash always binds the stored result.
		///
		/// - `winners` must be exactly `winner_count` unique candidate ids.
		/// - `totals` must contain exactly one entry per candidate, in any
		///   order; entries may be `(candidate, 0)` for no votes received.
		#[pallet::call_index(2)]
		#[pallet::weight(T::WeightInfo::submit_tally(
			totals.len() as u32,
			winners.len() as u32,
		))]
		pub fn submit_tally(
			origin: OriginFor<T>,
			election_id: ElectionId,
			winners: Vec<CandidateId>,
			totals: Vec<(CandidateId, u32)>,
		) -> DispatchResult {
			Self::do_submit_tally(origin, election_id, winners, totals, TallyMethod::Score)
		}

		/// Submit a STAR-method tally result (Bloc STAR or STAR-PR) of an
		/// election whose voting phase ended.
		///
		/// Same validation and storage effects as [`Pallet::submit_tally`],
		/// but the result is bound with the STAR-method digest
		/// ([`compute_star_result_hash`]) and the stored `method` records
		/// which STAR algorithm produced it, so verifiers recompute the right
		/// one. Score-method results must go through `submit_tally`.
		#[pallet::call_index(3)]
		#[pallet::weight(T::WeightInfo::submit_tally_star(
			totals.len() as u32,
			winners.len() as u32,
		))]
		pub fn submit_tally_star(
			origin: OriginFor<T>,
			method: TallyMethod,
			election_id: ElectionId,
			winners: Vec<CandidateId>,
			totals: Vec<(CandidateId, u32)>,
		) -> DispatchResult {
			ensure!(method != TallyMethod::Score, Error::<T>::InvalidTallyMethod);
			Self::do_submit_tally(origin, election_id, winners, totals, method)
		}
	}

	impl<T: Config> Pallet<T> {
		/// Shared body of [`Pallet::submit_tally`] and
		/// [`Pallet::submit_tally_star`].
		fn do_submit_tally(
			origin: OriginFor<T>,
			election_id: ElectionId,
			winners: Vec<CandidateId>,
			totals: Vec<(CandidateId, u32)>,
			method: TallyMethod,
		) -> DispatchResult {
			// With a root-based TallyOrigin there is no signer account, so the
			// submitter is recorded when the origin is signed and `None` for
			// root.
			let submitted_by = ensure_signed(origin.clone()).ok();
			T::TallyOrigin::ensure_origin(origin)?;

			let election = Elections::<T>::get(election_id).ok_or(Error::<T>::ElectionNotFound)?;
			ensure!(election.phase != Phase::Finalized, Error::<T>::AlreadyFinalized);
			ensure!(
				election.phase == Phase::Voting
					&& frame_system::Pallet::<T>::block_number() >= election.voting_end,
				Error::<T>::VotingNotEnded
			);

			// Winners: exactly winner_count unique candidates.
			ensure!(
				winners.len() == election.winner_count as usize,
				Error::<T>::WinnerCountMismatch
			);
			ensure!(
				winners.iter().all(|w| election.candidates.contains(w)),
				Error::<T>::InvalidWinners
			);
			let mut sorted_winners = winners.clone();
			sorted_winners.sort_unstable();
			sorted_winners.dedup();
			ensure!(sorted_winners.len() == winners.len(), Error::<T>::InvalidWinners);

			// Totals: exactly one entry per candidate.
			ensure!(
				totals.len() == election.candidates.len(),
				Error::<T>::InvalidTotals
			);
			let mut sorted_totals: Vec<CandidateId> =
				totals.iter().map(|(c, _)| *c).collect();
			sorted_totals.sort_unstable();
			sorted_totals.dedup();
			ensure!(
				sorted_totals.len() == election.candidates.len(),
				Error::<T>::InvalidTotals
			);
			ensure!(
				sorted_totals
					.iter()
					.all(|c| election.candidates.contains(c)),
				Error::<T>::InvalidTotals
			);

			let winners = BoundedVec::try_from(winners)
				.map_err(|_| Error::<T>::WinnerCountMismatch)?;
			let winners_for_event = winners.clone().into_inner();
			let totals = BoundedVec::try_from(totals).map_err(|_| Error::<T>::InvalidTotals)?;

			let result_hash = match method {
				TallyMethod::Score => compute_result_hash(
					election_id,
					election.winner_count,
					&winners,
					&totals,
				),
				TallyMethod::BlocStar | TallyMethod::StarPr => compute_star_result_hash(
					method,
					election_id,
					election.winner_count,
					&winners,
					&totals,
				),
			};

			let submitted_at = frame_system::Pallet::<T>::block_number();
			Tally::<T>::insert(
				election_id,
				TallyResult {
					result_hash,
					method,
					winners,
					totals,
					submitted_by,
					submitted_at,
				},
			);

			// Move the election to its final phase.
			let mut election = election;
			election.phase = Phase::Finalized;
			Elections::<T>::insert(election_id, election);

			Self::deposit_event(Event::TallySubmitted {
				election_id,
				method,
				result_hash,
				winners: winners_for_event,
			});
			Ok(())
		}

		/// The election of `election_id`, if it exists.
		pub fn election(election_id: ElectionId) -> Option<Election<T>> {
			Elections::<T>::get(election_id)
		}

		/// The submitted tally result of `election_id`, if any.
		pub fn tally(election_id: ElectionId) -> Option<TallyResult<T>> {
			Tally::<T>::get(election_id)
		}
	}
}

/// The canonical digest of a tally result.
///
/// This is the single definition of `result_hash`; both the pallet and the
/// off-chain `tally-client` must use it. The encoding is plain SCALE of the
/// tuple
/// `(election_id: u32, winner_count: u16, winners: Vec<u32>, totals: Vec<(u32, u32)>)`,
/// hashed with BLAKE2b-256 (`sp_io::hashing::blake2_256`).
pub fn compute_result_hash(
	election_id: ElectionId,
	winner_count: u16,
	winners: &[CandidateId],
	totals: &[(CandidateId, u32)],
) -> [u8; 32] {
	blake2_256(&(election_id, winner_count, winners, totals).encode())
}

/// Domain tag separating the STAR-method result digest (Bloc STAR / STAR-PR)
/// from the plain score digest above. Changing it changes every STAR-method
/// result hash — both sides must stay in sync.
pub const STAR_DIGEST_TAG: &[u8] = b"pallet-star-voting/star/1";

/// The canonical digest of a STAR-method (Bloc STAR or STAR-PR) tally result.
///
/// Mirrors [`compute_result_hash`] but with a domain tag and the method in
/// the payload, so a Bloc STAR result can never be confused with a
/// score-method result:
///
/// ```text
/// result_hash = blake2_256(scale_encode((
///     tag: &[u8],            // STAR_DIGEST_TAG
///     method: TallyMethod,   // variant index (BlocStar = 1, StarPr = 2)
///     election_id: u32, winner_count: u16,
///     winners: &[u32], totals: &[(u32, u32)],
/// )))
/// ```
///
/// This is the single definition for both pallet and off-chain client; keep
/// them in sync.
pub fn compute_star_result_hash(
	method: TallyMethod,
	election_id: ElectionId,
	winner_count: u16,
	winners: &[CandidateId],
	totals: &[(CandidateId, u32)],
) -> [u8; 32] {
	blake2_256(
		&(STAR_DIGEST_TAG, method, election_id, winner_count, winners, totals).encode(),
	)
}
