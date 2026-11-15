//! Chain access glue for the tally client, built on subxt's *static* codegen.
//!
//! The typed interface is generated at compile time from
//! `artifacts/metadata.scale` (see README for how to regenerate it when the
//! runtime changes), so every call and storage read is compiler-checked
//! against the runtime.

use anyhow::{anyhow, bail, Context, Result};
use subxt::{OnlineClient, PolkadotConfig};
use subxt_signer::sr25519::Keypair;

use crate::tally::Ballot;

#[subxt::subxt(
	runtime_metadata_path = "artifacts/metadata.scale",
	derive_for_all_types = "Clone, Debug, PartialEq, Eq"
)]
pub mod api {}

/// An election as stored on-chain.
#[derive(Clone, Debug)]
pub struct ElectionInfo {
	pub winner_count: u16,
	pub candidates: Vec<u32>,
	pub voting_end: u32,
	pub phase: String,
}

/// The stored tally result of an election.
#[derive(Clone, Debug)]
pub struct StoredTally {
	pub result_hash: [u8; 32],
	/// The tally method this result was computed with.
	pub method: crate::tally::TallyMethod,
	pub winners: Vec<u32>,
	pub totals: Vec<(u32, u32)>,
	pub submitted_by: Option<[u8; 32]>,
	pub submitted_at: u32,
}

/// A connected node client.
pub struct Chain {
	pub(crate) client: OnlineClient<PolkadotConfig>,
}

impl Chain {
	/// Connect to a node's RPC endpoint.
	pub async fn connect(url: &str) -> Result<Self> {
		let client = OnlineClient::<PolkadotConfig>::from_url(url)
			.await
			.with_context(|| format!("connecting to {url}"))?;
		Ok(Self { client })
	}

	/// Current block number.
	pub async fn block_number(&self) -> Result<u32> {
		Ok(self.client.blocks().at_latest().await?.header().number)
	}

	/// Fetch an election by id.
	pub async fn election(&self, election_id: u32) -> Result<Option<ElectionInfo>> {
		let Some(election) = self
			.client
			.storage()
			.at_latest()
			.await?
			.fetch(&api::storage().star_voting().elections(election_id))
			.await?
		else {
			return Ok(None);
		};
		Ok(Some(ElectionInfo {
			winner_count: election.winner_count,
			candidates: election.candidates.0,
			voting_end: election.voting_end,
			phase: phase_string(&election.phase),
		}))
	}

	/// The stored tally result of an election, if one was submitted.
	pub async fn stored_tally(&self, election_id: u32) -> Result<Option<StoredTally>> {
		let Some(tally) = self
			.client
			.storage()
			.at_latest()
			.await?
			.fetch(&api::storage().star_voting().tally(election_id))
			.await?
		else {
			return Ok(None);
		};
		Ok(Some(StoredTally {
			result_hash: tally.result_hash,
			method: method_of(&tally.method),
			winners: tally.winners.0,
			totals: tally.totals.0,
			submitted_by: tally.submitted_by.map(|a| a.0),
			submitted_at: tally.submitted_at,
		}))
	}

	/// All ballots of an election, read by iterating the `Ballots` map and
	/// filtering on the election id encoded in each key.
	pub async fn ballots(&self, election_id: u32) -> Result<Vec<Ballot>> {
		let mut ballots = Vec::new();
		let mut iter = self
			.client
			.storage()
			.at_latest()
			.await?
			.iter(api::storage().star_voting().ballots_iter())
			.await?;
		while let Some(kv) = iter.next().await {
			let kv = kv?;
			let key_election_id = key_first_u32(&kv.key_bytes)
				.context("decoding Ballots map key")?;
			if key_election_id != election_id {
				continue;
			}
			// Key layout after the 32-byte storage root (single
			// `Blake2_128Concat` hasher over the whole tuple key):
			// blake2_128 ++ u32 election id ++ [u8; 32] account id, i.e. the
			// voter account starts at byte 32 + 16 + 4 = 52.
			let voter: [u8; 32] = kv
				.key_bytes
				.get(52..84)
				.and_then(|b| b.try_into().ok())
				.ok_or_else(|| anyhow!("Ballots key too short: {}", kv.key_bytes.len()))?;
			ballots.push(Ballot { voter, scores: kv.value.0 });
		}
		Ok(ballots)
	}

	/// Submit `StarVoting::submit_tally` (score method), wrapped in
	/// `Sudo::sudo` unless `use_sudo` is false (for runtimes whose
	/// TallyOrigin accepts signed accounts).
	pub async fn submit_tally(
		&self,
		signer: &Keypair,
		use_sudo: bool,
		election_id: u32,
		winners: &[u32],
		totals: &[(u32, u32)],
	) -> Result<()> {
		let inner = api::runtime_types::pallet_star_voting::pallet::Call::submit_tally {
			election_id,
			winners: winners.to_vec(),
			totals: totals.to_vec(),
		};
		let call = api::Call::StarVoting(inner);

		// The two payload types differ, so dispatch in separate branches.
		if use_sudo {
			self.submit_and_wait(&api::tx().sudo().sudo(call), signer).await
		} else {
			// Same call dispatched from a signed origin, for runtimes whose
			// TallyOrigin accepts accounts.
			self.submit_and_wait(
				&api::tx().star_voting().submit_tally(election_id, winners.to_vec(), totals.to_vec()),
				signer,
			)
			.await
		}
	}

	/// Submit `StarVoting::submit_tally_star` (Bloc STAR / STAR-PR), wrapped
	/// in `Sudo::sudo` unless `use_sudo` is false.
	pub async fn submit_tally_star(
		&self,
		signer: &Keypair,
		use_sudo: bool,
		method: crate::tally::TallyMethod,
		election_id: u32,
		winners: &[u32],
		totals: &[(u32, u32)],
	) -> Result<()> {
		let method = star_method_of(method);
		let inner = api::runtime_types::pallet_star_voting::pallet::Call::submit_tally_star {
			method: method.clone(),
			election_id,
			winners: winners.to_vec(),
			totals: totals.to_vec(),
		};
		let call = api::Call::StarVoting(inner);

		if use_sudo {
			self.submit_and_wait(&api::tx().sudo().sudo(call), signer).await
		} else {
			self.submit_and_wait(
				&api::tx().star_voting().submit_tally_star(
					method,
					election_id,
					winners.to_vec(),
					totals.to_vec(),
				),
				signer,
			)
			.await
		}
	}

	/// Sign, submit, and wait for finalization of any payload.
	async fn submit_and_wait<P: subxt::tx::Payload>(
		&self,
		payload: &P,
		signer: &Keypair,
	) -> Result<()> {
		let events = self
			.client
			.tx()
			.sign_and_submit_then_watch_default(payload, signer)
			.await?
			.wait_for_finalized_success()
			.await?;
		println!(
			"submitted (extrinsic index {}); tx hash {:?}",
			events.extrinsic_index(),
			events.extrinsic_hash()
		);
		Ok(())
	}

	/// Submit `StarVoting::create_election` (signed origin).
	pub async fn create_election(
		&self,
		signer: &Keypair,
		winner_count: u16,
		candidates: &[u32],
		voting_period: u32,
	) -> Result<()> {
		let events = self
			.client
			.tx()
			.sign_and_submit_then_watch_default(
				&api::tx().star_voting().create_election(winner_count, candidates.to_vec(), voting_period),
				signer,
			)
			.await?
			.wait_for_finalized_success()
			.await?;
		println!(
			"election created (extrinsic index {}); tx hash {:?}",
			events.extrinsic_index(),
			events.extrinsic_hash()
		);
		Ok(())
	}

	/// Submit `StarVoting::vote` (signed origin; voting is feeless).
	pub async fn vote(&self, signer: &Keypair, election_id: u32, scores: &[u8]) -> Result<()> {
		let events = self
			.client
			.tx()
			.sign_and_submit_then_watch_default(
				&api::tx().star_voting().vote(election_id, scores.to_vec()),
				signer,
			)
			.await?
			.wait_for_finalized_success()
			.await?;
		println!(
			"ballot cast (extrinsic index {}); tx hash {:?}",
			events.extrinsic_index(),
			events.extrinsic_hash()
		);
		Ok(())
	}
}

/// The `Phase` enum of the election, as a display string.
fn phase_string(phase: &api::runtime_types::pallet_star_voting::pallet::Phase) -> String {
	use api::runtime_types::pallet_star_voting::pallet::Phase;
	match phase {
		Phase::Voting => "Voting".into(),
		Phase::Tallying => "Tallying".into(),
		Phase::Finalized => "Finalized".into(),
	}
}

/// Map the codegen `TallyMethod` to the client enum. Variant order must
/// match `tally::TallyMethod` (which must match the pallet).
fn method_of(
	method: &api::runtime_types::pallet_star_voting::pallet::TallyMethod,
) -> crate::tally::TallyMethod {
	use api::runtime_types::pallet_star_voting::pallet::TallyMethod;
	match method {
		TallyMethod::Score => crate::tally::TallyMethod::Score,
		TallyMethod::BlocStar => crate::tally::TallyMethod::BlocStar,
		TallyMethod::StarPr => crate::tally::TallyMethod::StarPr,
	}
}

/// Map the client enum to the codegen `TallyMethod` for call args.
fn star_method_of(method: crate::tally::TallyMethod)
-> api::runtime_types::pallet_star_voting::pallet::TallyMethod {
	use api::runtime_types::pallet_star_voting::pallet::TallyMethod;
	match method {
		crate::tally::TallyMethod::Score => TallyMethod::Score,
		crate::tally::TallyMethod::BlocStar => TallyMethod::BlocStar,
		crate::tally::TallyMethod::StarPr => TallyMethod::StarPr,
	}
}

/// Extract the election id from an iterated map key whose first key
/// component is an `ElectionId`.
///
/// Iterated `key_bytes` are the full storage address:
/// `twox128(pallet ++ entry) ++ blake2_128(key) ++ key ++ ...`, so after the
/// 32-byte storage root the `Blake2_128Concat` key is laid out as
/// `blake2_128(component) ++ component ++ ...`, i.e. the `ElectionId` starts
/// at byte 48.
pub fn key_first_u32(key_bytes: &[u8]) -> Result<u32> {
	let key = if key_bytes.len() > 20 { &key_bytes[32..] } else { key_bytes };
	if key.len() < 20 {
		bail!("key too short for blake2_128concat(u32): {}", key_bytes.len());
	}
	Ok(u32::from_le_bytes(key[16..20].try_into().expect("4 bytes")))
}
