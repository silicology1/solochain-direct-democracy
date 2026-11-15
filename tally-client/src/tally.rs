//! The multi-winner STAR tally algorithms, mirroring the on-chain ballot
//! format and the digests defined by `pallet-star-voting`.
//!
//! Three tally methods are supported, selectable with [`TallyMethod`]:
//!
//! * [`TallyMethod::Score`] — plain multi-winner score voting: top-W by
//!   real signed total score, ties broken by smaller candidate id.
//! * [`TallyMethod::BlocStar`] — bloc STAR: per seat, take the top-2
//!   remaining candidates by score, run an automatic runoff between them,
//!   seat the majority-preference winner, remove, repeat.
//! * [`TallyMethod::StarPr`] — STAR-PR (Allocated Score): proportional
//!   representation by sequential fractional ballot allocation.

use anyhow::{bail, Result};
use parity_scale_codec::Encode;

use crate::digest::{compute_result_hash, compute_star_result_hash};

/// Score offset meaning "deliberately abstained" for one candidate.
pub const ABSTAIN_OFFSET: u8 = 255;
/// Maximum valid score offset (raw score + 5).
pub const MAX_SCORE_OFFSET: u8 = 10;

/// The tally method a result was computed with.
///
/// Must stay variant-order-identical with the pallet's `TallyMethod` (the
/// SCALE encoding is part of the STAR result digest) and with
/// `runtime_types::pallet_star_voting::pallet::TallyMethod`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode)]
pub enum TallyMethod {
	/// Plain multi-winner score voting.
	Score,
	/// Bloc STAR (per-seat top-2 + automatic runoff).
	BlocStar,
	/// STAR-PR (Allocated Score, proportional).
	StarPr,
}

impl TallyMethod {
	/// Lowercase CLI name.
	pub fn as_str(&self) -> &'static str {
		match self {
			TallyMethod::Score => "score",
			TallyMethod::BlocStar => "bloc-star",
			TallyMethod::StarPr => "star-pr",
		}
	}

	/// Parse a CLI name.
	pub fn from_str(s: &str) -> Result<Self> {
		match s {
			"score" => Ok(TallyMethod::Score),
			"bloc-star" => Ok(TallyMethod::BlocStar),
			"star-pr" => Ok(TallyMethod::StarPr),
			_ => bail!("unknown tally method {s:?} (expected score, bloc-star or star-pr)"),
		}
	}
}

/// A ballot as stored on-chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ballot {
	/// Voter account id (32 bytes for our runtime's `AccountId32`).
	pub voter: [u8; 32],
	/// One score offset per candidate, in candidate order:
	/// `0..=10` (raw score + 5) or [`ABSTAIN_OFFSET`].
	pub scores: Vec<u8>,
}

/// The full tally result for an election.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TallyOutput {
	/// Winning candidate ids, in ranked (seating) order.
	pub winners: Vec<u32>,
	/// Offset-based total score per candidate, covering every candidate
	/// exactly once (this is exactly what the pallet's `submit_tally` /
	/// `submit_tally_star` take, for every method).
	pub totals: Vec<(u32, u32)>,
}

impl TallyOutput {
	/// The on-chain result hash this output binds, per its method.
	pub fn result_hash(&self, election_id: u32, winner_count: u16, method: TallyMethod) -> [u8; 32] {
		match method {
			TallyMethod::Score => {
				compute_result_hash(election_id, winner_count, &self.winners, &self.totals)
			},
			TallyMethod::BlocStar | TallyMethod::StarPr => compute_star_result_hash(
				method,
				election_id,
				winner_count,
				&self.winners,
				&self.totals,
			),
		}
	}
}

/// Validate the common tally preconditions.
fn validate_winner_count(candidates: &[u32], winner_count: usize) -> Result<()> {
	if winner_count == 0 {
		bail!("winner_count must be at least 1");
	}
	if winner_count > candidates.len() {
		bail!(
			"winner_count ({winner_count}) exceeds number of candidates ({})",
			candidates.len()
		);
	}
	Ok(())
}

/// The score stage shared by every method: per-candidate offset totals and
/// the number of non-abstaining votes, with ballot validation.
///
/// Abstentions (`255`) are excluded from both the total and the count, so a
/// candidate a voter deliberately abstained on neither gains nor loses score
/// and the score scale stays comparable between voters.
fn score_stage(candidates: &[u32], ballots: &[Ballot]) -> Result<(Vec<u64>, Vec<u64>)> {
	let mut offset_totals = vec![0u64; candidates.len()];
	let mut counts = vec![0u64; candidates.len()];

	for ballot in ballots {
		if ballot.scores.len() != candidates.len() {
			bail!(
				"ballot of voter 0x{} has {} scores but the election has {} candidates",
				hex::encode(ballot.voter),
				ballot.scores.len(),
				candidates.len()
			);
		}
		for (i, &offset) in ballot.scores.iter().enumerate() {
			if offset == ABSTAIN_OFFSET {
				continue;
			}
			if offset > MAX_SCORE_OFFSET {
				bail!(
					"ballot of voter 0x{} has invalid score offset {offset}",
					hex::encode(ballot.voter)
				);
			}
			offset_totals[i] += offset as u64;
			counts[i] += 1;
		}
	}

	Ok((offset_totals, counts))
}

/// Real signed total per candidate (`offset_total - 5 * count`), sorted by
/// real total descending, ties broken by the smaller candidate id.
fn ranked_by_real(candidates: &[u32], offset_totals: &[u64], counts: &[u64]) -> Vec<(u32, i64)> {
	let mut ranked: Vec<(u32, i64)> = candidates
		.iter()
		.copied()
		.zip(offset_totals.iter().zip(counts.iter()))
		.map(|(c, (&offset_total, &count))| (c, offset_total as i64 - 5 * count as i64))
		.collect();
	ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
	ranked
}

/// Offset totals in `(candidate, total)` form, in candidate order.
fn totals_output(candidates: &[u32], offset_totals: &[u64]) -> Vec<(u32, u32)> {
	candidates
		.iter()
		.copied()
		.zip(offset_totals.iter())
		.map(|(c, &t)| (c, t as u32))
		.collect()
}

/// Tally `ballots` for an election with `candidates` and `winner_count`.
///
/// Ranking rule (score method): candidates are ranked by their *real* signed
/// total score `real_total[c] = offset_total[c] - 5 * count[c]`, which is
/// what voters expressed (each vote contributes `-5..+5`). Ties are broken by
/// the smaller candidate id. The top `winner_count` candidates win.
pub fn tally(candidates: &[u32], ballots: &[Ballot], winner_count: usize) -> Result<TallyOutput> {
	validate_winner_count(candidates, winner_count)?;
	let (offset_totals, counts) = score_stage(candidates, ballots)?;
	let winners: Vec<u32> = ranked_by_real(candidates, &offset_totals, &counts)
		.into_iter()
		.take(winner_count)
		.map(|(c, _)| c)
		.collect();
	Ok(TallyOutput { winners, totals: totals_output(candidates, &offset_totals) })
}

/// Bloc STAR tally: per seat, the top-2 remaining candidates by real signed
/// total score go to an automatic runoff, and the majority-preference winner
/// is seated and removed; repeated `winner_count` times.
///
/// Runoff rules (deterministic):
/// * each ballot supports the finalist it scored **higher** (comparing raw
///   scores; equal scores support neither);
/// * an explicit abstention (`255`) on one finalist and a real score on the
///   other supports the *scored* finalist (an abstain means "no score", not
///   "equal to everything");
/// * two abstentions or equal scores support neither;
/// * a runoff tie (equal support) is won by the finalist that ranked first
///   in the score stage (higher real total, then smaller candidate id);
/// * when only one candidate remains, it is seated without a runoff.
pub fn bloc_star_tally(
	candidates: &[u32],
	ballots: &[Ballot],
	winner_count: usize,
) -> Result<TallyOutput> {
	validate_winner_count(candidates, winner_count)?;
	let (offset_totals, counts) = score_stage(candidates, ballots)?;
	let ranked = ranked_by_real(candidates, &offset_totals, &counts);
	// Candidate id -> score offset index, for O(1) per-ballot lookup.
	let index_of: std::collections::HashMap<u32, usize> =
		candidates.iter().copied().enumerate().map(|(i, c)| (c, i)).collect();

	let mut winners = Vec::with_capacity(winner_count);
	// Remaining candidate ids in score-stage ranked order.
	let mut remaining: Vec<u32> = ranked.iter().map(|(c, _)| *c).collect();

	while winners.len() < winner_count {
		if remaining.len() <= winner_count - winners.len() {
			// Every remaining candidate gets a seat; seat in ranked order.
			winners.extend(remaining.iter().copied());
			break;
		}

		// The two finalists: first and second in ranked order among the
		// remaining candidates.
		let (a, b) = (remaining[0], remaining[1]);
		let (ia, ib) = (index_of[&a], index_of[&b]);

		// Automatic runoff: count each ballot's preference.
		let mut support_a = 0i64;
		let mut support_b = 0i64;
		for ballot in ballots {
			let (sa, sb) = (ballot.scores[ia], ballot.scores[ib]);
			let pref = match (sa, sb) {
				(ABSTAIN_OFFSET, ABSTAIN_OFFSET) => None,
				(ABSTAIN_OFFSET, _) => Some(false),  // scored finalist wins
				(_, ABSTAIN_OFFSET) => Some(true),
				(sa, sb) if sa > sb => Some(true),
				(sa, sb) if sb > sa => Some(false),
				_ => None, // equal scores: no support for either
			};
			match pref {
				Some(true) => support_a += 1,
				Some(false) => support_b += 1,
				None => {},
			}
		}

		// Majority wins; a tie goes to the score-stage leader (`a`).
		let winner = if support_a >= support_b { a } else { b };
		winners.push(winner);
		remaining.retain(|&c| c != winner);
	}

	Ok(TallyOutput { winners, totals: totals_output(candidates, &offset_totals) })
}

/// STAR-PR (Allocated Score) tally: proportional representation by
/// sequential fractional ballot allocation. Each voter starts with weight 1.
///
/// Round rules (deterministic):
/// * quota `H = total remaining ballot weight / seats remaining`;
/// * each candidate is allocated ballots that scored it **above 0**
///   (raw score `1..=5`, offsets `6..=10`), highest score first (ties in
///   ascending voter-account order), until its total reaches `H`; the ballot
///   crossing `H` is allocated only fractionally. A candidate's allocated
///   support is therefore `min(H, Σ weights of its positive-score ballots)`;
/// * the seat goes to the candidate with the highest allocated support,
///   ties broken by the smaller candidate id;
/// * only the winner's allocated ballots are spent (fully allocated ballots
///   lose all weight, the boundary ballot keeps its remainder);
/// * when the number of remaining candidates equals the number of remaining
///   seats, all of them are seated in score-stage ranked order.
pub fn star_pr_tally(
	candidates: &[u32],
	ballots: &[Ballot],
	winner_count: usize,
) -> Result<TallyOutput> {
	validate_winner_count(candidates, winner_count)?;
	let (offset_totals, counts) = score_stage(candidates, ballots)?;
	let ranked = ranked_by_real(candidates, &offset_totals, &counts);

	// Fixed-point ballot weights: 1.0 vote == WEIGHT_UNIT microvotes.
	const WEIGHT_UNIT: u64 = 1_000_000_000_000;
	let mut weights: Vec<u64> = vec![WEIGHT_UNIT; ballots.len()];
	let total_weight = |weights: &[u64]| weights.iter().sum::<u64>();

	// Per candidate: ballots scoring it above zero, sorted by score
	// descending, then voter account ascending (deterministic).
	let candidate_supporters = |candidate_index: usize| -> Vec<(usize, u8)> {
		let mut supporters: Vec<(usize, u8)> = ballots
			.iter()
			.enumerate()
			.filter_map(|(i, b)| {
				let s = b.scores[candidate_index];
				(s != ABSTAIN_OFFSET && s > 5).then_some((i, s))
			})
			.collect();
		supporters.sort_by(|a, b| b.1.cmp(&a.1).then(ballots[a.0].voter.cmp(&ballots[b.0].voter)));
		supporters
	};

	let mut winners = Vec::with_capacity(winner_count);
	// Remaining candidate ids in score-stage ranked order.
	let mut remaining: Vec<u32> = ranked.iter().map(|(c, _)| *c).collect();
	let index_of: std::collections::HashMap<u32, usize> =
		candidates.iter().copied().enumerate().map(|(i, c)| (c, i)).collect();

	let mut seats_left = winner_count;
	while seats_left > 0 {
		if remaining.len() <= seats_left {
			// Every remaining candidate wins a seat; seat in ranked order.
			winners.extend(remaining.iter());
			break;
		}

		let quota = total_weight(&weights) / seats_left as u64;

		// Allocated support per remaining candidate.
		let mut best: Option<(u32, u64, usize)> = None; // (id, allocated, candidate_index)
		for &c in &remaining {
			let ci = index_of[&c];
			let supporters = candidate_supporters(ci);
			let available: u64 = supporters.iter().map(|&(i, _)| weights[i]).sum();
			let allocated = available.min(quota);
			// Tie -> smaller candidate id (candidates are visited in ranked
			// order, so keep the first on ties).
			if best.map_or(true, |(_, best_alloc, _)| allocated > best_alloc) {
				best = Some((c, allocated, ci));
			}
		}
		let (winner, allocated, winner_index) = best.expect("remaining is non-empty");

		// Spend the winner's allocated weight from its ballots, in the
		// deterministic allocation order.
		let mut to_spend = allocated;
		for (ballot_index, _) in candidate_supporters(winner_index) {
			if to_spend == 0 {
				break;
			}
			let take = weights[ballot_index].min(to_spend);
			weights[ballot_index] -= take;
			to_spend -= take;
		}

		winners.push(winner);
		remaining.retain(|&c| c != winner);
		seats_left -= 1;
	}

	Ok(TallyOutput { winners, totals: totals_output(candidates, &offset_totals) })
}

/// Tally with the given method.
pub fn tally_with_method(
	method: TallyMethod,
	candidates: &[u32],
	ballots: &[Ballot],
	winner_count: usize,
) -> Result<TallyOutput> {
	match method {
		TallyMethod::Score => tally(candidates, ballots, winner_count),
		TallyMethod::BlocStar => bloc_star_tally(candidates, ballots, winner_count),
		TallyMethod::StarPr => star_pr_tally(candidates, ballots, winner_count),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn ballot(voter: u8, scores: &[u8]) -> Ballot {
		let mut voter_id = [0u8; 32];
		voter_id[0] = voter;
		Ballot { voter: voter_id, scores: scores.to_vec() }
	}

	// ------------------------------------------------------------------
	// Score method
	// ------------------------------------------------------------------

	#[test]
	fn ranks_by_signed_total_not_offset_total() {
		// Candidate 30: 20 votes of +1 -> offset 120, real 120 - 100 = 20.
		// Candidate 31: 5 votes of +5  -> offset 50,  real 50 - 25 = 25.
		// Ranking by offset total would put 30 first; ranking by *real*
		// signed total puts 31 first.
		let mut ballots = Vec::new();
		for v in 0..20u8 {
			ballots.push(ballot(v, &[6, ABSTAIN_OFFSET]));
		}
		for v in 20..25u8 {
			ballots.push(ballot(v, &[ABSTAIN_OFFSET, 10]));
		}

		let out = tally(&[30, 31], &ballots, 2).unwrap();
		assert_eq!(out.totals, vec![(30, 120), (31, 50)]);
		assert_eq!(out.winners, vec![31, 30]);
	}

	#[test]
	fn tie_broken_by_smaller_candidate_id() {
		let ballots = vec![ballot(1, &[10, 10]), ballot(2, &[10, 10])];
		let out = tally(&[30, 31], &ballots, 2).unwrap();
		assert_eq!(out.winners, vec![30, 31]);
	}

	#[test]
	fn abstentions_are_excluded() {
		// Voter abstains on candidate 1 and scores candidate 2 at -5.
		let ballots = vec![ballot(1, &[ABSTAIN_OFFSET, 0])];
		let out = tally(&[1, 2], &ballots, 2).unwrap();
		// Both candidates end at real total 0; tie -> smaller id first.
		assert_eq!(out.totals, vec![(1, 0), (2, 0)]);
		assert_eq!(out.winners, vec![1, 2]);
	}

	#[test]
	fn negative_scores_lose_to_zero_vote_candidates() {
		// Candidate 1: two votes of -5 -> real -10.
		// Candidate 2: both voters abstain -> real 0.
		let ballots = vec![ballot(1, &[0, ABSTAIN_OFFSET]), ballot(2, &[0, ABSTAIN_OFFSET])];
		let out = tally(&[1, 2], &ballots, 2).unwrap();
		assert_eq!(out.totals, vec![(1, 0), (2, 0)]);
		assert_eq!(out.winners, vec![2, 1]);
	}

	#[test]
	fn basic_three_voter_election() {
		// Scores: candidate 1: (+5, +1, -5) = +1; candidate 2: (+5, +5, +5) = +15.
		let ballots = vec![
			ballot(1, &[10, 10]),
			ballot(2, &[6, 10]),
			ballot(3, &[0, 10]),
		];
		let out = tally(&[1, 2], &ballots, 1).unwrap();
		assert_eq!(out.totals, vec![(1, 16), (2, 30)]);
		assert_eq!(out.winners, vec![2]);
	}

	#[test]
	fn rejects_malformed_ballots() {
		let ballots = vec![ballot(1, &[0])];
		assert!(tally(&[1, 2], &ballots, 2).is_err());
		let ballots = vec![ballot(1, &[11, 0])];
		assert!(tally(&[1, 2], &ballots, 2).is_err());
	}

	// ------------------------------------------------------------------
	// Bloc STAR
	// ------------------------------------------------------------------

	#[test]
	fn bloc_star_runoff_beats_score_leader() {
		// The STAR showcase: score and runoff disagree.
		// 60 voters: candidate 30 = +2, candidate 31 = +5 (prefer 31).
		// 40 voters: candidate 30 = +5, candidate 31 = -5 (prefer 30).
		// Score stage: 30 real = 60*2 + 40*5 = +320 leads 31 real
		// = 60*5 - 40*5 = +100 -> score-only seats 30.
		// Runoff: 60 ballots prefer 31, 40 prefer 30 -> 31 wins the runoff.
		let mut ballots = Vec::new();
		for v in 0..60u8 {
			ballots.push(ballot(v, &[7, 10]));
		}
		for v in 60..100u8 {
			ballots.push(ballot(v, &[10, 0]));
		}
		let score_out = tally(&[30, 31], &ballots, 1).unwrap();
		assert_eq!(score_out.winners, vec![30]);
		let out = bloc_star_tally(&[30, 31], &ballots, 1).unwrap();
		assert_eq!(out.winners, vec![31]);
		// The score totals submitted on-chain are identical either way.
		assert_eq!(out.totals, score_out.totals);
	}

	#[test]
	fn bloc_star_runoff_tie_goes_to_score_leader() {
		// 2 voters prefer candidate 1, 2 voters prefer candidate 2 ->
		// runoff tie; score stage: candidate 2 has higher real total
		// (+10 vs -10) but candidate 1 ranked FIRST in the stage order?
		// No: real totals 1 = 2*5 - 2*5 = 0? Let's be precise:
		// voters 1,2: candidate 1 = +5, candidate 2 = 0.
		// voters 3,4: candidate 1 = 0, candidate 2 = +5.
		// Real totals: both 0 -> tie -> candidate 1 ranks first.
		// Runoff: 2 vs 2 -> tie -> score-stage leader (candidate 1) wins.
		let ballots = vec![
			ballot(1, &[10, 5]),
			ballot(2, &[10, 5]),
			ballot(3, &[5, 10]),
			ballot(4, &[5, 10]),
		];
		let out = bloc_star_tally(&[1, 2], &ballots, 1).unwrap();
		assert_eq!(out.winners, vec![1]);
		// Same election, ids swapped: the higher id loses the tie.
		let out = bloc_star_tally(&[2, 1], &ballots, 1).unwrap();
		// Careful: candidate order determines index, not ranking. Real
		// totals are equal, so ranked order is [1, 2] regardless; the
		// runoff tie goes to candidate 1 again.
		assert_eq!(out.winners, vec![1]);
	}

	#[test]
	fn bloc_star_abstain_vs_scored_supports_scored() {
		// 3 voters: candidate 1 = +5, candidate 2 = abstain.
		// 2 voters: candidate 1 = abstain, candidate 2 = +5.
		// Score: candidate 1 = +15; candidate 2 = +10. 1 leads.
		// Runoff: scored finalist wins the abstain ballots: voters 1-3
		// prefer 1, voters 4-5 prefer 2 -> 3-2, candidate 1.
		let ballots = vec![
			ballot(1, &[10, ABSTAIN_OFFSET]),
			ballot(2, &[10, ABSTAIN_OFFSET]),
			ballot(3, &[10, ABSTAIN_OFFSET]),
			ballot(4, &[ABSTAIN_OFFSET, 10]),
			ballot(5, &[ABSTAIN_OFFSET, 10]),
		];
		let out = bloc_star_tally(&[1, 2], &ballots, 1).unwrap();
		assert_eq!(out.winners, vec![1]);

		// Flip the sizes: 2 vs 3 -> candidate 2 wins the runoff despite
		// losing the score stage.
		let ballots = vec![
			ballot(1, &[10, ABSTAIN_OFFSET]),
			ballot(2, &[10, ABSTAIN_OFFSET]),
			ballot(3, &[ABSTAIN_OFFSET, 10]),
			ballot(4, &[ABSTAIN_OFFSET, 10]),
			ballot(5, &[ABSTAIN_OFFSET, 10]),
		];
		let out = bloc_star_tally(&[1, 2], &ballots, 1).unwrap();
		assert_eq!(out.winners, vec![2]);
	}

	#[test]
	fn bloc_star_equal_scores_support_neither() {
		// 2 voters rate both candidates equally: no runoff support.
		// 1 voter prefers candidate 2.
		let ballots = vec![
			ballot(1, &[10, 10]),
			ballot(2, &[10, 10]),
			ballot(3, &[5, 10]),
		];
		// Score: candidate 1 = +20+0 = +20; candidate 2 = +20+5 = +25 -> 2 leads.
		// Runoff: only voter 3 supports 2; voters 1-2 support neither.
		// 0 vs 1 -> candidate 2 wins.
		let out = bloc_star_tally(&[1, 2], &ballots, 1).unwrap();
		assert_eq!(out.winners, vec![2]);
	}

	#[test]
	fn bloc_star_seats_one_by_one() {
		// 3 candidates, 2 winners. Candidate 1 is a strong faction favorite,
		// candidates 2/3 split the rest.
		let ballots = vec![
			ballot(1, &[10, 0, 0]),
			ballot(2, &[10, 0, 0]),
			ballot(3, &[10, 0, 0]),
			ballot(4, &[0, 10, 0]),
			ballot(5, &[0, 10, 6]),
			ballot(6, &[0, 6, 10]),
		];
		// Score: 1 = +15; 2 = +10+1 = +11... precisely:
		// candidate 2: voters 4 (+5), 5 (+5), 6 (+1) = +11.
		// candidate 3: voters 5 (+1), 6 (+5) = +6.
		// Round 1: finalists 1, 2. Runoff: voters 1-3 prefer 1 (+5 > -5);
		// voters 4-6 prefer 2. 3 vs 3 -> tie -> score-stage leader (1) wins.
		let out = bloc_star_tally(&[1, 2, 3], &ballots, 2).unwrap();
		assert_eq!(out.winners[0], 1);
		// Round 2: remaining {2, 3}; runoff: voters 1-3 rate both -5 (equal,
		// no support); voter 4 prefers 2 (+5 > -5); voter 5 prefers 2
		// (+5 > +1); voter 6 prefers 3 (+5 > +1). 2 vs 1 -> candidate 2.
		assert_eq!(out.winners[1], 2);
	}

	#[test]
	fn bloc_star_two_winners_fills_last_seat_without_runoff() {
		let ballots = vec![ballot(1, &[10, 0]), ballot(2, &[0, 10])];
		let out = bloc_star_tally(&[1, 2], &ballots, 2).unwrap();
		assert_eq!(out.winners.len(), 2);
		assert_eq!(out.winners, vec![1, 2]);
	}

	// ------------------------------------------------------------------
	// STAR-PR (Allocated Score)
	// ------------------------------------------------------------------

	#[test]
	fn star_pr_two_factions_two_seats() {
		// 100 voters for faction A, 100 for faction B, plus an unloved
		// candidate 3; 2 seats. Round 1: H = 200/2 = 100; candidates 1 and 2
		// both reach exactly the quota -> tie -> smaller id (1) wins and its
		// ballots are spent fully. Round 2: H = 100/1 = 100; candidate 2
		// allocated 100, candidate 3 allocated 0 -> candidate 2 wins.
		let mut ballots = Vec::new();
		for v in 0..100u8 {
			ballots.push(ballot(v, &[10, 0, 0]));
		}
		for v in 100..200u8 {
			ballots.push(ballot(v, &[0, 10, 0]));
		}
		let out = star_pr_tally(&[1, 2, 3], &ballots, 2).unwrap();
		assert_eq!(out.winners, vec![1, 2]);
	}

	#[test]
	fn star_pr_majority_cannot_sweep_two_seats() {
		// 60 voters: candidate 1 = +5, candidate 2 = +1, candidate 3 = 0.
		// 40 voters: candidate 3 = +5, candidate 2 = +1, candidate 1 = 0.
		let mut ballots = Vec::new();
		for v in 0..60u8 {
			ballots.push(ballot(v, &[10, 6, 0]));
		}
		for v in 60..100u8 {
			ballots.push(ballot(v, &[0, 6, 10]));
		}
		let out = star_pr_tally(&[1, 2, 3], &ballots, 2).unwrap();
		// Round 1: H = 100/2 = 50.
		// candidate 1: positive weight 60 -> allocated 50 (cap).
		// candidate 3: positive weight 40 -> allocated 40.
		// candidate 2: positive weight 100 -> allocated 50 (cap).
		// Tie between 1 and 2 at 50 -> smaller id: candidate 1 wins.
		// Spent: 50 of the 60 first-faction ballots (all scored +5).
		// Round 2: remaining weight = 10 (unspent faction-1 ballots) + 100
		// (faction-2 ballots, untouched — candidate 2 did not win) = 110;
		// H = 110/1 = 110.
		// candidate 2: positive weight 100 -> allocated 100.
		// candidate 3: positive weight 40 -> allocated 40.
		// candidate 2 wins.
		assert_eq!(out.winners, vec![1, 2]);
	}

	#[test]
	fn star_pr_zero_scores_never_allocated() {
		// Candidate 2 only receives 0-scores -> allocated support is always 0.
		let mut ballots = Vec::new();
		for v in 0..10u8 {
			ballots.push(ballot(v, &[10, 5, 0]));
		}
		let out = star_pr_tally(&[1, 2, 3], &ballots, 2).unwrap();
		// Round 1: H = 10/2 = 5; candidate 1 allocated 5, candidates 2 and 3
		// allocated 0 -> candidate 1 wins (5 > 0); 5 of its ballots spent.
		// Round 2: remaining weight 5; H = 5; candidates 2 and 3 allocated 0;
		// tie -> smaller id: candidate 2.
		assert_eq!(out.winners, vec![1, 2]);
	}

	#[test]
	fn star_pr_abstentions_not_allocated() {
		// Abstains must not act like positive scores.
		let ballots = vec![
			ballot(1, &[10, ABSTAIN_OFFSET]),
			ballot(2, &[10, ABSTAIN_OFFSET]),
			ballot(3, &[10, ABSTAIN_OFFSET]),
			ballot(4, &[ABSTAIN_OFFSET, 10]),
		];
		let out = star_pr_tally(&[1, 2], &ballots, 1).unwrap();
		// H = 4; candidate 1 allocated 3, candidate 2 allocated 1 -> 1 wins.
		assert_eq!(out.winners, vec![1]);
	}

	#[test]
	fn star_pr_minority_gets_its_seat() {
		// 80 voters of faction A (candidates 1..4), 20 voters of faction B
		// (candidates 5..8): with 4 seats, STAR-PR gives B one seat; bloc
		// STAR would sweep all four for A.
		let mut ballots = Vec::new();
		for v in 0..80u8 {
			ballots.push(ballot(v, &[10, 10, 10, 10, 0, 0, 0, 0]));
		}
		for v in 80..100u8 {
			ballots.push(ballot(v, &[0, 0, 0, 0, 10, 10, 10, 10]));
		}
		let out = star_pr_tally(&[1, 2, 3, 4, 5, 6, 7, 8], &ballots, 4).unwrap();
		// Round 1: H = 100/4 = 25.
		// candidates 1-4: positive weight 80 -> 25 each (cap).
		// candidates 5-8: positive weight 20 -> 20 each.
		// Tie 25 among 1..4 -> candidate 1 wins; 25 ballots spent.
		// Round 2: total = 75; H = 75/3 = 25.
		// candidates 2-4: 55 -> 25 (cap); candidates 5-8: 20.
		// candidate 2 wins; 25 spent.
		// Round 3: total = 50; H = 50/2 = 25.
		// candidates 3, 4: 30 -> 25 (cap); candidates 5-8: 20.
		// candidate 3 wins; 25 spent.
		// Round 4: total = 25; H = 25/1 = 25.
		// candidate 4: 5 -> 5; candidates 5-8: 20 -> 20.
		// candidate 5 wins.
		assert_eq!(out.winners, vec![1, 2, 3, 5]);
	}
}
