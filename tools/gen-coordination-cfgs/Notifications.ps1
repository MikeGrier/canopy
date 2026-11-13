# Copyright (c) Michael Grier.
# The notification configurations of tools/gen-coordination-cfgs.ps1: a waiter samples what it waits
# for, sleeps registered while it is absent, and is woken to sample again by a notification
# (Notifications), with a control in which a notification is lost and one in which a waiter is woken
# with nothing changed.

$plain = Plain 2
$safeReader = Upgrade @('safeUpgrader', 'reader') @(2, 1)
$notifyInvariants = $allInv + @('ReadyMatchesTheWaits')

# ---- notifications: every invariant and property holds ----
Write-Cfg 'Coordination-Notify.cfg' `
    (Header @('Notifications with transactions, two writers, one of which may die, a host crash, a read',
        'and a discovery: every invariant and property holds.')) `
    (Cfg $plain 2 1 1 1 'none' $true $false $notifyInvariants $allProp 0 0 0 $true 1 0 $false $false 'Spec' $false $true)
Write-Cfg 'Coordination-NotifyAttachment.cfg' `
    (Header @('Notifications with an upgrader draining an operation of its handle and a reader, one of',
        'which may die: every invariant and property holds.')) `
    (Cfg $safeReader 0 1 0 0 'none' $true $false $notifyInvariants $allProp 1 0 0 $false 0 0 $false $false 'Spec' $false $true)
Write-Cfg 'Coordination-NoDeadlock-Notify.cfg' `
    (Header @('No deadlock with notifications, transactions and two writers, neither of which gives up or',
        'dies: every invariant and property holds, and no state but every process ended has no',
        'successor.')) `
    (Cfg $plain 2 0 0 0 'none' $false $true $notifyInvariants $allProp 0 0 0 $true 0 0 $false $false 'Spec' $false $true)
Write-Cfg 'Coordination-HoldsEnd-Notify.cfg' `
    (Header @('Under the fairness of each process''s own steps and of a notification, with an upgrader',
        'draining an operation of its handle and a reader, one of which may die, every drain,',
        'admission and upgrade holding its tiers ends: every invariant and property holds.')) `
    (Cfg $safeReader 0 1 0 0 'none' $true $false @() @('ADrainEnds', 'AnAdmissionCompletes', 'AnUpgradeHoldingItsTiersCompletes') 1 0 0 $false 0 0 $false $false 'HoldSpec' $false $true)
Write-Cfg 'Coordination-NotificationSpurious.cfg' `
    (Header @('A waiter may be woken with nothing changed, with transactions and two writers, one of which',
        'may die: every invariant and property holds.')) `
    (Cfg $plain 2 1 0 0 'notification-spurious' $true $false $notifyInvariants $allProp 0 0 0 $true 0 0 $false $false 'Spec' $false $true)
Write-Cfg 'Coordination-HoldsEnd-NotificationSpurious.cfg' `
    (Header @('Under the fairness of each process''s own steps and of a notification, a waiter may be',
        'woken with nothing changed, with an upgrader draining an operation of its handle and a',
        'reader: every drain, admission and upgrade holding its tiers ends: every invariant and',
        'property holds.')) `
    (Cfg $safeReader 0 0 0 0 'notification-spurious' $true $false @() @('ADrainEnds', 'AnAdmissionCompletes', 'AnUpgradeHoldingItsTiersCompletes') 1 0 0 $false 0 0 $false $false 'HoldSpec' $false $true)
Write-Cfg 'Coordination-Witness-AWaiterIsNotified.cfg' `
    (Header @('Witness: a waiter that registered and slept is woken by a change to what it waits for.',
        'TLC must report Witness_AWaiterIsNotified violated, which shows it is reachable.')) `
    (Cfg $plain 2 0 0 0 'none' $true $false @() @('Witness_AWaiterIsNotified') 0 0 0 $false 0 0 $false $false 'Spec' $false $true)

# ---- a notification lost ----
Write-Cfg 'Coordination-Control-HoldsEnd-NotificationLost.cfg' `
    (Header @('Negative control: a draining upgrader''s notification is lost, so it sleeps on after its',
        'clone''s operation has ended. TLC must report ADrainEnds violated.')) `
    (Cfg $safeReader 0 0 0 0 'notification-lost' $true $false @() @('ADrainEnds') 1 0 0 $false 0 0 $false $false 'HoldSpec' $false $true)
Write-Cfg 'Coordination-Control-NotificationLost.cfg' `
    (Header @('Negative control: a writer waiting for exclusion loses its notification, so it sleeps on',
        'once the lock is free. TLC must report a deadlock.')) `
    (Cfg $plain 2 0 0 0 'notification-lost' $false $true @() @() 0 0 0 $false 0 0 $false $false 'Spec' $false $true)
