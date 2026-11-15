# solochain-direct-democracy

A Polkadot SDK solochain implementing **gasless multi-winner STAR voting**
(Score Then Automatic Runoff, Phase 1 — no proving system yet), with an
off-chain tally service (`tally-client/`) that computes results from chain
state and posts a tamper-evident result hash on-chain.

Built on the [Polkadot SDK solochain
template](https://github.com/paritytech/polkadot-sdk/tree/master/templates/solochain).

## How it works

```
voters (feeless)             tally service (off-chain)          chain (sudo)
┌───────────────────┐        ┌──────────────────────────┐       ┌──────────────────────┐
│ StarVoting::vote  │──────▶ │ read Ballots from state  │       │ StarVoting::submit_  │
│ scores −5..+5     │        │ run the tally locally    │       │ tally(winners,totals) │
│ or abstain        │        │ score / bloc STAR /      │ sudo  │  or submit_tally_star │
│                   │        │ STAR-PR                  │       │  → recomputes hash    │
└───────────────────┘        └──────────────────────────┘       │  → binds Tally(id)   │
                                                               └──────────────────────┘
                                        anyone: re-derive hash from ballots & compare
```

- **Ballots** are stored per `(election, voter)` as one score offset per
  candidate: `raw score + 5` (so `-5..=+5` → `0..=10`), with `255` meaning
  *deliberate abstention*. Abstentions are excluded from both the total and
  the voter count, keeping each voter's score scale comparable.
- **Tally methods** (Phase 1): the pallet stores which method a result was
  computed with (`TallyMethod`: `Score`, `BlocStar`, `StarPr`); anyone
  verifying must recompute with the stored method. All three share the same
  score stage: candidates are ranked by their *real* signed total
  `offset_total − 5·voter_count` (not the offset total), ties broken by the
  smaller candidate id.
  - **Score**: the top `winner_count` candidates by real total win.
  - **Bloc STAR**: per seat, the top-2 remaining candidates by score go to an
    automatic runoff (each ballot supports the finalist it scored higher;
    equal scores and double abstentions support neither; an abstain-vs-scored
    pair supports the scored one; a runoff tie goes to the score-stage
    leader), the majority winner is seated and removed, repeat.
  - **STAR-PR** (Allocated Score): proportional. Each voter starts with
    weight 1; per round the quota is `H = remaining weight / seats left`,
    candidates are allocated the ballots that scored them *above zero*
    (highest score first, boundary ballot fractionally) up to `H`, the seat
    goes to the candidate with the most allocated support (ties → smaller
    id), and the winner's allocated ballots are spent. When the remaining
    candidates fill the remaining seats exactly, they are seated in ranked
    order.
- **Result binding**: the pallet recomputes, on-chain, over whatever
  `submit_tally` (score) / `submit_tally_star` (Bloc STAR / STAR-PR)
  receives:
  - score: `result_hash = blake2_256(scale_encode((election_id, winner_count, winners, totals)))`
  - STAR methods: `result_hash = blake2_256(scale_encode((STAR_DIGEST_TAG, method, election_id, winner_count, winners, totals)))`
    with domain tag `pallet-star-voting/star/1` and the `TallyMethod`
    variant index, so a Bloc STAR / STAR-PR result can never be confused
    with a score result.
  Anyone can independently recompute this from the ballots and compare with
  the stored `Tally.result_hash` — `star-voting-tally verify` does exactly
  that (cross-checked by unit tests on both sides).
- **Gasless voting**: the pallet marks `vote` with
  `#[pallet::feeless_if]` and the runtime wraps `ChargeTransactionPayment`
  with `pallet_skip_feeless_payment::SkipCheckIfFeeless`, so casting a
  ballot never charges a fee.

> **Phase 1 scope**: the tally service is trusted-but-auditable (the stored
> hash binds whatever was submitted, and `verify` lets anyone check it).
> Later phases will replace the trusted submitter with validity proofs
> (STARK/Stwo) of correct tallying.

## Project layout

| Path | What it is |
| --- | --- |
| `pallets/star-voting/` | The FRAME pallet: elections, ballots, tally submission |
| `runtime/` | The runtime wiring the pallet (index 7) and `SkipFeelessPayment` (index 8) |
| `node/` | The solochain node (Aura + GRANDPA, RPC) |
| `tally-client/` | Standalone off-chain tally service (subxt, static codegen) — see [its README](./tally-client/README.md) for metadata regeneration and CLI details |

## Getting started

Prerequisites: a working [Rust toolchain pinned by
`rust-toolchain.toml`](https://docs.polkadot.com/develop/_install/),
`protobuf-compiler` (for the node build), and `git`.

### Build

```sh
cargo build --release            # node + runtime + pallet
cargo build -p star-voting-tally-client --manifest-path tally-client/Cargo.toml  # tally service
```

Note: `tally-client/` is its own workspace (own `Cargo.lock`) so the subxt
dependency stack never enters the node build.

### Run a development chain

```sh
./target/release/solochain-template-node --dev --tmp
```

Dev chains use **Alice** and **Bob** as validators with several pre-funded
accounts, and **Alice** as the `sudo` account (which Phase 1 needs, see
below). State persists in a temp dir for the lifetime of the process; use
`--base-path ./my-chain-state/` to keep it.

### Tests

```sh
cargo test -p pallet-star-voting                 # pallet logic (incl. digest vector)
cargo test --manifest-path tally-client/Cargo.toml  # tally algorithm + digest cross-check
```

## Using the system

### 1. Create an election and vote (on-chain)

Extrinsics (pallet `StarVoting`):

- `create_election(winner_count, candidates, voting_period)` — 5..=500
  winners, at least that many unique candidates.
- `vote(election_id, scores)` — one offset per candidate; **feeless**.

### 2. Tally and submit (off-chain client)

Pick the tally method with the global `--tally-method score|bloc-star|star-pr`
flag (default `score`); place it before the subcommand or anywhere after it:

```sh
cargo run --manifest-path tally-client/Cargo.toml -- create --winners 5 --candidates 1,2,3,4,5
cargo run --manifest-path tally-client/Cargo.toml -- vote 0 --seed //Alice 5,-2,0,a,3   # a = abstain
cargo run --manifest-path tally-client/Cargo.toml -- vote 0 --seed //Bob   5,5,5,5,5
cargo run --manifest-path tally-client/Cargo.toml -- tally 0                            # score, via sudo
cargo run --manifest-path tally-client/Cargo.toml -- --tally-method bloc-star tally 0   # bloc STAR
cargo run --manifest-path tally-client/Cargo.toml -- tally 0 --tally-method star-pr    # STAR-PR
cargo run --manifest-path tally-client/Cargo.toml -- verify 0                           # recompute & check
cargo run --manifest-path tally-client/Cargo.toml -- watch                              # daemon
```

`tally`/`watch` submit through `submit_tally` for `score` and through
`submit_tally_star` for `bloc-star`/`star-pr`, binding the matching
domain-separated digest. `verify` ignores the flag and always re-runs the
method stored on-chain, so a submission can be checked with the exact
algorithm that produced it.

The client uses subxt **static codegen** compiled from
[tally-client/artifacts/metadata.scale](./tally-client/README.md#regenerating-artifactsmetadata_scale).
Whenever the runtime changes, bump `spec_version` and regenerate that
artifact — the exact procedure is in the
[client README](./tally-client/README.md).

### Sudo note (Phase 1)

`TallyOrigin` is currently `EnsureRoot`, so `submit_tally` and
`submit_tally_star` must be wrapped in `Sudo::sudo` by the sudo account
(`//Alice` on dev chains) — this is the default behavior of
`tally`/`watch`. Use `tally --no-sudo` for runtimes whose `TallyOrigin`
accepts signed accounts.

## Runtime details

- Pallet indices: `7` = `StarVoting`, `8` = `SkipFeelessPayment` (wraps
  `ChargeTransactionPayment` in the `TxExtension` to honor `feeless_if`).
- The digest payloads are identical on-chain
  (`pallets/star-voting/src/lib.rs::compute_result_hash` and
  `compute_star_result_hash`) and off-chain (`tally-client/src/digest.rs`);
  both sides are pinned by tests against the same three vectors (score,
  Bloc STAR, STAR-PR). Keep them in sync.
- Weights are hand-estimated placeholders in
  `pallets/star-voting/src/weights.rs`; regenerate from benchmarks
  (`pallets/star-voting/src/benchmarking.rs`,
  `cargo check -p pallet-star-voting --features runtime-benchmarks`) before
  production use.

## Development notes

- **Embedded docs**: `cargo +nightly doc --open`
- **Node CLI help**: `./target/release/solochain-template-node -h`
- **Polkadot-JS Apps**: connect the running node at
  `https://polkadot.js.org/apps/#/explorer?rpc=ws://localhost:9944`
- **Multi-node testnet**: see [Simulate a
  network](https://docs.polkadot.com/develop/quickstart/#start-a-local-network)
  in the Polkadot docs.
- The original template structure docs (node service, chain spec, FRAME
  primitives) still apply; this repo renamed the template pallet to
  `pallet-star-voting` and added the runtime wiring described above.

## License

This project is licensed under the [GNU Affero General Public License v3.0](./LICENSE)
(SPDX: `AGPL-3.0-only`). If you modify and offer it as a network service, you
must make the modified source available to that service's users.
