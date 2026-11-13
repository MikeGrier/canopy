@{
    Timeline = 'T10'
    Base = 'Creation-Control-ReservationInTwoWrites'
    Note = 'A is process 1 and B process 2.'
    Steps = @(
        @{ Id = 'LOOK1'; Actions = @('Look(1)') }
        @{ Id = 'LOCK1'; Actions = @('Lock(1)') }
        @{ Id = 'PUBP'; Actions = @('Publish(1)') }
        @{ Id = 'LOOK2'; Actions = @('Look(2)') }
    )
}
