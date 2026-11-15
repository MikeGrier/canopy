// Copyright (c) Michael Grier.

use std::io;

#[cfg(windows)]
pub fn now() -> io::Result<u64> {
    use windows_sys::Win32::System::Performance::{
        QueryPerformanceCounter, QueryPerformanceFrequency,
    };
    let (mut count, mut frequency) = (0, 0);
    unsafe {
        if QueryPerformanceCounter(&mut count) == 0
            || QueryPerformanceFrequency(&mut frequency) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    if count < 0 || frequency <= 0 {
        return Err(io::Error::other("invalid performance clock"));
    }
    Ok(((count as u128 * 1_000_000_000) / frequency as u128) as u64)
}

#[cfg(unix)]
pub fn now() -> io::Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if time.tv_sec < 0 || !(0..1_000_000_000).contains(&time.tv_nsec) {
        return Err(io::Error::other("invalid monotonic clock"));
    }
    Ok(time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64)
}
