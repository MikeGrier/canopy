// Copyright (c) Michael Grier.

//! The registry of attached participants (canopy's DESIGN-NOTES.md, "The registry of attached
//! handles, modelled in `Coordination.tla`").

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use super::{Backend, on_every_implementation, participant, participant_on};
use crate::coord::file::FileCoordination;
use crate::coord::file::layout::{Layout, REGISTRY_ENTRIES};
use crate::coord::file::registry::decode;
use crate::coord::{
    Access, Admission, Attachment, CoordError, Coordination, CoordinationFactory, HeldTiers,
    ListedEntry, Listing, ReaderSafety, Registration, Registry, RegistryToken, TierHold, Waiting,
};
use crate::{Fault, FaultOp, FaultPolicy, FaultyFs, LockMode, Vfs, When};

fn attach(participant: &FileCoordination, access: Access) -> Box<dyn Attachment> {
    participant.attach(access, Waiting::Never).expect("attach")
}

fn registered(attachment: &mut dyn Attachment, payload: &[u8]) -> RegistryToken {
    match attachment.register(payload).expect("register") {
        Registration::Registered(token) => token,
        Registration::Unregistered => panic!("every entry is held"),
    }
}

fn list(participant: &FileCoordination) -> Listing {
    participant.list().expect("list")
}

fn live_tokens(listing: &Listing) -> HashSet<RegistryToken> {
    listing.live.iter().map(|entry| entry.token).collect()
}

/// The token entry `index`'s record names, read without its claim lock, or `None` if it has none.
fn recorded(participant: &FileCoordination, index: usize) -> Option<RegistryToken> {
    let bytes = participant
        .vfs()
        .read(&participant.layout().registry_entry(index))
        .ok()?;
    decode(&bytes).map(|entry| entry.token)
}

/// Takes entry `index`'s registry lock at `span`, as another participant mid-way through its work.
fn hold_registry_lock(
    participant: &FileCoordination,
    span: crate::LockSpan,
) -> crate::coord::file::HoldId {
    participant
        .vfs()
        .create_dir_all(&participant.layout().registry_dir())
        .expect("make the registry directory");
    participant
        .take_lock(
            &participant.layout().registry_lock(),
            LockMode::Exclusive,
            span,
            Waiting::Never,
        )
        .expect("the lock is free")
}

#[test]
fn a_registered_attachment_is_listed_with_its_tiers_and_payload() {
    on_every_implementation("coord-reg-listed", &|implementation, backend| {
        let mut reader = attach(&participant(implementation, backend), Access::Read);
        let mut writer = attach(&participant(implementation, backend), Access::Write);
        let reading = registered(&mut *reader, b"reader");
        let writing = registered(&mut *writer, b"writer");
        assert_ne!(reading, writing);
        let listing = list(&participant(implementation, backend));
        assert_eq!(listing.unknown, 0, "{}", backend.name());
        assert!(!listing.may_be_incomplete);
        let mut live = listing.live.clone();
        live.sort_by_key(|entry| entry.payload.clone());
        assert_eq!(
            live,
            vec![
                ListedEntry {
                    token: reading,
                    tiers: HeldTiers {
                        reader: Some(TierHold::Shared),
                        writer: None
                    },
                    payload: b"reader".to_vec(),
                },
                ListedEntry {
                    token: writing,
                    tiers: HeldTiers {
                        reader: Some(TierHold::Shared),
                        writer: Some(TierHold::Shared)
                    },
                    payload: b"writer".to_vec(),
                },
            ],
            "{}",
            backend.name()
        );
    });
}

#[test]
fn an_empty_registry_lists_nothing_and_is_complete() {
    on_every_implementation("coord-reg-empty", &|implementation, backend| {
        let listing = list(&participant(implementation, backend));
        assert_eq!(listing, Listing::default(), "{}", backend.name());
        let _unregistered = attach(&participant(implementation, backend), Access::Write);
        assert_eq!(
            list(&participant(implementation, backend)),
            Listing::default()
        );
    });
}

#[test]
fn registering_again_replaces_the_payload_and_keeps_the_token() {
    on_every_implementation("coord-reg-again", &|implementation, backend| {
        let mut attachment = attach(&participant(implementation, backend), Access::Write);
        let token = registered(&mut *attachment, b"serving");
        assert_eq!(registered(&mut *attachment, b"draining"), token);
        let listing = list(&participant(implementation, backend));
        assert_eq!(listing.live.len(), 1, "{}", backend.name());
        assert_eq!(listing.live[0].token, token);
        assert_eq!(listing.live[0].payload, b"draining");
    });
}

#[test]
fn an_update_while_the_entry_is_inspected_is_refused_and_keeps_the_payload() {
    on_every_implementation("coord-reg-update-held", &|implementation, backend| {
        let mut attachment = attach(&participant(implementation, backend), Access::Read);
        registered(&mut *attachment, b"before");
        let lister = participant(implementation, backend);
        let claim = hold_registry_lock(&lister, Layout::claim(0));
        assert!(matches!(
            attachment.register(b"after"),
            Err(CoordError::WouldWait)
        ));
        drop(lister.release_lock(claim));
        assert_eq!(
            list(&lister).live[0].payload,
            b"before",
            "{}",
            backend.name()
        );
    });
}

#[test]
fn a_detached_attachment_is_removed_from_the_registry() {
    on_every_implementation("coord-reg-detach", &|implementation, backend| {
        let observer = participant(implementation, backend);
        let mut attachment = attach(&participant(implementation, backend), Access::Write);
        registered(&mut *attachment, b"leaving");
        drop(attachment);
        assert!(list(&observer).live.is_empty(), "{}", backend.name());
        assert_eq!(recorded(&observer, 0), None, "its record is cleared");
    });
}

#[test]
fn a_dead_participants_entry_is_never_listed_and_is_reused() {
    on_every_implementation("coord-reg-dead", &|implementation, backend| {
        let observer = participant(implementation, backend);
        let mut previous = None;
        for round in 0..4 {
            let dying = participant(implementation, backend);
            let mut attachment = attach(&dying, Access::Read);
            let token = registered(&mut *attachment, &[round]);
            assert_eq!(recorded(&observer, 0), Some(token), "entry 0 is reused");
            assert_ne!(Some(token), previous);
            dying.abandon();
            drop(attachment);
            assert_eq!(
                recorded(&observer, 0),
                Some(token),
                "a death changes no entry"
            );
            let listing = list(&observer);
            assert!(listing.live.is_empty(), "{}: {round}", backend.name());
            assert!(!listing.may_be_incomplete);
            previous = Some(token);
        }
    });
}

#[test]
fn a_reuser_paused_before_writing_its_token_makes_the_entry_unknown_and_not_live() {
    on_every_implementation("coord-reg-paused", &|implementation, backend| {
        let observer = participant(implementation, backend);
        let dying = participant(implementation, backend);
        let mut attachment = attach(&dying, Access::Read);
        let dead = registered(&mut *attachment, b"dead");
        dying.abandon();
        drop(attachment);
        let reuser = participant(implementation, backend);
        let _claim = hold_registry_lock(&reuser, Layout::claim(0));
        let _liveness = hold_registry_lock(&reuser, Layout::liveness(0));
        let listing = list(&observer);
        assert!(
            !live_tokens(&listing).contains(&dead),
            "{}: the dead token beside the reuser's liveness lock",
            backend.name()
        );
        assert_eq!(listing.unknown, 1);
        assert!(listing.may_be_incomplete);
        let mut other = attach(&participant(implementation, backend), Access::Read);
        let token = registered(&mut *other, b"other");
        assert_eq!(recorded(&observer, 1), Some(token), "moved on to entry 1");
        reuser.abandon();
        let listing = list(&observer);
        assert_eq!(live_tokens(&listing), HashSet::from([token]));
        assert!(!listing.may_be_incomplete);
    });
}

/// The reuser's own claim lock is what keeps a listing from pairing its liveness lock with the dead
/// participant's token: the listing runs while the reuser writes its token.
#[test]
fn a_listing_while_a_reuser_writes_its_token_finds_the_entry_unknown() {
    for implementation in super::implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let make = |vfs: Arc<dyn Vfs>| participant_on(&implementation, vfs, &root);
        let dying = make(Arc::new(mem.another_process()));
        let mut attachment = attach(&dying, Access::Read);
        let dead = registered(&mut *attachment, b"dead");
        dying.abandon();
        drop(attachment);
        let lister = make(Arc::new(mem.another_process()));
        let entry = lister.layout().registry_entry(0);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let policy = {
            let seen = Arc::clone(&seen);
            FaultPolicy::new().inject(
                FaultOp::Write,
                &entry,
                Fault::observe_with(move |_, _| {
                    let listing = lister.list().expect("list");
                    seen.lock().expect("seen").push(listing);
                }),
                When::Always,
            )
        };
        let reuser = make(Arc::new(FaultyFs::new(mem.another_process(), policy)));
        let mut attachment = attach(&reuser, Access::Read);
        let token = registered(&mut *attachment, b"reuser");
        let seen = seen.lock().expect("seen");
        assert_eq!(seen.len(), 1, "the reuser wrote entry 0");
        assert!(
            !live_tokens(&seen[0]).contains(&dead),
            "the dead token is listed live"
        );
        assert_eq!(seen[0].unknown, 1);
        assert!(seen[0].may_be_incomplete);
        let observer = make(Arc::new(mem.another_process()));
        assert_eq!(live_tokens(&list(&observer)), HashSet::from([token]));
    }
}

/// An entry is cleared while its tiers are held, at a drop and at an upgrade's conversion alike,
/// so a listing never shows a participant that holds nothing.
#[test]
fn an_entry_is_cleared_while_its_tiers_are_still_held() {
    for implementation in super::implementations() {
        for upgrading in [false, true] {
            let mem = Backend::mem();
            let root = PathBuf::from(Backend::MEM_ROOT);
            let make = |vfs: Arc<dyn Vfs>| participant_on(&implementation, vfs, &root);
            let checker = make(Arc::new(mem.another_process()));
            let entry = checker.layout().registry_entry(0);
            let held_while_cleared = Arc::new(Mutex::new(Vec::new()));
            let policy = {
                let held = Arc::clone(&held_while_cleared);
                FaultPolicy::new().inject(
                    FaultOp::RemoveFile,
                    &entry,
                    Fault::observe_with(move |_, _| {
                        let tier = checker.take_lock(
                            &checker.layout().tiers_lock(),
                            LockMode::Exclusive,
                            Layout::writer_tier(),
                            Waiting::Never,
                        );
                        let refused = matches!(tier, Err(CoordError::WouldWait));
                        if let Ok(hold) = tier {
                            drop(checker.release_lock(hold));
                        }
                        held.lock().expect("held").push(refused);
                    }),
                    When::Always,
                )
            };
            let leaving = make(Arc::new(FaultyFs::new(mem.another_process(), policy)));
            let mut attachment = attach(&leaving, Access::Write);
            registered(&mut *attachment, b"leaving");
            if upgrading {
                drop(
                    attachment
                        .upgrade(ReaderSafety::Safe, Waiting::Never)
                        .expect("upgrade"),
                );
            } else {
                drop(attachment);
            }
            assert_eq!(
                *held_while_cleared.lock().expect("held"),
                vec![true],
                "upgrading: {upgrading}"
            );
        }
    }
}

#[test]
fn a_registrant_moves_on_from_an_entry_claimed_for_a_moment() {
    on_every_implementation("coord-reg-claimed", &|implementation, backend| {
        let observer = participant(implementation, backend);
        let claim = hold_registry_lock(&observer, Layout::claim(0));
        let mut attachment = attach(&participant(implementation, backend), Access::Read);
        let token = registered(&mut *attachment, b"moved");
        drop(observer.release_lock(claim));
        assert_eq!(recorded(&observer, 0), None, "{}", backend.name());
        assert_eq!(recorded(&observer, 1), Some(token));
    });
}

#[test]
fn a_registrant_never_reuses_a_live_entry() {
    on_every_implementation("coord-reg-live", &|implementation, backend| {
        let observer = participant(implementation, backend);
        let mut tokens = Vec::new();
        let mut attachments = Vec::new();
        for i in 0..5u8 {
            let mut attachment = attach(&participant(implementation, backend), Access::Read);
            tokens.push(registered(&mut *attachment, &[i]));
            attachments.push(attachment);
        }
        for (index, token) in tokens.iter().enumerate() {
            assert_eq!(
                recorded(&observer, index),
                Some(*token),
                "{}",
                backend.name()
            );
        }
        drop(attachments.remove(2));
        let mut next = attach(&participant(implementation, backend), Access::Read);
        let reused = registered(&mut *next, b"next");
        assert_eq!(recorded(&observer, 2), Some(reused), "the freed entry");
        let listing = list(&observer);
        assert_eq!(listing.live.len(), 5);
        assert!(!live_tokens(&listing).contains(&tokens[2]));
    });
}

#[test]
fn an_owner_that_cannot_claim_its_entry_leaves_it_dead() {
    on_every_implementation("coord-reg-skip-clear", &|implementation, backend| {
        let observer = participant(implementation, backend);
        let mut attachment = attach(&participant(implementation, backend), Access::Read);
        let token = registered(&mut *attachment, b"left");
        let claim = hold_registry_lock(&observer, Layout::claim(0));
        drop(attachment);
        drop(observer.release_lock(claim));
        assert_eq!(
            recorded(&observer, 0),
            Some(token),
            "{}: the record stays",
            backend.name()
        );
        let listing = list(&observer);
        assert!(listing.live.is_empty());
        assert!(!listing.may_be_incomplete);
        let mut next = attach(&participant(implementation, backend), Access::Read);
        let reused = registered(&mut *next, b"next");
        assert_eq!(recorded(&observer, 0), Some(reused));
    });
}

#[test]
fn a_live_entry_whose_record_cannot_be_read_is_unknown() {
    on_every_implementation("coord-reg-garbage", &|implementation, backend| {
        let observer = participant(implementation, backend);
        let mut attachment = attach(&participant(implementation, backend), Access::Read);
        registered(&mut *attachment, b"overwritten");
        for garbage in [&b""[..], b"ACRG", b"not a record at all"] {
            observer
                .vfs()
                .write(&observer.layout().registry_entry(0), garbage)
                .expect("write");
            let listing = list(&observer);
            assert!(listing.live.is_empty(), "{}", backend.name());
            assert_eq!(listing.unknown, 1);
            assert!(listing.may_be_incomplete);
        }
    });
}

#[test]
fn every_entry_held_attaches_unregistered_and_listings_say_so_until_a_restart() {
    on_every_implementation("coord-reg-overflow", &|implementation, backend| {
        let observer = participant(implementation, backend);
        let mut attachments: Vec<_> = (0..REGISTRY_ENTRIES)
            .map(|_| {
                let mut attachment = attach(&participant(implementation, backend), Access::Read);
                registered(&mut *attachment, b"held");
                attachment
            })
            .collect();
        assert!(!list(&observer).may_be_incomplete, "{}", backend.name());
        let mut extra = attach(&participant(implementation, backend), Access::Read);
        assert_eq!(
            extra.register(b"extra").expect("register"),
            Registration::Unregistered
        );
        let listing = list(&observer);
        assert_eq!(listing.live.len(), REGISTRY_ENTRIES);
        assert!(listing.may_be_incomplete);
        attachments.clear();
        drop(extra);
        let listing = list(&observer);
        assert!(listing.live.is_empty());
        assert!(
            listing.may_be_incomplete,
            "{}: only a restart discards the record",
            backend.name()
        );
        let mut later = attach(&participant(implementation, backend), Access::Read);
        registered(&mut *later, b"later");
        assert!(list(&observer).may_be_incomplete);
    });
}

#[test]
fn a_host_restart_discards_the_overflow_record() {
    for implementation in super::implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let make = || participant_on(&implementation, Arc::new(mem.another_process()), &root);
        let observer = make();
        let attachments: Vec<_> = (0..=REGISTRY_ENTRIES)
            .map(|_| {
                let mut attachment = attach(&make(), Access::Read);
                attachment.register(b"held").expect("register");
                attachment
            })
            .collect();
        drop(attachments);
        let layout = observer.layout();
        for path in [layout.registry_overflow(), layout.state_dir(), root.clone()] {
            mem.fsync(&path).expect("sync");
        }
        assert!(list(&observer).may_be_incomplete);
        mem.simulate_crash();
        assert!(
            observer
                .vfs()
                .read(&layout.registry_overflow())
                .is_ok_and(|bytes| !bytes.is_empty()),
            "the record survived the crash"
        );
        let listing = list(&make());
        assert!(!listing.may_be_incomplete, "it names the boot before");
    }
}

#[test]
fn an_upgrades_conversion_removes_its_entry_and_the_upgrade_registers_anew() {
    on_every_implementation("coord-reg-upgrade", &|implementation, backend| {
        let observer = participant(implementation, backend);
        let mut attachment = attach(&participant(implementation, backend), Access::Write);
        let attached = registered(&mut *attachment, b"attached");
        let mut upgrade = attachment
            .upgrade(ReaderSafety::Unsafe, Waiting::Never)
            .expect("upgrade");
        assert!(list(&observer).live.is_empty(), "{}", backend.name());
        let upgraded = match upgrade.register(b"upgraded").expect("register") {
            Registration::Registered(token) => token,
            Registration::Unregistered => panic!("every entry is held"),
        };
        assert_ne!(upgraded, attached);
        let listing = list(&observer);
        assert_eq!(listing.live.len(), 1);
        assert_eq!(listing.live[0].token, upgraded);
        assert_eq!(
            listing.live[0].tiers,
            HeldTiers {
                reader: Some(TierHold::Exclusive),
                writer: Some(TierHold::Exclusive)
            }
        );
        drop(upgrade);
        assert!(list(&observer).live.is_empty());
    });
}

#[test]
fn a_cancelled_upgrade_leaves_the_participant_without_an_entry() {
    on_every_implementation("coord-reg-cancelled", &|implementation, backend| {
        let observer = participant(implementation, backend);
        let mut reader = attach(&participant(implementation, backend), Access::Read);
        let reading = registered(&mut *reader, b"reader");
        let mut upgrader = attach(&participant(implementation, backend), Access::Write);
        registered(&mut *upgrader, b"upgrader");
        assert!(matches!(
            upgrader.upgrade(ReaderSafety::Unsafe, Waiting::Never),
            Err(CoordError::WouldWait)
        ));
        assert_eq!(
            live_tokens(&list(&observer)),
            HashSet::from([reading]),
            "{}",
            backend.name()
        );
    });
}

#[test]
fn a_listing_needs_no_attachment_and_sees_past_an_upgrade_holding_the_gate() {
    on_every_implementation("coord-reg-unattached", &|implementation, backend| {
        let mut upgrade = attach(&participant(implementation, backend), Access::Write)
            .upgrade(ReaderSafety::Unsafe, Waiting::Never)
            .expect("upgrade");
        upgrade.register(b"blocking").expect("register");
        let listing = list(&participant(implementation, backend));
        assert_eq!(listing.live.len(), 1, "{}", backend.name());
        assert_eq!(listing.live[0].payload, b"blocking");
    });
}

#[test]
fn an_abandoned_participant_neither_registers_nor_lists() {
    on_every_implementation("coord-reg-abandoned", &|implementation, backend| {
        let dying = participant(implementation, backend);
        let mut attachment = attach(&dying, Access::Read);
        dying.abandon();
        assert!(matches!(
            attachment.register(b"late"),
            Err(CoordError::Abandoned)
        ));
        assert!(matches!(dying.list(), Err(CoordError::Abandoned)));
        assert!(list(&participant(implementation, backend)).live.is_empty());
    });
}

#[test]
fn simultaneous_listings_see_every_live_entry_or_count_it_unknown() {
    const LISTERS: usize = 3;
    const ROUNDS: usize = 6;
    on_every_implementation("coord-reg-simultaneous", &|implementation, backend| {
        let mut attachments = Vec::new();
        let mut tokens = HashSet::new();
        for i in 0..8u8 {
            let mut attachment = attach(&participant(implementation, backend), Access::Read);
            tokens.insert(registered(&mut *attachment, &[i]));
            attachments.push(attachment);
        }
        let tokens = Arc::new(tokens);
        let listers: Vec<_> = (0..LISTERS)
            .map(|_| {
                let lister = participant(implementation, backend);
                let tokens = Arc::clone(&tokens);
                thread::spawn(move || {
                    for _ in 0..ROUNDS {
                        let listing = lister.list().expect("list");
                        let live = live_tokens(&listing);
                        assert!(live.is_subset(&tokens));
                        assert!(live.len() + listing.unknown >= tokens.len());
                        if listing.unknown == 0 {
                            assert_eq!(live, *tokens);
                            assert!(!listing.may_be_incomplete);
                        } else {
                            assert!(listing.may_be_incomplete);
                        }
                    }
                })
            })
            .collect();
        for lister in listers {
            lister
                .join()
                .unwrap_or_else(|_| panic!("{}: a lister", backend.name()));
        }
    });
}

#[test]
fn a_dead_participant_is_never_listed_live_while_entries_churn() {
    const CHURNERS: usize = 3;
    const ROUNDS: usize = 12;
    on_every_implementation("coord-reg-churn", &|implementation, backend| {
        let dead = Arc::new(Mutex::new(HashSet::new()));
        let churning = Arc::new(AtomicBool::new(true));
        let churners: Vec<_> = (0..CHURNERS)
            .map(|c| {
                let participants: Vec<_> = (0..ROUNDS)
                    .map(|_| participant(implementation, backend))
                    .collect();
                let dead = Arc::clone(&dead);
                thread::spawn(move || {
                    for (round, participant) in participants.into_iter().enumerate() {
                        let mut attachment = participant
                            .attach(Access::Read, Waiting::Wait)
                            .expect("attach");
                        let token = match attachment.register(&[c as u8]) {
                            Ok(Registration::Registered(token)) => Some(token),
                            Ok(Registration::Unregistered) => None,
                            Err(error) => panic!("{error}"),
                        };
                        if round % 2 == 0 {
                            participant.abandon();
                        }
                        drop(attachment);
                        if let Some(token) = token {
                            dead.lock().expect("dead").insert(token);
                        }
                    }
                })
            })
            .collect();
        let lister = participant(implementation, backend);
        let listing = {
            let (dead, churning) = (Arc::clone(&dead), Arc::clone(&churning));
            thread::spawn(move || {
                let mut listings = 0;
                while churning.load(Ordering::Acquire) || listings == 0 {
                    let gone = dead.lock().expect("dead").clone();
                    let listing = lister.list().expect("list");
                    let live = live_tokens(&listing);
                    assert!(
                        live.is_disjoint(&gone),
                        "a token gone before the listing began is listed live"
                    );
                    listings += 1;
                }
            })
        };
        for churner in churners {
            churner
                .join()
                .unwrap_or_else(|_| panic!("{}: a churner", backend.name()));
        }
        churning.store(false, Ordering::Release);
        listing
            .join()
            .unwrap_or_else(|_| panic!("{}: the lister", backend.name()));
        assert!(list(&participant(implementation, backend)).live.is_empty());
    });
}

#[test]
fn the_registry_is_reached_through_the_interface() {
    on_every_implementation("coord-reg-dyn", &|implementation, backend| {
        let coordination: Box<dyn Coordination> = implementation
            .open(backend.process(), &backend.root())
            .expect("open");
        let mut attachment = coordination
            .attach(Access::Read, Waiting::Never)
            .expect("attach");
        let token = registered(&mut *attachment, b"through the interface");
        let listing = coordination.list().expect("list");
        assert_eq!(live_tokens(&listing), HashSet::from([token]));
    });
}
