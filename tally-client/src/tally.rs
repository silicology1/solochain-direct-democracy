//! The multi-winner STAR tally algorithm, mirroring the on-chain ballot
//! format and the ranking used by `multiwinner-star-voting` (top-W by real
//! signed total score, ties broken by smaller candidate id).

use anyhow::{bail, Result};

/// Score offset meaning "deliberately abstained" for one candidate.
pub const ABSTAIN_OFFSET: u8 = 255;
/// Maximum valid score offset (raw score + 5).
pub const MAX_SCORE_OFFSET: u8 = 10;

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
	/// Winning candidate ids, in ranked order.
	pub winners: Vec<u32>,
	/// Offset-based total score per candidate, covering every candidate
	/// exactly once (this is exactly what the pallet's `submit_tally` takes).
	pub totals: Vec<(u32, u32)>,
}

/// Tally `ballots` for an election with `candidates` and `winner_count`.
///
/// Ranking rule: candidates are ranked by their *real* signed total score
/// `real_total[c] = offset_total[c] - 5 * count[c]`, which is what voters
/// expressed (each vote contributes `-5..+5`). Ties are broken by the smaller
/// candidate id. The top `winner_count` candidates win.
///
/// Abstentions (`255`) are excluded from both the total and the count, so a
/// candidate a voter deliberately abstained on neither gains nor loses score
/// and the score scale stays comparable between voters.
pub fn tally(candidates: &[u32], ballots: &[Ballot], winner_count: usize) -> Result<TallyOutput> {
	if winner_count == 0 {
		bail!("winner_count must be at least 1");
	}
	if winner_count > candidates.len() {
		bail!(
			"winner_count ({winner_count}) exceeds number of candidates ({})",
			candidates.len()
		);
	}

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

	let totals: Vec<(u32, u32)> = candidates
		.iter()
		.copied()
		.zip(offset_totals.iter())
		.map(|(c, &t)| (c, t as u32))
		.collect();

	let mut ranked: Vec<(u32, i64)> = candidates
		.iter()
		.copied()
		.zip(offset_totals.iter().zip(counts.iter()))
		.map(|(c, (&offset_total, &count))| (c, offset_total as i64 - 5 * count as i64))
		.collect();

	// Highest real total first; ties go to the smaller candidate id.
	ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

	let winners: Vec<u32> = ranked.into_iter().take(winner_count).map(|(c, _)| c).collect();

	Ok(TallyOutput { winners, totals })
}

#[cfg(test)]
mod tests {
	use super::*;

	fn ballot(voter: u8, scores: &[u8]) -> Ballot {
		let mut voter_id = [0u8; 32];
		voter_id[0] = voter;
		Ballot { voter: voter_id, scores: scores.to_vec() }
	}

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
}
