@{
    Timeline = 'T11'
    Base = 'Creation-Control-PublishReplacesRunsTwoCreations'
    Note = 'A is process 1, of implementation 1, and B process 2, of implementation 2.'
    Steps = @(
        @{ Id = 'LOOK1'; Actions = @('Look(1)') }
        @{ Id = 'LOOK2'; Actions = @('Look(2)') }
        @{ Id = 'LOCK1'; Actions = @('Lock(2)') }
        @{ Id = 'PUB1'; Actions = @('Publish(2)') }
        @{ Id = 'LOCK2'; Actions = @('Lock(1)') }
        @{ Id = 'PUB2'; Actions = @('Publish(1)') }
    )
}
