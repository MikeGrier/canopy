# windows-threadpool-sys: accept a `WaitOnlyEvent` as a wait target

A change requested of
[MikeGrier/windows-threadpool-sys](https://github.com/MikeGrier/windows-threadpool-sys), and the
canopy work that follows it. Decided 2026-11-12: a client waits on canopy's completion doorbell
through a handle that can only wait, and `windows-threadpool-sys` takes such a handle without
`unsafe`.

## Why

canopy's doorbell ([crates/canopy/src/ring/doorbell.rs](crates/canopy/src/ring/doorbell.rs)) is a
protocol over a manual-reset event: a producer sets it, and only the consumer resets it, in a fixed
order with the record of being rung. A reset from anywhere else can leave the doorbell rung with
its event low, after which every ring skips its set and the consumer is never woken
([crates/canopy/DESIGN-NOTES.md](crates/canopy/DESIGN-NOTES.md) "The doorbell's contract and its
mechanism, modelled in `Doorbell.tla`" and "The doorbell in code, `canopy::ring::Doorbell`").

On Windows the event is a `win-sync-sys` `Event`, held by acadia's `EventSignal`
([crates/acadia/src/signal/windows.rs](crates/acadia/src/signal/windows.rs)). A client must be able
to register the completion doorbell with a thread-pool wait, and canopy must not hand it anything
that can reset the event:

- An `Event` clone can reset it, so canopy will not lend one.
- `win-sync-sys`'s `WaitOnlyEvent` (`Event::duplicate_wait_only`) carries `SYNCHRONIZE` alone, so
  the kernel refuses a set or a reset through it. It is the right thing to lend.
- `windows-threadpool-sys` 0.4.0 takes a `WaitableHandle` without `unsafe` only from an `Event`
  (`impl From<Event> for WaitableHandle`). From a `WaitOnlyEvent` the only path is
  `WaitableHandle::assume_waitable_shared(SharedHandle::from(wait_only))`, which is `unsafe`, so a
  client following the safe path would have to be given the `Event`.

## The change to windows-threadpool-sys

1. **`impl From<WaitOnlyEvent> for WaitableHandle`** in `src/wait.rs`, beside
   `From<Event>`, building `WaitTarget::Shared` from `SharedHandle::from(event)`. It is sound for
   the reason `From<Event>` is: `win-sync-sys` guarantees a `WaitOnlyEvent` is an event and never a
   mutex, and that guarantee holds without `unsafe` because a `WaitOnlyEvent` can only be made from
   an `Event` (`win-sync-sys` WS-D-4 and WS-D-7). `SYNCHRONIZE` is the access a thread-pool wait
   needs.
2. **Documentation** on the new conversion, on `WaitableHandle`, and in the README's "Waits own a
   handle of proven provenance":
   - a wait built from a `WaitOnlyEvent` cannot be used to signal or reset the event, by the
     callback or by anyone holding the wait, which is the point of building it so;
   - `WaitActivation::handle` then borrows a wait-only handle, so the advice on
     `WaitActivation::rearm` to reset a manual-reset event before re-arming has to be followed
     through whatever owns the `Event`, or the callback re-arms on a still-signalled event and runs
     again at once. For a protocol like canopy's doorbell that is the intended behaviour: the
     owner's consumer step lowers the event, and the callback re-arms after it.
   - `WaitableHandle::into_handle` gives the handle back as its last holder, as it does for an
     `Event`, with `SYNCHRONIZE` access only.
3. **Tests**, in the style of the existing `From<Event>` ones:
   - a wait built from `event.duplicate_wait_only()` runs its callback when the `Event` is set;
   - re-arming from the callback after the owner resets the event waits for the next set, and
     re-arming without a reset activates again at once (manual-reset level);
   - the handle the wait and `WaitActivation::handle` lend refuses `SetEvent` and `ResetEvent`
     with access denied;
   - the wait keeps the handle open after every other clone of the `WaitOnlyEvent` and the
     `Event` have dropped, and `into_handle` returns it once the wait is the last holder.
4. **Release** as a minor version (0.5.0 if the 0.4 series is otherwise unchanged): the new
   `From` impl is additive, but `win-sync-sys`'s `WaitOnlyEvent` becomes part of the public API,
   so a `win-sync-sys` minor release is a minor release here, as `windows-overlapped-io-sys`'s
   already is.

An alternative with the same effect is a constructor, `WaitableHandle::wait_only(event: &Event) ->
io::Result<Self>`, which duplicates the event itself. The `From` impl is preferred: it lets the
owner of the event decide when the wait-only handle is made, and matches the existing conversion.

## The canopy work that follows

Once the release exists:

1. acadia: `EventSignal::wait_only(&self) -> io::Result<WaitOnlyEvent>`, through
   `Event::duplicate_wait_only`, beside `EventSignal::event`.
2. canopy: the completion doorbell lends the wait-only event on Windows, publicly, and canopy's
   tests register a `ThreadpoolWait` from it instead of from the `Event`; the test-only
   `Doorbell::signal` accessor goes away if nothing else needs it.
3. canopy: bump the `windows-threadpool-sys` dev-dependency
   ([crates/canopy/Cargo.toml](crates/canopy/Cargo.toml)) to the release.

This is part of API-3 in [crates/canopy/CHECKLIST.md](crates/canopy/CHECKLIST.md), which carries
the obligation; this file is the specification of the upstream change it depends on.
