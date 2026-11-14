//! The canonical tally-result digest.
//!
//! This must stay byte-for-byte identical to
//! `pallet-star-voting/src/lib.rs::compute_result_hash`:
//!
//! ```text
//! result_hash = blake2b_256(
//!     scale_encode((election_id: u32, winner_count: u16, winners: &[u32], totals: &[(u32, u32)]))
//! )
//! ```
//!
//! The pallet recomputes this hash on-chain from whatever `submit_tally`
//! receives, so any tampering with the stored winners/totals (by the tally
//! service or anyone else) is detectable here by re-deriving the hash from
//! the ballots and comparing.

use blake2::digest::{Update, VariableOutput};
use blake2::Blake2bVar;
use parity_scale_codec::Encode;

/// BLAKE2b-256, matching Substrate's `sp_io::hashing::blake2_256`.
pub fn blake2b_256(data: &[u8]) -> [u8; 32] {
	let mut hasher = Blake2bVar::new(32).expect("32 is a valid BLAKE2b output size");
	hasher.update(data);
	let out = hasher.finalize_boxed();
	let mut hash = [0u8; 32];
	hash.copy_from_slice(&out);
	hash
}

/// Compute the canonical digest of a tally result (see module docs).
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

#[cfg(test)]
mod tests {
	use super::*;

	/// Cross-check against the pallet. The expected value was produced by the
	/// `digest_cross_check` test in `pallets/star-voting/src/tests.rs`, which
	/// calls the pallet's own `compute_result_hash` (via
	/// `sp_io::hashing::blake2_256`). Regenerate both whenever the digest
	/// payload changes.
	#[test]
	fn matches_on_chain_digest() {
		let expected: [u8; 32] = [
			0xeb, 0xe9, 0x50, 0xd4, 0x2a, 0x66, 0x1b, 0x76, 0xca, 0x27, 0x25, 0xb9, 0x92, 0xb5,
			0xf7, 0x22, 0x29, 0xaa, 0xc9, 0x90, 0x04, 0xea, 0x5e, 0xa9, 0x5c, 0xbf, 0xa7, 0xb0,
			0xab, 0x9b, 0xf0, 0xff,
		];
		let actual = compute_result_hash(0, 5, &[5, 4, 3, 2, 1], &[(1, 0), (2, 0), (3, 0), (4, 0), (5, 0)]);
		assert_eq!(actual, expected);
	}
}
