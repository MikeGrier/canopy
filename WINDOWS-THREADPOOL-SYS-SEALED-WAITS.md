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

## What lets safe code change the level today

Rust's I/O safety guarantees that a borrowed handle stays open while it is borrowed, not what is
done with it. Changing an event's level is not memory-unsafe, so any crate may offer it as a safe
function, and the question is only which handles safe code can reach.

1. **A handle can be duplicated with more access.** `win-handles-sys`'s
   `duplicate_handle_with_access(handle: impl AsHandle, access)` and `duplicate_handle` are safe.
   `win-sync-sys`'s documentation of `WaitOnlyEvent` says Windows lets a duplicate ask for more
   access than its source, so a `SYNCHRONIZE`-only handle can become one that sets and resets.
   (Not yet verified by a test; see step 1 below.)
2. **`WaitOnlyEvent` gives its handle to safe code.** It implements `AsHandle` and `AsRawHandle`,
   offers `try_into_owned`, and converts into a `SharedHandle`. Each is a way into 1.
3. **A thread-pool wait gives its handle back.** In `windows-threadpool-sys`,
   `WaitableHandle::handle` and `WaitActivation::handle` return a `BorrowedHandle`, and
   `WaitableHandle::into_handle` an `OwnedHandle`, whatever the wait was built from. A callback
   holding the activation reaches the handle, and so reaches 1.
4. **The multiple-object waits take `impl AsHandle`.** `win-sync-sys`'s `wait_for_any`,
   `wait_for_all` and `wait_for_single_object` can only be given a handle a caller can borrow, so
   whoever is to wait on the event must first be lent one.

If 1 is false -- a duplicate of a `SYNCHRONIZE`-only event handle cannot gain access -- then 2, 3
and 4 lend only a handle the kernel refuses to signal through, and `WaitOnlyEvent` is already a
sound seal. Everything below depends on that answer.

## Step 1 -- establish what the kernel allows

A test in `win-sync-sys`, beside its existing wait-only tests:

- make an `Event`, take `duplicate_wait_only`, and duplicate that with `EVENT_MODIFY_STATE`
  through `duplicate_handle_with_access`; record whether it succeeds and whether `SetEvent` and
  `ResetEvent` then work through the duplicate;
- the same for `EVENT_ALL_ACCESS`, `WRITE_DAC` and `WRITE_OWNER`, since a handle that can rewrite
  the event's security can grant itself the rest.

Record the result in `win-sync-sys`'s design notes as a finding, and correct `WaitOnlyEvent`'s
documentation if it says otherwise. If no escalation succeeds, step 2 and step 3 are unnecessary;
only the documentation changes, saying that a `WaitOnlyEvent` cannot be used to signal by any
holder.

## Step 2 -- make escalation fail in the kernel (preferred)

An event made so that no handle opened or duplicated after its creation can carry more than
`SYNCHRONIZE`, while the creator's own handle keeps full access:

- **`Event::new_sealed(mode, initial)`** (name to taste) in `win-sync-sys`: `CreateEventExW` with
  `EVENT_ALL_ACCESS` as the handle's access and a security descriptor whose DACL grants
  `SYNCHRONIZE` alone. The DACL includes an `OWNER RIGHTS` (S-1-3-4) ACE granting `SYNCHRONIZE`
  only, which removes the owner's implicit `READ_CONTROL` and `WRITE_DAC`, so the owner cannot
  rewrite the DACL either.
- **What must hold, each established by a test**: the handle `new_sealed` returns sets and resets;
  `duplicate_wait_only` of it waits and cannot signal; a duplicate of that wait-only handle asking
  for `EVENT_MODIFY_STATE`, `WRITE_DAC`, `WRITE_OWNER` or `EVENT_ALL_ACCESS` is refused; and the
  same requests made against the full-access handle with `DUPLICATE_SAME_ACCESS` absent are refused
  too, so the only full-access handles are the creator's and its same-access clones.
- **What it does not cover**, stated in its documentation: a process that enables
  `SeTakeOwnershipPrivilege` (an elevated process with the privilege granted) can take ownership
  and rewrite the DACL; the guarantee is against code in an ordinary process, not an administrator.
  The creator's handle and its `Clone`s keep full access, so the creator must not lend those.
- **Why preferred**: it changes no type and no signature in any crate of the family.
  `WaitOnlyEvent`, `From<WaitOnlyEvent> for WaitableHandle`, `WaitActivation::handle` and the
  multiple-object waits all stay as they are, and every handle they lend is one the kernel refuses
  to signal through, by any holder, with or without `unsafe`. The seal is the kernel's, so it holds
  across crates the family does not control.

With step 2, acadia makes its `EventSignal` with `new_sealed`, and canopy's doorbell can lend a
`WaitOnlyEvent` safely -- through `AsHandle` for a native wait and by value for a thread-pool wait
-- with the `unsafe` accessors kept only as an alias or retired.

## Step 3 -- an opaque wait-only handle (if step 2 cannot be made to hold)

If the kernel cannot be made to refuse escalation, the seal has to come from the types: a handle
that safe code can wait on but never borrow.

- **`win-sync-sys`: a wait-only event with no safe way to its handle.** A new type beside
  `WaitOnlyEvent` (`WaitOnlyEvent` is public, and taking `AsHandle` from it would be breaking),
  say `SealedWaitOnlyEvent`, made by `Event::duplicate_sealed_wait_only`. It does not implement
  `AsHandle`, `AsRawHandle` or `Into<SharedHandle>`, and has no `try_into_owned`. Its handle is
  reachable only through `unsafe fn as_waitable_handle(&self) -> BorrowedHandle<'_>`, whose
  contract is that the caller only waits on it and never duplicates it with more access nor lets
  it reach code that might. `Clone`, `Send` and `Sync` as `WaitOnlyEvent` is.
- **`win-sync-sys`: waits that accept it.** `wait_for_single_object`, `wait_for_any` and
  `wait_for_all` take a waitable that is either a borrowed handle or a `&SealedWaitOnlyEvent`, for
  example a `WaitObject<'a>` made by `From<BorrowedHandle<'a>>` and `From<&'a
  SealedWaitOnlyEvent>`, so that a caller can mix the sealed event with its own handles in one
  wait. The existing signatures can stay as they are with new functions beside them, if changing
  them is breaking.
- **`windows-threadpool-sys`: a wait that never lends a sealed target.** `From<SealedWaitOnlyEvent>
  for WaitableHandle` (or a constructor), with the wait's handle unreachable from safe code:
  `WaitableHandle::handle`, `WaitableHandle::into_handle` and `WaitActivation::handle` must not
  return it. Two shapes:
  - a type parameter or a separate type for a sealed wait (`ThreadpoolWait<Sealed>`,
    `SealedWaitActivation`) whose activation has `rearm` and no `handle`, so the absence is a
    compile-time fact; preferred, since a callback cannot ask for what is not there;
  - or the existing accessors return an `Option`, `None` for a sealed target, which is breaking
    and moves the absence to run time.

  Either way, recovering the handle from a sealed wait is an `unsafe fn` with the same contract as
  `as_waitable_handle`.
- **Documentation** in each crate: what a sealed wait-only event guarantees (no safe code reaches a
  handle to it, so no safe code can signal or escalate), what it does not (an `unsafe` caller that
  breaks the contract), and that the re-arm advice for a manual-reset event has to be followed
  through whatever owns the `Event`, as for `WaitOnlyEvent`.
- **Tests**: a thread-pool wait built from a sealed event runs its callback when the `Event` is
  set and re-arms as the `WaitOnlyEvent` tests show; a multiple-object wait mixing it with an
  ordinary event reports whichever is signalled; and `compile_fail` doctests showing that
  `AsHandle`, `as_handle` and `WaitActivation::handle` are not available for it.

With step 3, acadia makes a `SealedWaitOnlyEvent` from its `EventSignal`, and canopy's doorbell
lends that safely, by value, for a thread-pool wait or a multiple-object wait.

## Releases

Step 1 is documentation and a test. Step 2 is additive (a new constructor) and is a minor release
of `win-sync-sys`; `windows-threadpool-sys` needs no change. Step 3 adds types to `win-sync-sys`
and `windows-threadpool-sys`, additive if the existing functions are kept, and is a minor release
of each.

## The canopy and acadia work that follows

- acadia: `EventSignal` made with `Event::new_sealed` (step 2), or an `EventSignal::sealed_wait_only`
  (step 3), in place of `EventSignal::wait_only`'s role behind canopy's `unsafe` accessor.
- canopy: a safe accessor on the doorbell for a consumer's own native wait, and a decision on
  whether `native_wait_handle` stays as `unsafe` or is retired; canopy's thread-pool test
  registering through the safe one. `Doorbell::watch` (canopy's `RG-3`, over acadia's `AC-4`)
  stays the recommended path, which registers on the thread pool inside the sealed layer and needs
  none of this.
- The Linux side is not the family's: an `eventfd` has no wait-only form, so canopy keeps the
  `unsafe` descriptor accessor there and offers `watch` as the safe path.

Recorded in [crates/canopy/DESIGN-NOTES.md](crates/canopy/DESIGN-NOTES.md) "The doorbell's signal
is sealed"; the canopy and acadia work is `RG-4` in
[crates/canopy/CHECKLIST.md](crates/canopy/CHECKLIST.md), blocked on the family's release.
