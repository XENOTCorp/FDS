//! Public readiness drivers must reuse their storage on every poll.
use fds::api::{Driver, EpollDriver, Interest};
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{Read, Write};
use std::os::{fd::AsRawFd, unix::net::UnixStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct CountingAllocator;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

// One test in this executable avoids allocator noise from other tests.
#[test]
fn empty_and_ready_polls_allocate_nothing() {
    for capacity in [0, 1, 64] {
        let mut driver = EpollDriver::new(capacity).unwrap();
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        receiver.set_nonblocking(true).unwrap();
        driver
            .register(receiver.as_raw_fd(), 42, Interest::Readable)
            .unwrap();
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        for _ in 0..100 {
            assert_eq!(driver.poll(Some(Duration::ZERO)).unwrap(), 0);
            sender.write_all(b"x").unwrap();
            assert_eq!(driver.poll(Some(Duration::from_secs(2))).unwrap(), 1);
            assert_eq!(driver.events()[0].token, 42);
            assert!(driver.events()[0].readable);
            let mut byte = [0];
            receiver.read_exact(&mut byte).unwrap();
            assert_eq!(byte, *b"x");
            driver.clear_events();
        }
        assert_eq!(
            ALLOCATIONS.load(Ordering::Relaxed),
            before,
            "poll allocated"
        );
    }
    #[cfg(feature = "io-uring")]
    {
        let mut driver = fds::api::IoUringDriver::new(64).unwrap();
        let mut sockets: Vec<_> = (0..64).map(|_| UnixStream::pair().unwrap()).collect();
        for (token, (_, receiver)) in sockets.iter().enumerate() {
            receiver.set_nonblocking(true).unwrap();
            driver
                .register(receiver.as_raw_fd(), token as u64, Interest::Readable)
                .unwrap();
        }
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        for _ in 0..100 {
            assert_eq!(driver.poll(Some(Duration::ZERO)).unwrap(), 0);
            for (sender, _) in &mut sockets {
                sender.write_all(b"x").unwrap();
            }
            let mut received = 0;
            while received < sockets.len() {
                assert!(driver.poll(Some(Duration::from_secs(2))).unwrap() > 0);
                for event in driver.events() {
                    assert!(event.readable);
                    let mut byte = [0];
                    sockets[event.token as usize]
                        .1
                        .read_exact(&mut byte)
                        .unwrap();
                    received += 1;
                }
            }
        }
        assert_eq!(
            ALLOCATIONS.load(Ordering::Relaxed),
            before,
            "io_uring poll allocated"
        );
    }
}
