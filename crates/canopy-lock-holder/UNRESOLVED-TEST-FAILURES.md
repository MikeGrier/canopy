# Unresolved Test Failures

## 2026-11-18 -- `shared_count` `upgrades::a_killed_upgrader_resumes_after_every_durable_body_boundary` times out in the full suite

Seen on Windows while finishing acadia's fifth-review fixes (acadia
[CHECKLIST.md](../acadia/COMPLETED-CHECKLIST.md) M28 to M31). Run with the rest of the package, the test
fails at
[shared_count.rs](tests/shared_count.rs) `Writer::expect`: the resuming writer does not report
`recovered` within `STEP_DEADLINE` (30 s). Run alone it passes, taking about 33 s for all its
units.

- **Not caused by the change under test.** With the acadia changes stashed, the full suite at
  `e422686` fails the same way, three runs out of three; the same commit's full run passed on
  2026-11-17.
- **Suspected load, not established.** The test kills and resumes an upgrader once per body unit,
  each resume spawning a process that recovers the store, while the other cross-process tests in
  the binary run in parallel. A per-step deadline sized for an idle machine is the first thing to
  check; a resume that genuinely stalls behind another test's process is the second.
