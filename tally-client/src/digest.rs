//! The canonical tally-result digests.
//!
//! These must stay byte-for-byte identical to the pallet definitions in
//! `pallet-star-voting/src/lib.rs`:
//!
//! ```text
//! score:    result_hash = blake2b_256(
//!               scale_encode((election_id: u32, winner_count: u16,
//!                             winners: &[u32], totals: &[(u32, u32)])))
//! star:     result_hash = blake2b_256(
//!               scale_encode((tag: &[u8], method: TallyMethod,
//!                             election_id: u32, winner_count: u16,
//!                             winners: &[u32], totals: &[(u32, u32)])))
//! ```
//!
//! The STAR digest covers Bloc STAR and STAR-PR results; its domain tag
//! (`STAR_DIGEST_TAG`) and the `TallyMethod` variant index keep a STAR result
//! cryptographically distinguishable from a score result.
//!
//! The pallet recomputes these hashes on-chain from whatever `submit_tally` /
//! `submit_tally_star` receives, so any tampering with the stored
//! winners/totals (by the tally service or anyone else) is detectable here by
//! re-deriving the hash from the ballots and comparing.

use blake2::digest::{Update, VariableOutput};
use blake2::Blake2bVar;
use parity_scale_codec::Encode;

use crate::tally::TallyMethod;

/// BLAKE2b-256, matching Substrate's `sp_io::hashing::blake2_256`.
pub fn blake2b_256(data: &[u8]) -> [u8; 32] {
	let mut hasher = Blake2bVar::new(32).expect("32 is a valid BLAKE2b output size");
	hasher.update(data);
	let out = hasher.finalize_boxed();
	let mut hash = [0u8; 32];
	hash.copy_from_slice(&out);
	hash
}

/// Compute the canonical digest of a score-method tally result.
pub fn compute_result_hash(
	election_id: u32,
	winner_count: u16,
	winners: &[u32],
	totals: &[(u32, u32)],
) -> [u8; 32] {
	let mut buf = Vec::new();
	election_id.encode_to(&mut buf);
	winner_count.encode_to(&mut buf);
	winners.encode_to(&mut buf);
	totals.encode_to(&mut buf);
	blake2b_256(&buf)
}

/// Domain tag separating the STAR-method digest (Bloc STAR / STAR-PR) from
/// the score digest. Must match `STAR_DIGEST_TAG` in the pallet.
pub const STAR_DIGEST_TAG: &[u8] = b"pallet-star-voting/star/1";

/// Compute the canonical digest of a Bloc STAR / STAR-PR tally result.
pub fn compute_star_result_hash(
	method: TallyMethod,
	election_id: u32,
	winner_count: u16,
	winners: &[u32],
	totals: &[(u32, u32)],
) -> [u8; 32] {
	let mut buf = Vec::new();
	STAR_DIGEST_TAG.encode_to(&mut buf);
	method.encode_to(&mut buf);
	election_id.encode_to(&mut buf);
	winner_count.encode_to(&mut buf);
	winners.encode_to(&mut buf);
	totals.encode_to(&mut buf);
	blake2b_256(&buf)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Cross-check against the pallet. The expected values were produced by
	/// the `digest_cross_check` / `star_digest_cross_check` tests in
	/// `pallets/star-voting/src/tests.rs`, which call the pallet's own
	/// digest functions (via `sp_io::hashing::blake2_256`). Regenerate both
	/// whenever a digest payload changes.
	fn expected(kind: &str) -> [u8; 32] {
		let hex = match kind {
			"score" => "ebe950d42a661b76ca2725b992b5f72229aac99004ea5ea95cbfa7b0ab9bf0ff",
			"bloc-star" => "4d6fef530d8d4c49ea7edc6a72189af280de49dbfd643afc14c6a77bb22ae4a7",
			"star-pr" => "4c91eccf2850b8d0030bc6ea7d7083538784727b72e00417fcaa8d4b35c94cba",
			_ => unreachable!(),
		};
		let mut out = [0u8; 32];
		hex::decode_to_slice(hex, &mut out).expect("valid hex");
		out
	}

	#[test]
	fn score_digest_matches_on_chain() {
		assert_eq!(
			compute_result_hash(0, 5, &[5, 4, 3, 2, 1], &[(1, 0), (2, 0), (3, 0), (4, 0), (5, 0)]),
			expected("score")
		);
	}

	#[test]
	fn star_digests_match_on_chain() {
		let payload = (&[5u32, 4, 3, 2, 1], &[(1u32, 0u32), (2, 0), (3, 0), (4, 0), (5, 0)]);
		assert_eq!(
			compute_star_result_hash(TallyMethod::BlocStar, 0, 5, payload.0, payload.1),
			expected("bloc-star")
		);
		assert_eq!(
			compute_star_result_hash(TallyMethod::StarPr, 0, 5, payload.0, payload.1),
			expected("star-pr")
		);
	}

	#[test]
	fn star_digests_differ_from_score_digest() {
		let payload = (&[5u32, 4, 3, 2, 1], &[(1u32, 0u32), (2, 0), (3, 0), (4, 0), (5, 0)]);
		let score = compute_result_hash(0, 5, payload.0, payload.1);
		assert_ne!(compute_star_result_hash(TallyMethod::BlocStar, 0, 5, payload.0, payload.1), score);
		assert_ne!(compute_star_result_hash(TallyMethod::StarPr, 0, 5, payload.0, payload.1), score);
		assert_ne!(
			compute_star_result_hash(TallyMethod::BlocStar, 0, 5, payload.0, payload.1),
			compute_star_result_hash(TallyMethod::StarPr, 0, 5, payload.0, payload.1)
		);
	}
}
