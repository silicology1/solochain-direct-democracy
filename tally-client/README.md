# star-voting-tally — off-chain tally client for `pallet-star-voting`

This crate is the **Phase 1** off-chain companion to the on-chain
`pallet-star-voting` pallet. It:

1. reads ballots straight from chain state,
2. runs a multi-winner tally locally, in one of three selectable methods
   (see [Tally methods](#tally-methods)),
3. submits the result on-chain via `StarVoting::submit_tally` (score) or
   `StarVoting::submit_tally_star` (Bloc STAR / STAR-PR), and
4. can independently re-verify a stored result against the ballots, using
   the tally method stored on-chain.

It is a standalone crate (own `[workspace]`) so the `subxt` dependency stack
never enters the node build.

## Typed interface (static codegen)

The client uses subxt's **static codegen** (`src/chain.rs`):

```rust
#[subxt::subxt(runtime_metadata_path = "artifacts/metadata.scale")]
pub mod api {}
```

All calls and storage reads are compiler-checked against the runtime.
Because of that, `artifacts/metadata.scale` **must be regenerated and
recommitted whenever the runtime changes** (any pallet change ⇒ bump
`spec_version` and regenerate), otherwise the client may compile fine but
disagree with the chain, or fail metadata validation at runtime.

### Regenerating `artifacts/metadata.scale`

```sh
# 1. Build and start the node briefly (dev chain, ephemeral db).
cargo build -p solochain-template-node
./target/debug/solochain-template-node --dev --tmp &

# 2. Fetch and decode the runtime metadata into a binary SCALE file.
curl -s -H "Content-Type: application/json" \
     -d '{"jsonrpc":"2.0","id":1,"method":"state_getMetadata","params":[]}' \
     http://127.0.0.1:9944 \
| python3 -c '
import json, sys
d = json.load(sys.stdin)
open("artifacts/metadata.scale", "wb").write(bytes.fromhex(d["result"][2:]))
'

# 3. Stop the node, rebuild the client, commit the artifact.
kill %1
cargo build
```

(No `subxt-cli` install needed; `chain_getMetadata` is not exposed by this
node, use `state_getMetadata` as above.)

## Usage

```sh
cargo run -- create --winners 5 --candidates 1,2,3,4,5 --voting-period 60
cargo run -- vote 0 --seed //Bob   5,-2,0,a,3    # a = abstain
cargo run -- vote 0 --seed //Charlie 5,5,5,5,5
cargo run -- tally 0               # score method, via Sudo
cargo run -- --tally-method bloc-star tally 0   # Bloc STAR, via Sudo
cargo run -- tally 0 --tally-method star-pr     # STAR-PR, via Sudo
cargo run -- verify 0              # recomputes and cross-checks the stored result
cargo run -- watch                 # daemon: tally every election whose voting ended
```

Global flags: `--url` (default `ws://127.0.0.1:9944`), `--seed` (secret URI,
default `//Alice`), and `--tally-method score|bloc-star|star-pr`
(default `score`). Being global, `--tally-method` can sit before or after
the subcommand. `tally`/`watch` submit through `submit_tally` for `score`
and through `submit_tally_star` for `bloc-star`/`star-pr`. `verify` ignores
the flag and always re-runs the method stored on-chain — a submission can
be verified with the exact algorithm that produced it.

### Sudo note

In Phase 1 the pallet's `TallyOrigin` is `EnsureRoot`, so `submit_tally` and
`submit_tally_star` are submitted wrapped in `Sudo::sudo` and the signer
must be the sudo account (`//Alice` on dev chains). `tally --no-sudo` /
`watch --no-sudo` submit the bare call instead, for runtimes whose
`TallyOrigin` accepts signed accounts.

Voting (`vote`) is **gasless**: the pallet marks the call with
`#[pallet::feeless_if]` and the runtime wraps `ChargeTransactionPayment`
with `SkipCheckIfFeeless`, so no fees are charged for casting a ballot.

## Tally methods

All three methods share the same **score stage**: scores are stored as
offsets (`raw score + 5`, so `-5..=5` → `0..=10`) with `255` meaning
"deliberately abstained". Abstentions are excluded from both the total and
the voter count, keeping each voter's score scale comparable. Candidates
are ranked by their *real* signed total `offset_total − 5·voter_count`
(not the offset total), ties broken by smaller candidate id. The totals
submitted on-chain are always these offset totals, regardless of method.

### `score` — plain multi-winner score voting

The top `winner_count` candidates by real total win. Fast, simple, but a
minority that intensely prefers one candidate cannot protect it against a
mildly-preferred majority favorite.

### `bloc-star` — Bloc STAR (per-seat automatic runoff)

Elected seat by seat: take the two remaining candidates with the highest
real total, run an automatic runoff between them — each ballot supports the
finalist it scored *higher* (equal scores support neither; an abstain
compared against a real score supports the scored one; two abstentions
support neither) — and seat the majority-preference winner, then remove it
and repeat. A runoff tie goes to the score-stage leader; when the remaining
candidates exactly fill the remaining seats they are all seated in ranked
order. This is the classic STAR payoff: a burying minority can flip a seat
to the majority's true preference even when it loses the score stage.

### `star-pr` — STAR-PR (Allocated Score, proportional)

Proportional representation by sequential fractional ballot allocation
(Allocated Score). Each voter starts with weight 1. Per round:

1. quota `H = total remaining ballot weight / seats remaining`;
2. each candidate is "allocated" the ballots that scored it **above zero**
   (raw `1..=5`, offsets `6..=10`; zero scores and abstentions are never
   allocated), highest score first (ties in ascending voter-account order),
   up to `H`; the ballot crossing `H` is allocated only fractionally —
   a candidate's allocated support is `min(H, Σ positive-score ballot weights)`;
3. the seat goes to the candidate with the highest allocated support (ties →
   smaller candidate id), and only the winner's allocated ballots are spent
   (fully allocated ballots lose all weight; the boundary ballot keeps its
   remainder);
4. when the remaining candidates fill the remaining seats exactly, they are
   all seated in score-stage ranked order.

With this, a faction holding e.g. 20% of the voters can win its share of
seats, where plain score or bloc STAR would let the 80% majority sweep
everything.

## Result digest binding

The pallet recomputes, on-chain, over whatever `submit_tally` (score) /
`submit_tally_star` (Bloc STAR / STAR-PR) receives:

```text
score:      result_hash = blake2b_256(scale_encode((
                election_id: u32, winner_count: u16,
                winners: &[u32], totals: &[(u32, u32)])))

star:       result_hash = blake2b_256(scale_encode((
                tag: &[u8] = "pallet-star-voting/star/1",
                method: TallyMethod,       // Score=0, BlocStar=1, StarPr=2
                election_id: u32, winner_count: u16,
                winners: &[u32], totals: &[(u32, u32)])))
```

The domain tag and the method variant index keep a Bloc STAR / STAR-PR
result cryptographically distinguishable from a score result. The client
re-derives the same digest (`src/digest.rs`) from the ballots it read, so a
stored `Tally.result_hash` always binds the payload. The pinned test
vectors on both sides must agree:

- pallet: `cargo test -p pallet-star-voting digest_cross_check
  star_digest_cross_check -- --nocapture`
- client: `cargo test -p star-voting-tally-client`

## Testing

```sh
cargo test          # tally algorithms (score / bloc-star / star-pr) + digest cross-checks
```

The tally algorithms mirror the on-chain ballot format: scores are stored
as offsets (`raw score + 5`, so `-5..=5` → `0..=10`) with `255` meaning
"deliberately abstained". Abstentions are excluded from both the total and
the voter count, keeping each voter's score scale comparable.

The subxt metadata artifact in `artifacts/` was generated from the runtime
at `spec_version` 102 (the version that introduced `submit_tally_star` and
`TallyMethod`).

## License

Licensed under the [GNU Affero General Public License v3.0](../LICENSE)
(SPDX: `AGPL-3.0-only`, matching `license = "AGPL-3.0-only"` in
`Cargo.toml`).
