# acadia design rationale

## Discovery inspection

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Recovery discovery lends a
read-only inspection". A caller that reads records after discovery returns cannot know whether
a writer acquired exclusion between those reads. A callback inside the probe preserves the
existing observation boundary without changing ownership, generations or recovery state.
Exposing a bookkeeping-bearing writer hold for diagnostics would instead alter the state being
diagnosed. The inspection is skipped wherever no free probe exists.

## Alternate process factory

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Test support supplies a distinct
file coordination implementation". A process test needs a real second factory to demonstrate
implementation admission, not an injected mismatch error or a hand-forged reservation. The
existing alternate implementation supplies that independent identity and creation exclusion.
Keeping it behind test support leaves the canonical factory's specification unchanged.

## Incremental tier conversion

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Incremental upgrade conversion
retains the admission gate". A native blocking conversion cannot cancel between acquisitions.
An owned polling conversion keeps partial holds explicit and lets dropping it cancel promptly.
Retaining the gate in the result lets the consumer make its schema admission visible before readers
can attach. The blocking API's automatic reader reopening is a distinct contract, and its native
wait path is preserved rather than emulated by polling.