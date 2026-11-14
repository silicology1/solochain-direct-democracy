//! Off-chain tally service for `pallet-star-voting`.
//!
//! Reads ballots straight from chain state, runs the multi-winner STAR tally
//! locally, and submits the result (winners + score totals) on-chain via
//! `submit_tally`. The pallet recomputes the result hash from the submitted
//! payload, so the stored hash always binds the result; run `verify` to
//! independently recompute everything and confirm the stored tally matches
//! the ballots.

mod chain;
mod digest;
mod tally;

use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use subxt_signer::{sr25519::Keypair, SecretUri};

use crate::chain::Chain;
use crate::digest::compute_result_hash;
use crate::tally::{tally as star_tally, ABSTAIN_OFFSET};

#[derive(Parser)]
#[command(
	name = "star-voting-tally",
	about = "Off-chain multi-winner STAR tally service for pallet-star-voting"
)]
struct Cli {
	/// Node RPC endpoint.
	#[arg(long, global = true, default_value = "ws://127.0.0.1:9944")]
	url: String,

	/// Signer secret URI (e.g. "//Alice"). Defaults to //Alice.
	#[arg(long, global = true, default_value = "//Alice")]
	seed: String,

	#[command(subcommand)]
	command: Command,
}

#[derive(Subcommand)]
enum Command {
	/// Create a new election (useful for demos/testing).
	Create {
		/// Number of winners (5..=500).
		#[arg(long, default_value_t = 5)]
		winners: u16,
		/// Comma-separated candidate ids.
		#[arg(long, value_delimiter = ',')]
		candidates: Vec<u32>,
		/// Voting period in blocks.
		#[arg(long, default_value_t = 60)]
		voting_period: u32,
	},
	/// Cast a ballot (feeless).
	Vote {
		/// Election id.
		id: u32,
		/// One score per candidate, comma-separated: raw scores in -5..=5,
		/// or `a`/`A` to abstain (e.g. "5,-2,0,a,3").
		#[arg(value_delimiter = ',')]
		scores: Vec<String>,
	},
	/// Run the off-chain tally for an election and submit the result.
	Tally {
		/// Election id.
		id: u32,
		/// Submit the bare `submit_tally` call instead of wrapping it in
		/// `Sudo::sudo` (for runtimes whose TallyOrigin is a signed account).
		#[arg(long, default_value_t = false)]
		no_sudo: bool,
	},
	/// Recompute the tally from chain state and compare it with the stored
	/// result.
	Verify {
		/// Election id.
		id: u32,
	},
	/// Poll the chain and automatically tally elections whose voting period
	/// has ended.
	Watch {
		/// Poll interval in seconds.
		#[arg(long, default_value_t = 6)]
		interval: u64,
		/// Submit the bare `submit_tally` call instead of wrapping it in
		/// `Sudo::sudo`.
		#[arg(long, default_value_t = false)]
		no_sudo: bool,
	},
}

/// Optional wrapper for `seed` parsing + `--seed` override from env.
fn parse_scores(inputs: &[String], n_candidates: usize) -> Result<Vec<u8>> {
	if inputs.len() != n_candidates {
		bail!(
			"got {} scores but the election has {} candidates",
			inputs.len(),
			n_candidates
		);
	}
	inputs
		.iter()
		.map(|s| {
			if s.eq_ignore_ascii_case("a") {
				return Ok(ABSTAIN_OFFSET);
			}
			let score: i32 = s.parse().context("score must be an integer or 'a'")?;
			if !(-5..=5).contains(&score) {
				bail!("score {score} out of range -5..=5");
			}
			Ok((score + 5) as u8)
		})
		.collect()
}

fn signer_from_uri(seed: &str) -> Result<Keypair> {
	let uri: SecretUri = seed.parse().context("parsing secret URI")?;
	Keypair::from_uri(&uri).context("deriving keypair")
}

/// Compute the tally for `election_id` and print it.
async fn compute_and_print(chain: &Chain, election_id: u32) -> Result<(u16, Vec<u32>, Vec<(u32, u32)>, [u8; 32])> {
	let election = chain
		.election(election_id)
		.await?
		.with_context(|| format!("election {election_id} does not exist"))?;
	println!(
		"election {election_id}: {} winners, {} candidates, phase {} (ends at block {})",
		election.winner_count,
		election.candidates.len(),
		election.phase,
		election.voting_end,
	);

	println!("reading ballots from chain...");
	let ballots = chain.ballots(election_id).await?;
	println!("got {} ballots", ballots.len());

	let out = star_tally(&election.candidates, &ballots, election.winner_count as usize)?;
	for (i, w) in out.winners.iter().enumerate() {
		println!("  {}. candidate {w}", i + 1);
	}

	let result_hash = compute_result_hash(election_id, election.winner_count, &out.winners, &out.totals);
	println!("result hash: 0x{}", hex::encode(result_hash));
	Ok((election.winner_count, out.winners, out.totals, result_hash))
}

async fn cmd_tally(chain: &Chain, seed: &str, id: u32, no_sudo: bool) -> Result<()> {
	let signer = signer_from_uri(seed)?;
	let (winner_count, winners, totals, result_hash) = compute_and_print(chain, id).await?;

	if let Some(election) = chain.election(id).await? {
		let block = chain.block_number().await?;
		if election.phase != "Voting" || block < election.voting_end {
			bail!(
				"voting has not ended yet (phase {}, block {block}, ends at {})",
				election.phase,
				election.voting_end
			);
		}
	}
	if (winners.len() as u16) != winner_count {
		bail!("tally produced {} winners but the election wants {winner_count}", winners.len());
	}

	println!("submitting tally result{}...", if no_sudo { "" } else { " via Sudo" });
	chain.submit_tally(&signer, !no_sudo, id, &winners, &totals).await?;
	println!("done. result hash: 0x{}", hex::encode(result_hash));
	Ok(())
}

async fn cmd_verify(chain: &Chain, id: u32) -> Result<()> {
	let stored = chain
		.stored_tally(id)
		.await?
		.with_context(|| format!("election {id} has no stored tally"))?;

	println!("stored winners: {:?}", stored.winners);
	println!("stored result hash: 0x{}", hex::encode(stored.result_hash));

	let election = chain
		.election(id)
		.await?
		.with_context(|| format!("election {id} does not exist"))?;
	let ballots = chain.ballots(id).await?;
	println!("recomputing tally from {} ballots...", ballots.len());

	let out = star_tally(&election.candidates, &ballots, election.winner_count as usize)?;
	let recomputed_hash =
		compute_result_hash(id, election.winner_count, &out.winners, &out.totals);

	if stored.winners != out.winners {
		bail!(
			"MISMATCH: stored winners {:?} != recomputed {:?}",
			stored.winners,
			out.winners
		);
	}
	if stored.totals != out.totals {
		bail!(
			"MISMATCH: stored totals {:?} != recomputed {:?}",
			stored.totals,
			out.totals
		);
	}
	if stored.result_hash != recomputed_hash {
		bail!(
			"MISMATCH: stored hash 0x{} != recomputed 0x{}",
			hex::encode(stored.result_hash),
			hex::encode(recomputed_hash)
		);
	}

	println!("OK: stored tally matches the ballots and the result hash.");
	if let Some(submitter) = stored.submitted_by {
		println!("  submitted by: 0x{} at block {}", hex::encode(submitter), stored.submitted_at);
	}
	Ok(())
}

async fn cmd_watch(chain: &Chain, seed: &str, interval: u64, no_sudo: bool) -> Result<()> {
	let signer = signer_from_uri(seed)?;
	println!("watching for elections to tally (every {interval}s)...");
	loop {
		if let Err(e) = watch_once(chain, &signer, no_sudo).await {
			eprintln!("watch iteration failed: {e:#}");
		}
		tokio::time::sleep(Duration::from_secs(interval)).await;
	}
}

async fn watch_once(chain: &Chain, signer: &Keypair, no_sudo: bool) -> Result<()> {
	// Iterate all elections and tally any whose voting phase has ended and
	// which have not been finalized yet.
	let mut iter = chain
		.client
		.storage()
		.at_latest()
		.await?
		.iter(chain::api::storage().star_voting().elections_iter())
		.await?;

	while let Some(kv) = iter.next().await {
		let kv = kv?;
		let election_id = chain::key_first_u32(&kv.key_bytes)
			.with_context(|| format!("parsing election id from key 0x{}", hex::encode(&kv.key_bytes)))?;
		if kv.value.phase != chain::api::runtime_types::pallet_star_voting::pallet::Phase::Voting {
			continue;
		}
		let block = chain.block_number().await?;
		if block < kv.value.voting_end {
			continue;
		}

		println!("election {election_id} voting ended; tallying...");

		match tally_and_submit(chain, signer, election_id, no_sudo).await {
			Ok(()) => println!("election {election_id} finalized"),
			Err(e) => eprintln!("tally of election {election_id} failed: {e:#}"),
		}
	}
	Ok(())
}

async fn tally_and_submit(chain: &Chain, signer: &Keypair, election_id: u32, no_sudo: bool) -> Result<()> {
	let election = chain
		.election(election_id)
		.await?
		.with_context(|| format!("election {election_id} does not exist"))?;
	if chain.stored_tally(election_id).await?.is_some() {
		bail!("already finalized");
	}
	let ballots = chain.ballots(election_id).await?;
	let out = star_tally(&election.candidates, &ballots, election.winner_count as usize)?;
	let result_hash =
		compute_result_hash(election_id, election.winner_count, &out.winners, &out.totals);
	chain.submit_tally(signer, !no_sudo, election_id, &out.winners, &out.totals).await?;
	println!("election {election_id}: result hash 0x{}", hex::encode(result_hash));
	Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
	let cli = Cli::parse();
	let chain = Chain::connect(&cli.url).await?;

	match cli.command {
		Command::Create { winners, candidates, voting_period } => {
			let signer = signer_from_uri(&cli.seed)?;
			chain.create_election(&signer, winners, &candidates, voting_period).await?;
		},
		Command::Vote { id, scores } => {
			let signer = signer_from_uri(&cli.seed)?;
			let election = chain
				.election(id)
				.await?
				.with_context(|| format!("election {id} does not exist"))?;
			let scores = parse_scores(&scores, election.candidates.len())?;
			chain.vote(&signer, id, &scores).await?;
		},
		Command::Tally { id, no_sudo } => cmd_tally(&chain, &cli.seed, id, no_sudo).await?,
		Command::Verify { id } => cmd_verify(&chain, id).await?,
		Command::Watch { interval, no_sudo } => cmd_watch(&chain, &cli.seed, interval, no_sudo).await?,
	}

	Ok(())
}
