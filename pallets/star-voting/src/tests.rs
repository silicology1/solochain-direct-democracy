use crate::{
	compute_result_hash, compute_star_result_hash, mock::*, ElectionId, Error, Event, Phase,
	ABSTAIN_OFFSET,
};
use frame_support::{assert_noop, assert_ok};
use frame_system::RawOrigin;

const ALICE: u64 = 1;
const BOB: u64 = 2;
const CHARLIE: u64 = 3;

const VOTING_PERIOD: u32 = 10;

fn candidates(n: u32) -> Vec<u32> {
	(1..=n).collect()
}

fn create_election(winner_count: u16, n_candidates: u32) -> ElectionId {
	System::set_block_number(1);
	assert_ok!(crate::Pallet::<Test>::create_election(
		RuntimeOrigin::signed(ALICE),
		winner_count,
		candidates(n_candidates),
		VOTING_PERIOD.into(),
	));
	0
}

/// A ballot of all-`base` scores for `n_candidates` candidates.
fn ballot(n_candidates: u32, base: u8) -> Vec<u8> {
	vec![base; n_candidates as usize]
}

/// Cast `ballot` for `who` in `election_id`.
fn vote(who: u64, election_id: ElectionId, scores: Vec<u8>) {
	assert_ok!(crate::Pallet::<Test>::vote(
		RuntimeOrigin::signed(who),
		election_id,
		scores,
	));
}

/// Submit `winners`/`totals` as root (the mock TallyOrigin).
fn submit(election_id: ElectionId, winners: Vec<u32>, totals: Vec<(u32, u32)>) {
	assert_ok!(crate::Pallet::<Test>::submit_tally(
		RawOrigin::Root.into(),
		election_id,
		winners,
		totals,
	));
}

/// Totals of `(candidate, offset_total)` covering candidates `1..=n` with
/// optional overrides. Computed from the cast ballots, like the off-chain
/// tally service would.
fn expected_totals(ballots: &[(u64, Vec<u8>)], n_candidates: u32) -> Vec<(u32, u32)> {
	(1..=n_candidates)
		.map(|c| {
			let total: u32 = ballots
				.iter()
				.filter_map(|(_, b)| {
					let offset = *b.get((c - 1) as usize)?;
					(offset != ABSTAIN_OFFSET).then_some(offset as u32)
				})
				.sum();
			(c, total)
		})
		.collect()
}

mod create_election {
	use super::*;

	#[test]
	fn creates_election_and_emits_event() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			assert_ok!(crate::Pallet::<Test>::create_election(
				RuntimeOrigin::signed(ALICE),
				5,
				candidates(10),
				VOTING_PERIOD.into(),
			));

			assert_eq!(crate::NextElectionId::<Test>::get(), 1);
			let election = crate::Pallet::<Test>::election(0).unwrap();
			assert_eq!(election.winner_count, 5);
			assert_eq!(election.candidates, candidates(10));
			assert_eq!(election.voting_end, 11u64);
			assert_eq!(election.phase, Phase::Voting);

			System::assert_last_event(
				Event::ElectionCreated { election_id: 0, winner_count: 5, candidate_count: 10, voting_end: 11 }.into(),
			);
		});
	}

	#[test]
	fn election_ids_increment() {
		new_test_ext().execute_with(|| {
			create_election(5, 10);
			create_election(6, 20);
			assert!(crate::Pallet::<Test>::election(0).is_some());
			assert!(crate::Pallet::<Test>::election(1).is_some());
			assert!(crate::Pallet::<Test>::election(2).is_none());
		});
	}

	#[test]
	fn rejects_winner_count_out_of_bounds() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				crate::Pallet::<Test>::create_election(
					RuntimeOrigin::signed(ALICE),
					4,
					candidates(10),
					VOTING_PERIOD.into(),
				),
				Error::<Test>::InvalidWinnerCount,
			);
			assert_noop!(
				crate::Pallet::<Test>::create_election(
					RuntimeOrigin::signed(ALICE),
					501,
					candidates(600),
					VOTING_PERIOD.into(),
				),
				Error::<Test>::InvalidWinnerCount,
			);
		});
	}

	#[test]
	fn requires_at_least_winner_count_candidates() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				crate::Pallet::<Test>::create_election(
					RuntimeOrigin::signed(ALICE),
					5,
					candidates(4),
					VOTING_PERIOD.into(),
				),
				Error::<Test>::TooFewCandidates,
			);
		});
	}

	#[test]
	fn rejects_duplicate_candidates() {
		new_test_ext().execute_with(|| {
			let mut c = candidates(10);
			c[9] = 1; // duplicate of candidate 1
			assert_noop!(
				crate::Pallet::<Test>::create_election(
					RuntimeOrigin::signed(ALICE),
					5,
					c,
					VOTING_PERIOD.into(),
				),
				Error::<Test>::DuplicateCandidate,
			);
		});
	}

	#[test]
	fn rejects_too_many_candidates() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				crate::Pallet::<Test>::create_election(
					RuntimeOrigin::signed(ALICE),
					5,
					candidates(1001),
					VOTING_PERIOD.into(),
				),
				Error::<Test>::TooManyCandidates,
			);
		});
	}
}

mod vote {
	use super::*;

	#[test]
	fn stores_ballot_and_counts_voters() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			vote(ALICE, election_id, ballot(10, 10));
			vote(BOB, election_id, ballot(10, 0));

			let alice_ballot = crate::Ballots::<Test>::get((election_id, ALICE)).unwrap();
			assert_eq!(alice_ballot, ballot(10, 10));
			assert_eq!(crate::BallotCount::<Test>::get(election_id), 2);

			System::assert_has_event(Event::VoteCast { election_id, who: ALICE }.into());
		});
	}

	#[test]
	fn rejects_double_voting() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			vote(ALICE, election_id, ballot(10, 10));
			assert_noop!(
				crate::Pallet::<Test>::vote(
					RuntimeOrigin::signed(ALICE),
					election_id,
					ballot(10, 5),
				),
				Error::<Test>::AlreadyVoted,
			);
		});
	}

	#[test]
	fn rejects_scores_out_of_range() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			// 11 = raw score +6, invalid.
			let mut scores = ballot(10, 5);
			scores[0] = 11;
			assert_noop!(
				crate::Pallet::<Test>::vote(RuntimeOrigin::signed(ALICE), election_id, scores),
				Error::<Test>::InvalidScore,
			);
			// 254 is neither a valid offset nor the abstain marker.
			let mut scores = ballot(10, 5);
			scores[0] = 254;
			assert_noop!(
				crate::Pallet::<Test>::vote(RuntimeOrigin::signed(ALICE), election_id, scores),
				Error::<Test>::InvalidScore,
			);
		});
	}

	#[test]
	fn accepts_abstain_marker() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			let mut scores = ballot(10, 5);
			scores[0] = ABSTAIN_OFFSET;
			assert_ok!(crate::Pallet::<Test>::vote(RuntimeOrigin::signed(ALICE), election_id, scores));
			assert_eq!(crate::Ballots::<Test>::get((election_id, ALICE)).unwrap()[0], ABSTAIN_OFFSET);
		});
	}

	#[test]
	fn rejects_wrong_ballot_length() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			assert_noop!(
				crate::Pallet::<Test>::vote(RuntimeOrigin::signed(ALICE), election_id, ballot(9, 5)),
				Error::<Test>::BallotLengthMismatch,
			);
			assert_noop!(
				crate::Pallet::<Test>::vote(RuntimeOrigin::signed(ALICE), election_id, ballot(11, 5)),
				Error::<Test>::BallotLengthMismatch,
			);
		});
	}

	#[test]
	fn rejects_after_voting_period() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			// voting_end = 1 + 10 = 11; block 10 is the last valid block.
			System::set_block_number(10);
			vote(ALICE, election_id, ballot(10, 5));
			System::set_block_number(11);
			assert_noop!(
				crate::Pallet::<Test>::vote(RuntimeOrigin::signed(BOB), election_id, ballot(10, 5)),
				Error::<Test>::VotingClosed,
			);
		});
	}

	#[test]
	fn rejects_unknown_election() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				crate::Pallet::<Test>::vote(RuntimeOrigin::signed(ALICE), 42, ballot(10, 5)),
				Error::<Test>::ElectionNotFound,
			);
		});
	}

	#[test]
	fn rejects_unsigned_origin() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			assert_noop!(
				crate::Pallet::<Test>::vote(RuntimeOrigin::none(), election_id, ballot(10, 5)),
				sp_runtime::DispatchError::BadOrigin,
			);
		});
	}
}

/// Cross-check against the client: prints the digest so the value can be
/// embedded in `tally-client/src/digest.rs`. Run with
/// `cargo test -p pallet-star-voting digest_cross_check -- --nocapture`.
#[test]
fn digest_cross_check() {
	let winners = vec![5u32, 4, 3, 2, 1];
	let totals = vec![(1u32, 0u32), (2, 0), (3, 0), (4, 0), (5, 0)];
	println!(
		"digest_cross_check result_hash: 0x{}",
		sp_core::hexdisplay::HexDisplay::from(&compute_result_hash(0, 5, &winners, &totals))
	);
}

mod submit_tally {
	use super::*;

	/// Election with 2 candidates ranking by signed total: candidate 10 first.
	fn setup_3_voters_election() -> ElectionId {
		let election_id = create_election(5, 10);
		// Candidate 10: three +5 votes (offset 10) = real +15
		// Candidate 1..9 get one -5 vote (offset 0) each from different voters.
		vote(ALICE, election_id, vec![10, 0, 0, 0, 0, 0, 0, 0, 0, 10]);
		vote(BOB, election_id, vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 10]);
		vote(CHARLIE, election_id, vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 10]);
		System::set_block_number(11);
		election_id
	}

	#[test]
	fn binds_result_hash_and_finalizes() {
		new_test_ext().execute_with(|| {
			let election_id = setup_3_voters_election();

			let winners = (5..10).collect::<Vec<u32>>();
			let totals = expected_totals(
				&[
					(ALICE, vec![10, 0, 0, 0, 0, 0, 0, 0, 0, 10]),
					(BOB, vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 10]),
					(CHARLIE, vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 10]),
				],
				10,
			);
			submit(election_id, winners.clone(), totals.clone());

			let result = crate::Pallet::<Test>::tally(election_id).unwrap();
			assert_eq!(result.winners, winners);
			assert_eq!(result.totals, totals);
			assert_eq!(result.submitted_at, 11);
			assert_eq!(result.method, crate::TallyMethod::Score);
			assert_eq!(result.result_hash, compute_result_hash(election_id, 5, &winners, &totals));

			assert_eq!(crate::Pallet::<Test>::election(election_id).unwrap().phase, Phase::Finalized);

			System::assert_last_event(
				Event::TallySubmitted {
					election_id,
					method: crate::TallyMethod::Score,
					result_hash: result.result_hash,
					winners,
				}
				.into(),
			);
		});
	}

	#[test]
	fn result_hash_is_deterministic_and_payload_sensitive() {
		let winners_a = vec![1u32, 2, 3, 4, 5];
		let totals_a = vec![(1u32, 0u32), (2, 0), (3, 0), (4, 0), (5, 0)];
		let h1 = compute_result_hash(0, 5, &winners_a, &totals_a);
		let h2 = compute_result_hash(0, 5, &winners_a, &totals_a);
		assert_eq!(h1, h2);

		let winners_b = vec![5u32, 4, 3, 2, 1];
		let h3 = compute_result_hash(0, 5, &winners_b, &totals_a);
		assert_ne!(h1, h3);

		let totals_b = vec![(1u32, 1u32), (2, 0), (3, 0), (4, 0), (5, 0)];
		let h4 = compute_result_hash(0, 5, &winners_a, &totals_b);
		assert_ne!(h1, h4);
	}

	#[test]
	fn rejects_before_voting_ends() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			// Still in the voting window (block 1 < voting_end 11).
			assert_noop!(
				crate::Pallet::<Test>::submit_tally(
					RawOrigin::Root.into(),
					election_id,
					candidates(5),
					expected_totals(&[], 10),
				),
				Error::<Test>::VotingNotEnded,
			);
		});
	}

	#[test]
	fn rejects_wrong_winner_count() {
		new_test_ext().execute_with(|| {
			let election_id = setup_3_voters_election();
			assert_noop!(
				crate::Pallet::<Test>::submit_tally(
					RawOrigin::Root.into(),
					election_id,
					candidates(4),
					expected_totals(&[], 10),
				),
				Error::<Test>::WinnerCountMismatch,
			);
			assert_noop!(
				crate::Pallet::<Test>::submit_tally(
					RawOrigin::Root.into(),
					election_id,
					candidates(6),
					expected_totals(&[], 10),
				),
				Error::<Test>::WinnerCountMismatch,
			);
		});
	}

	#[test]
	fn rejects_unknown_or_duplicate_winners() {
		new_test_ext().execute_with(|| {
			let election_id = setup_3_voters_election();

			// Candidate 99 is not a candidate of this election.
			assert_noop!(
				crate::Pallet::<Test>::submit_tally(
					RawOrigin::Root.into(),
					election_id,
					vec![99, 2, 3, 4, 5],
					expected_totals(&[], 10),
				),
				Error::<Test>::InvalidWinners,
			);

			// Duplicate winner.
			assert_noop!(
				crate::Pallet::<Test>::submit_tally(
					RawOrigin::Root.into(),
					election_id,
					vec![1, 1, 2, 3, 4],
					expected_totals(&[], 10),
				),
				Error::<Test>::InvalidWinners,
			);
		});
	}

	#[test]
	fn rejects_incomplete_totals() {
		new_test_ext().execute_with(|| {
			let election_id = setup_3_voters_election();

			// Missing one candidate's total.
			assert_noop!(
				crate::Pallet::<Test>::submit_tally(
					RawOrigin::Root.into(),
					election_id,
					candidates(5),
					expected_totals(&[], 9),
				),
				Error::<Test>::InvalidTotals,
			);

			// Duplicate entry for one candidate.
			let mut totals = expected_totals(&[], 10);
			totals[9] = totals[0];
			assert_noop!(
				crate::Pallet::<Test>::submit_tally(
					RawOrigin::Root.into(),
					election_id,
					candidates(5),
					totals,
				),
				Error::<Test>::InvalidTotals,
			);

			// Entry for a candidate that does not exist.
			let mut totals = expected_totals(&[], 10);
			totals[0] = (99, 5);
			assert_noop!(
				crate::Pallet::<Test>::submit_tally(
					RawOrigin::Root.into(),
					election_id,
					candidates(5),
					totals,
				),
				Error::<Test>::InvalidTotals,
			);
		});
	}

	#[test]
	fn rejects_second_submission() {
		new_test_ext().execute_with(|| {
			let election_id = setup_3_voters_election();
			let totals = expected_totals(&[], 10);
			submit(election_id, candidates(5), totals);
			assert_noop!(
				crate::Pallet::<Test>::submit_tally(
					RawOrigin::Root.into(),
					election_id,
					candidates(5),
					expected_totals(&[], 10),
				),
				Error::<Test>::AlreadyFinalized,
			);
		});
	}

	#[test]
	fn rejects_non_tally_origin() {
		new_test_ext().execute_with(|| {
			let election_id = setup_3_voters_election();
			assert_noop!(
				crate::Pallet::<Test>::submit_tally(
					RuntimeOrigin::signed(ALICE),
					election_id,
					candidates(5),
					expected_totals(&[], 10),
				),
				sp_runtime::DispatchError::BadOrigin,
			);
		});
	}
}

mod submit_tally_star {
	use super::*;

	fn submit_star(
		election_id: ElectionId,
		method: crate::TallyMethod,
		winners: Vec<u32>,
		totals: Vec<(u32, u32)>,
	) {
		assert_ok!(crate::Pallet::<Test>::submit_tally_star(
			RawOrigin::Root.into(),
			method,
			election_id,
			winners,
			totals,
		));
	}

	#[test]
	fn binds_star_hash_and_records_method() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			vote(ALICE, election_id, ballot(10, 10));
			System::set_block_number(11);

			let winners = (5..10).collect::<Vec<u32>>();
			let totals = expected_totals(&[(ALICE, ballot(10, 10))], 10);
			submit_star(election_id, crate::TallyMethod::BlocStar, winners.clone(), totals.clone());

			let result = crate::Pallet::<Test>::tally(election_id).unwrap();
			assert_eq!(result.method, crate::TallyMethod::BlocStar);
			assert_eq!(
				result.result_hash,
				compute_star_result_hash(crate::TallyMethod::BlocStar, election_id, 5, &winners, &totals)
			);
			assert_eq!(crate::Pallet::<Test>::election(election_id).unwrap().phase, Phase::Finalized);

			System::assert_last_event(
				Event::TallySubmitted {
					election_id,
					method: crate::TallyMethod::BlocStar,
					result_hash: result.result_hash,
					winners,
				}
				.into(),
			);
		});
	}

	#[test]
	fn rejects_score_method() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			System::set_block_number(11);
			assert_noop!(
				crate::Pallet::<Test>::submit_tally_star(
					RawOrigin::Root.into(),
					crate::TallyMethod::Score,
					election_id,
					candidates(5),
					expected_totals(&[], 10),
				),
				Error::<Test>::InvalidTallyMethod,
			);
		});
	}

	#[test]
	fn star_hash_differs_from_score_hash() {
		// Domain separation: the same payload must produce different digests
		// under the score and STAR-method definitions.
		let winners = vec![5u32, 4, 3, 2, 1];
		let totals = expected_totals(&[], 10);
		assert_ne!(
			compute_result_hash(0, 5, &winners, &totals),
			compute_star_result_hash(crate::TallyMethod::BlocStar, 0, 5, &winners, &totals)
		);
		assert_ne!(
			compute_star_result_hash(crate::TallyMethod::BlocStar, 0, 5, &winners, &totals),
			compute_star_result_hash(crate::TallyMethod::StarPr, 0, 5, &winners, &totals)
		);
	}

	#[test]
	fn rejects_before_voting_ends() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			System::set_block_number(5);
			assert_noop!(
				crate::Pallet::<Test>::submit_tally_star(
					RawOrigin::Root.into(),
					crate::TallyMethod::StarPr,
					election_id,
					candidates(5),
					expected_totals(&[], 10),
				),
				Error::<Test>::VotingNotEnded,
			);
		});
	}

	#[test]
	fn rejects_second_submission() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			System::set_block_number(11);
			submit_star(election_id, crate::TallyMethod::BlocStar, candidates(5), expected_totals(&[], 10));
			assert_noop!(
				crate::Pallet::<Test>::submit_tally_star(
					RawOrigin::Root.into(),
					crate::TallyMethod::StarPr,
					election_id,
					candidates(5),
					expected_totals(&[], 10),
				),
				Error::<Test>::AlreadyFinalized,
			);
		});
	}

	#[test]
	fn rejects_non_tally_origin() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			System::set_block_number(11);
			assert_noop!(
				crate::Pallet::<Test>::submit_tally_star(
					RuntimeOrigin::signed(ALICE),
					crate::TallyMethod::BlocStar,
					election_id,
					candidates(5),
					expected_totals(&[], 10),
				),
				sp_runtime::DispatchError::BadOrigin,
			);
		});
	}
}

/// Cross-check against the client: prints the STAR-method digests so the
/// values can be embedded in `tally-client/src/digest.rs`. Run with
/// `cargo test -p pallet-star-voting star_digest_cross_check -- --nocapture`.
#[test]
fn star_digest_cross_check() {
	let winners = vec![5u32, 4, 3, 2, 1];
	let totals = vec![(1u32, 0u32), (2, 0), (3, 0), (4, 0), (5, 0)];
	let bloc = compute_star_result_hash(crate::TallyMethod::BlocStar, 0, 5, &winners, &totals);
	let pr = compute_star_result_hash(crate::TallyMethod::StarPr, 0, 5, &winners, &totals);
	println!(
		"star_digest_cross_check BlocStar result_hash: 0x{}",
		sp_core::hexdisplay::HexDisplay::from(&bloc)
	);
	println!(
		"star_digest_cross_check StarPr result_hash: 0x{}",
		sp_core::hexdisplay::HexDisplay::from(&pr)
	);
}

mod ballot_totals {
	use super::*;

	/// Mirrors the off-chain service: abstained candidates contribute nothing,
	/// offsets sum into offset totals.
	#[test]
	fn computes_expected_totals_like_the_service() {
		new_test_ext().execute_with(|| {
			let election_id = create_election(5, 10);
			// ALICE: candidate 1 => +5 (offset 10), candidate 2 => -5 (offset 0),
			// candidate 3 abstains.
			vote(ALICE, election_id, vec![10, 0, ABSTAIN_OFFSET, 0, 0, 0, 0, 0, 0, 0]);
			vote(BOB, election_id, vec![6, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // candidate 1: +1

			let ballots = vec![
				(ALICE, vec![10u8, 0, ABSTAIN_OFFSET, 0, 0, 0, 0, 0, 0, 0]),
				(BOB, vec![6, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
			];
			let totals = expected_totals(&ballots, 10);
			assert_eq!(totals[0], (1, 16)); // 10 + 6
			assert_eq!(totals[1], (2, 0)); // only -5 vote counts as offset 0
			assert_eq!(totals[2], (3, 0)); // abstention contributes nothing
			assert_eq!(totals[9], (10, 0));

			System::set_block_number(11);
			submit(election_id, candidates(5), totals);
			assert_eq!(crate::Pallet::<Test>::tally(election_id).unwrap().totals[0], (1, 16));
		});
	}
}
