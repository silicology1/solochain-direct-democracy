Good question — and your instinct about the early-voter disadvantage is pointing at the real issue. Let me lay out the game theory honestly, because "allow replacement" and "your actual concern" have different fixes.

## What re-voting on open ballots actually does

Your ballots are public from the moment they're cast (`Ballots` is plain chain state anyone can read). That's the crucial fact. With **open ballots + allowed replacement**, the game changes like this:

**1. Early votes become pure information gifts.** As you said: an early voter reveals exactly which candidate they support *and* which they're burying with −5. An opponent can then compute, exactly, the score pattern needed to flip the outcome and counter it. Under immutable ballots, this is costly for the early voter (they can't respond). Under re-voting, it triggers tit-for-tat adjustment until someone runs out of time — the end of the voting period becomes an escalating auction between coalitions, won by whoever can field more funded accounts or react faster.

**2. The rational equilibrium is "everyone waits."** Since voting early only exposes you, sophisticated voters delay to the final block. Re-voting doesn't fix your concern — it *guarantees* the behavior you're worried about, just concentrated at the deadline.

**3. Deadline races are dangerous here specifically.** Voting is feeless (`feeless_if`), so there's no cost to spamming ballot replacements in the last block, and outcome-deciding becomes an ordering lottery inside one block (whoever's tx lands first/last). That's an attack surface validators and well-resourced actors can exploit — worse than the current design, not better.

**4. Herding amplification.** Open + replaceable scores reward "back the current perceived leader" and "bury the current leader" strategies (bandwagoning and anti-crowding), which in score voting distorts toward popularity contests rather than genuine preference. STAR's runoff stage adds one more wrinkle: late movers can also game *which two candidates make the runoff*.

## The standard fix for your actual concern

The problem isn't replaceability — it's **transparency during the voting window**. The classic remedy is **commit–reveal**:

- During voting: `vote` stores only `blake2_256(scores ++ salt)` — a commitment. Nothing is revealed, so there's nothing to counter-vote against. Early voters pay no information cost, so no one is penalized for voting early.
- Replacement of the *commitment* is then safe and positive: it lets voters correct mistakes or update opinions without creating a strategic battleground, because nobody can see what they're replacing.
- After close: a reveal window where voters submit `(scores, salt)`, the pallet verifies the hash, and only revealed ballots count (unrevealed ≈ abstention). The tally service reads revealed ballots only; digest and verify logic unchanged.

This is the game-theoretically sound version of what you proposed: **you get re-voting (mistake correction, no early-voter penalty) and you kill the "competition to make someone win" dynamic**, because the information that powers it no longer exists during voting.

Trade-offs to be aware of: a second transaction per voter (reveal), unrevealed ballots are silently dropped (mitigate with a generous reveal window and client-side prompting), and it's a bigger change than flipping the `AlreadyVoted` check into an overwrite.

## Options, ranked

1. **Commit–reveal, replacement allowed for commitments only** — fixes your concern directly; re-vote becomes a feature, not a vulnerability. ~Medium change: new storage for commitments + revealed ballots, a `reveal` extrinsic with a reveal window config, tally client reads revealed ballots, tests + client updates.
2. **Keep as-is (immutable ballot, fully open)** — simplest, audit-friendly; accept that rational voters may delay and that late movers can counter-vote. Defensible for Phase 1 precisely because a trusted submitter exists anyway.
3. **Open ballots + replacement** (your original proposal) — I'd advise against: it doesn't fix the early-voter disadvantage and adds deadline-race and free-spam attack surface that immutable ballots at least bound.

One honest caveat: commit–reveal doesn't remove herding on *beliefs* (people still see turnout, and past election results), but it removes the dominant targeted counter-voting strategy you described.

Want me to write up an implementation plan for option 1 (pallet storage/extrinsic changes, reveal-window config, digest/verify impact, tally-client + tests)? If so, tell me the agent to switch to and I'll draft it — or we can keep discussing the trade-offs first.



**Short answer:** allowing updates doesn't fix the tactical-voting problem under open ballots, and mostly makes it worse. If you want updates, hide ballots until `voting_end`.

**Why open ballots are already a problem (no updates)**

- STAR is a non-dictatorial rule with 3+ candidates, so by Gibbard–Satterthwaite it is manipulable. Nothing here changes that.
- With public ballots the vote is a sequential game with perfect information about earlier moves. Any voter can mimic what they'd have done by voting earlier, so voting later **weakly dominates** voting earlier. Late voters can see the running totals and exaggerate (min-max scoring), which pushes STAR toward approval-like behavior.
- Public ballots also make vote-buying and coercion verifiable.

**What updates add**

1. **Deferral stays dominant.** The set of reachable final ballots is the same whether you vote early or late, but waiting gives you more information. So rational voters wait until the last block. Early ballots then carry no real information, and the game collapses to a last-block race.
2. **Last-block sniping becomes an ordering (MEV) problem.** Whoever controls transaction ordering within the final block has the full-information advantage. The tally service or block author sees everything.
3. **Early ballots become cheap talk.** A voter can post a decoy ballot to shift others' behavior, then switch. Revision with no cost is a bluffing tool.
4. **No equilibrium guarantee.** Best-response revisions can cycle, as in a Condorcet cycle, so the final outcome depends on who moves last, not on preferences.
5. **Coercion is not fixed.** Updates normally help against coercion, because you can revote secretly. With public ballots the buyer just checks at the deadline, so that benefit is lost.

I can't give you a full formal proof, since the result depends on the information and utility model, but points 1 and 2 are the core of it and follow from the dominance argument above.

**What I'd do**

- **Best:** keep ballots encrypted or committed until `voting_end`, using commit-reveal or threshold encryption. Then allow updates, last-write-wins. There's no information to exploit, revoting protects against coercion, and honest mistakes can be fixed. Tactical voting then only works from polls, not from live ballots.
- **If ballots must stay public:** keep one ballot per voter, as the pallet does now (`AlreadyVoted`). The information advantage remains, but you avoid decoy ballots and cycling.
- **Middle ground:** a hidden-until-end vote with a limited number of updates per voter, to bound storage churn. This matters because `vote` is feeless, so unlimited updates are a spam vector.

If you want, I can sketch the commit-reveal changes to `vote` and `submit_tally`.


**Yes, but only if ballots are hidden until `voting_end`. With public ballots, a cap doesn't fix the game theory.**

**With public ballots**
- A cap doesn't remove the late-voting advantage. A sophisticated voter can still wait, watch the totals, and spend their one vote (or their last revision) in the final block.
- A cap does stop endless best-response cycling and bounds decoy bluffing (one decoy, then one real vote).
- It creates an unfairness: voters who vote honestly early and don't watch lose out to voters who watch and revise tactically.

**With hidden ballots (commit-reveal or threshold encryption)**
- Revisions can't be exploited, since nobody sees anything to respond to.
- 1-2 revisions is enough to cover the real benefits: fixing honest mistakes and re-voting if someone coerced you. A coercer can't verify which ballot is final.
- More than about 2 gives almost no extra benefit and mostly adds storage and event churn.

**Because `vote` is feeless**, the cap also bounds spam. Without it, one account could rewrite its ballot every block. With a cap, each voter has at most `1 + max_revisions` writes per election.

**Implementation notes**
- Track revisions per voter, either in a new `Revisions: (ElectionId, AccountId) -> u8` map or by storing `(scores, revisions_used)` in `Ballots`.
- Replace `AlreadyVoted` with `RevisionLimitReached`, and make `MaxRevisions` a `Config` constant.
- Don't increment `BallotCount` on a revision.
- Last write wins, so the tally client must read the final stored ballot. Emit `VoteRevised` so observers can tell the difference.

**Recommendation:** hidden ballots plus a limit of 1 or 2 revisions. If ballots must stay public, keep the current one-vote-only rule, since revisions there only help the people who watch most closely.

