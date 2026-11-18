# Unresolved Test Failures

## `tests::coord::notify::a_registration_rings_when_its_deadline_passes_and_not_before` rang early once, 2026-11-18

Observed once in a run of the `tests::coord` suite on Windows while the eighth review's fixes were
made; it passed alone and in two further runs of the same suite. The failure message names a ring
before the deadline; which backend and implementation it came from was not captured.

A registration rings early only when its participant's sample differs from the value it registered
with, or the sample fails. Nothing moves the count in this case. The suspected cause is the Windows
oplock path over `OsFs`: another process opening the count file, such as a scanner, breaks the lock,
and the settling samples then ring at their last step. The contract allows a ring the count did not
cause, since the caller samples again; if that is the cause, the test's assertion is stronger than
the contract on that path. Not yet reproduced or confirmed.
