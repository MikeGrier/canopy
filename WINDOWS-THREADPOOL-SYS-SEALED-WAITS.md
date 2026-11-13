# windows-threadpool-sys family: waits on an object the waiter cannot change

A request to [MikeGrier/windows-threadpool-sys](https://github.com/MikeGrier/windows-threadpool-sys)
and the crates beside it (`win-handles-sys`, `win-sync-sys`, `windows-threadpool-sys`), written
2026-11-13. It follows
[WINDOWS-THREADPOOL-SYS-WAIT-ONLY-EVENT.md](WINDOWS-THREADPOOL-SYS-WAIT-ONLY-EVENT.md), which gave a
thread-pool wait a handle that can only wait, and asks for the rest of what makes that guarantee
hold against safe code.

The aim is that the owner of an event can let other code wait on it -- on the thread pool, which is
the use the family exists to encourage, or with a single- or multiple-object wait -- and be certain
that no safe code anywhere can set or reset it as a result. Code that does so must have written
`unsafe`.

## Why canopy needs it

canopy's doorbell ([crates/canopy/src/ring/doorbell.rs](crates/canopy/src/ring/doorbell.rs)) is a
protocol over a manual-reset event. Its model, `Doorbell.tla`, assumes nothing but the protocol's
own steps changes the event's level; a reset from outside can leave the doorbell rung with its
event low, after which no ring wakes the consumer. Two requirements follow
([crates/canopy/DESIGN-NOTES.md](crates/canopy/DESIGN-NOTES.md) "The doorbell's signal is sealed"):
the analysis may treat the doorbell's layer as sealed, and only an `unsafe` caller can break the
seal. Today canopy meets them by lending the event only through an `unsafe` accessor. With what this
document asks for, it could lend a safe one.

## The protection is the type system, not the handle's access

Once code holds a handle to the event, all bets are off: code working at that level can reach the
event's original handle whatever the access of the one it holds, so neither a handle's access mask
nor the event's security descriptor protects the level. What protects it is the Rust type system
and the line between safe and `unsafe`: **no safe code may obtain a handle to the event at all**
-- not an `OwnedHandle`, a `BorrowedHandle`, a `RawHandle`, nor anything that implements
`AsHandle` -- except through an `unsafe` operation whose contract forbids changing the level.

Rust's I/O safety guarantees only that a borrowed handle stays open while it is borrowed, not what
is done with it. Changing an event's level is not memory-unsafe, so any crate may offer it as a safe
function over `impl AsHandle`; `win-handles-sys`'s safe `duplicate_handle_with_access` is one such
function already in the family. So every safe route to a handle is a breach, whatever that handle's
access.

## Where safe code reaches the handle today

1. **`WaitOnlyEvent` gives its handle to safe code.** It implements `AsHandle` and `AsRawHandle`,
   offers `try_into_owned`, and converts into a `SharedHandle`.
2. **A thread-pool wait gives its handle back.** In `windows-threadpool-sys`,
   `WaitableHandle::handle` and `WaitActivation::handle` return a `BorrowedHandle`, and
   `WaitableHandle::into_handle` an `OwnedHandle`, whatever the wait was built from. A callback
   holding the activation reaches the handle.
3. **The single- and multiple-object waits take `impl AsHandle`.** `win-sync-sys`'s
   `wait_for_single_object`, `wait_for_any` and `wait_for_all` can only be given a handle a caller
   can borrow, so whoever is to wait on the event must first be lent one.

## What the family would provide

**A wait-only event with no safe way to its handle, in `win-sync-sys`.** A new type beside
`WaitOnlyEvent` -- which is public, so taking `AsHandle` from it would be breaking -- say
`SealedWaitOnlyEvent`, made by `Event::duplicate_sealed_wait_only`:

- it does not implement `AsHandle`, `AsRawHandle` or `Into<SharedHandle>`, and has no
  `try_into_owned` or other way to an owned handle;
- its handle is reachable only through `unsafe fn as_waitable_handle(&self) -> BorrowedHandle<'_>`,
  whose contract is that the caller only waits on the handle, never changes the event through it
  or anything derived from it, and does not let it reach code that might;
- `Clone`, `Send` and `Sync`, as `WaitOnlyEvent` is, and it keeps the event open while any clone
  lives.

**Waits that accept it, in `win-sync-sys`.** `wait_for_single_object`, `wait_for_any` and
`wait_for_all` take a waitable that is either a borrowed handle or a `&SealedWaitOnlyEvent` -- for
example a `WaitObject<'a>` made by `From<BorrowedHandle<'a>>` and `From<&'a SealedWaitOnlyEvent>`
-- so that a caller can mix the sealed event with its own handles in one wait. Where changing the
existing signatures would be breaking, new functions can sit beside them.

**A thread-pool wait that never lends a sealed target, in `windows-threadpool-sys`.**
`From<SealedWaitOnlyEvent> for WaitableHandle`, or a constructor, with the wait's handle unreachable
from safe code: `WaitableHandle::handle`, `WaitableHandle::into_handle` and `WaitActivation::handle`
must not return it. Two shapes:

- a type parameter or a separate type for a sealed wait (`ThreadpoolWait<Sealed>`,
  `SealedWaitActivation`) whose activation has `rearm` and no `handle`, so the absence is a
  compile-time fact; preferred, since a callback cannot ask for what is not there;
- or the existing accessors return an `Option`, `None` for a sealed target, which is breaking and
  moves the absence to run time.

Either way, recovering the handle from a sealed wait is an `unsafe fn` with the same contract as
`as_waitable_handle`.

**Documentation** in each crate: what a sealed wait-only event guarantees -- no safe code reaches a
handle to it, so no safe code can change the event through it -- and what it does not: an `unsafe`
caller that breaks the contract, and whoever holds the `Event` it was made from. The re-arm advice
for a manual-reset event is followed through whatever owns the `Event`, as for `WaitOnlyEvent`.

**Tests**: a thread-pool wait built from a sealed event runs its callback when the `Event` is set
and re-arms as the `WaitOnlyEvent` tests show; a multiple-object wait mixing it with an ordinary
event reports whichever is signalled; and `compile_fail` doctests showing that `AsHandle`,
`as_handle`, `into_handle` and `WaitActivation::handle` are not available for it.

## Releases

New types in `win-sync-sys` and `windows-threadpool-sys`, additive if the existing functions are
kept, and a minor release of each.

## The canopy and acadia work that follows

- acadia: `EventSignal::sealed_wait_only`, beside or in place of `EventSignal::wait_only`, which
  today sits behind canopy's `unsafe` accessor.
- canopy: a safe accessor on the doorbell for a consumer's own native wait, lending the sealed
  event by value, and a decision on whether `native_wait_handle` stays as `unsafe` or is retired;
  canopy's thread-pool test registering through the safe one. `Doorbell::watch` (canopy's `RG-3`,
  over acadia's `AC-4`) stays the recommended path, which registers on the thread pool inside the
  sealed layer and needs none of this.
- The Linux side is not the family's: an `eventfd` has no wait-only form, so canopy keeps the
  `unsafe` descriptor accessor there and offers `watch` as the safe path.

Recorded in [crates/canopy/DESIGN-NOTES.md](crates/canopy/DESIGN-NOTES.md) "The doorbell's signal
is sealed"; the canopy and acadia work is `RG-4` in
[crates/canopy/CHECKLIST.md](crates/canopy/CHECKLIST.md), blocked on the family's release.
