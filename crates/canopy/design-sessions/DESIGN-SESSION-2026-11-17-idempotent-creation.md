# Design session 2026-11-17 -- idempotent store creation

Resulting decision: [DESIGN-NOTES.md](../DESIGN-NOTES.md) "Store creation is idempotent", with
rationale in [DESIGN-RATIONALE.md](../DESIGN-RATIONALE.md) "Creation is idempotent, and a store's
nonce is set by its first user". The work is milestone M15 in [CHECKLIST.md](../CHECKLIST.md).
No open questions remain from this session.

## What prompted it

`tests::lock::identical_concurrent_creators_never_replace_a_completed_store` failed intermittently
(about 1 to 3 in 25 runs of the library test binary). Two threads call `Store::init_unsourced` on one
empty root with one configuration and one agent. The losing creator reported
`NotEmpty { entry: "files" }` instead of `AlreadyInitialized`. `MemFs` lists entries in sorted
order, so the listing held no marker: the winner was still building its trees. The refusal came
from `init_through`'s occupancy check made before writer exclusion; the check under exclusion would
have found the marker. The test required more than the design notes promised, which covered only a
creator finding the completed marker.

## The discussion

- The engineer asked whether `init_through` is creation or attachment. It is creation (`Store::init`,
  `init_unsourced`); opening is `open_through`. Creation ends by attaching to the store it made.
- The engineer asked whether the race matters if creation is idempotent. Creation was idempotent only
  up to the reservation: identical inputs share one reservation, because its nonce is derived from
  the inputs. Past that point the loser refused. Making it fully idempotent changes what the loser
  gets after waiting, but it must still wait: the early check is what observed a half-built store.
- The engineer asked for something extremely simple: either creation is simply idempotent, or the
  target must be empty and one atomic claim picks the one and only creator, and that is all. The
  engineer slightly preferred the idempotent model.
- The engineer proposed that creation store a zero nonce, and that an operation needing the nonce,
  which already holds exclusion, read it and, finding zero, generate and store one. This keeps
  creation idempotent and defers the nonce's specialization. It was not a lazy initialization
  discussed earlier; it is only how the nonce gets its value.
- Settled in the exchange that followed:
  - the value is drawn from the handle's nonce source, since canopy has no random source;
  - it is durable before it is used, so a crash cannot lose a value already handed out;
  - it lives in a canopy-owned record, not in the reservation, which is never rewritten;
  - identity (compared) is the store id, format version, layout and the agent's
    version-independent identifier; provenance (the winner's, not compared) is the creation time,
    the agent's software version and description, and the schemas. This supersedes binding every
    agent input into the reservation.
- The engineer chose that `Store::init` create through `Creation::create`, its trees and marker
  being the artifacts, which answers the open question of how `init` relates to the creation
  contract.
- Asked whether `init` should report created or existing (as `open` returns its discovery), the
  engineer preferred that it return the store, success either way, and asked for any reason beyond
  the earlier decisions to keep the distinction. None was found: no caller branched on it, the use
  it invites (seed only when created) is unsafe once a creator can die between the marker and the
  seeding, and the one capability lost, "fail if it already exists", is covered by refusing other
  identities and by a caller making its identity unique. The engineer chose the collapsed form.
