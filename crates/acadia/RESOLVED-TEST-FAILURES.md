# Resolved Test Failures

## Resolved 2026-11-19 07:23:17 -05:00 -- Windows notification deadline assertion treated a permitted early hint as a failure

The Windows `OsFs` oplock path may ring after an unrelated handle breaks the native lock, even while the published modification count remains unchanged. The public notification contract permits an early hint because the caller samples again. The original deadline test asserted the stronger poller-only property on that native path, so a permitted hint failed the suite intermittently.

The deadline-bound assertion now runs over the explicit polled factory. A separate default/native test accepts an early hint only after resampling the unchanged count, preserving the actual product contract while making the suite deterministic.

Original unresolved entry:

> ## `tests::coord::notify::a_registration_rings_when_its_deadline_passes_and_not_before` rang early once, 2026-11-18
>
> Observed once in a run of the `tests::coord` suite on Windows while the eighth review's fixes were made; it passed alone and in two further runs of the same suite. The failure message names a ring before the deadline; which backend and implementation it came from was not captured.
>
> A registration rings early only when its participant's sample differs from the value it registered with, or the sample fails. Nothing moves the count in this case. The suspected cause is the Windows oplock path over `OsFs`: another process opening the count file, such as a scanner, breaks the lock, and the settling samples then ring at their last step. The contract allows a ring the count did not cause, since the caller samples again; if that is the cause, the test's assertion is stronger than the contract on that path. Not yet reproduced or confirmed.
