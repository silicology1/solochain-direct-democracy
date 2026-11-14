# star-voting-tally — off-chain tally client for `pallet-star-voting`

This crate is the **Phase 1** off-chain companion to the on-chain
`pallet-star-voting` pallet. It:

1. reads ballots straight from chain state,
2. runs the multi-winner STAR tally locally (top-W by *real* signed total
   score, ties broken by smaller candidate id),
3. submits the result on-chain via `StarVoting::submit_tally`, and
4. can independently re-verify a stored result against the ballots.

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
cargo run -- tally 0               # tallies and submits via Sudo
cargo run -- verify 0              # recomputes and cross-checks the stored result
cargo run -- watch                 # daemon: tally every election whose voting ended
```

Global flags: `--url` (default `ws://127.0.0.1:9944`) and `--seed` (secret
URI, default `//Alice`).

### Sudo note

In Phase 1 the pallet's `TallyOrigin` is `EnsureRoot`, so `submit_tally` is
submitted wrapped in `Sudo::sudo` and the signer must be the sudo account
(`//Alice` on dev chains). `tally --no-sudo` / `watch --no-sudo` submit the
bare call instead, for runtimes whose `TallyOrigin` accepts signed accounts.

Voting (`vote`) is **gasless**: the pallet marks the call with
`#[pallet::feeless_if]` and the runtime wraps `ChargeTransactionPayment`
with `SkipCheckIfFeeless`, so no fees are charged for casting a ballot.

## Result digest binding

The pallet recomputes, on-chain, over whatever `submit_tally` receives:

```text
result_hash = blake2b_256(scale_encode((
    election_id: u32, winner_count: u16, winners: &[u32], totals: &[(u32, u32)]
)))
```

The client re-derives the same digest (`src/digest.rs`) from the ballots it
read, so a stored `Tally.result_hash` always binds the payload. The test
vectors on both sides must agree:

- pallet: `cargo test -p pallet-star-voting digest_cross_check -- --nocapture`
- client: `cargo test -p star-voting-tally-client`

## Testing

```sh
cargo test          # tally algorithm + digest cross-check
```

The tally algorithm mirrors the on-chain ballot format: scores are stored
as offsets (`raw score + 5`, so `-5..=5` → `0..=10`) with `255` meaning
"deliberately abstained". Abstentions are excluded from both the total and
the voter count, keeping each voter's score scale comparable.
