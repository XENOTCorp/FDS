//! Single-shot, level-triggered readiness with socket-independent request IDs.
use super::{Driver, Event, Interest};
use crate::io_uring_reactor::IoUringReactor;
use std::{
    collections::HashMap,
    io,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
struct Registration {
    fd: i32,
    token: u64,
    interest: Interest,
}

/// io_uring-backed readiness driver (feature `io-uring`). Unlike epoll,
/// single-shot polls are rearmed for level-triggered readiness. Application
/// tokens are separate from kernel request IDs: cancellations and stale
/// completions cannot masquerade as a newly registered token.
pub struct IoUringDriver {
    reactor: IoUringReactor,
    events: Vec<Event>,
    completions: Vec<(u64, io::Result<u32>)>,
    registrations: HashMap<u64, u64>, // application token -> current request
    requests: HashMap<u64, Registration>,
    next_request: u64,
    capacity: usize,
}

impl IoUringDriver {
    pub fn new(entries: u32) -> io::Result<Self> {
        if entries == 0 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let reactor = IoUringReactor::new(entries, 0)?;
        let (capacity, completion_capacity) = reactor.capacities();
        Ok(Self {
            reactor,
            events: Vec::with_capacity(capacity),
            completions: Vec::with_capacity(completion_capacity),
            // Keep occupancy below half capacity so deletion tombstones
            // can be rehashed in place instead of growing during polls.
            registrations: HashMap::with_capacity(capacity * 2),
            requests: HashMap::with_capacity(capacity * 2),
            next_request: 1, // zero belongs exclusively to cancellation CQEs
            capacity,
        })
    }

    fn flags(interest: Interest) -> u32 {
        let readiness = match interest {
            Interest::Readable => libc::POLLIN,
            Interest::Writable => libc::POLLOUT,
            Interest::ReadableWritable => libc::POLLIN | libc::POLLOUT,
        };
        (readiness | libc::POLLERR | libc::POLLHUP | libc::POLLRDHUP) as u32
    }

    fn arm(&mut self, registration: Registration) -> io::Result<u64> {
        let request = self.next_request;
        self.next_request = request
            .checked_add(1)
            .ok_or_else(|| io::Error::other("io_uring request identities exhausted"))?;
        self.reactor
            .submit_poll(registration.fd, Self::flags(registration.interest), request)?;
        self.requests.insert(request, registration);
        self.registrations.insert(registration.token, request);
        Ok(request)
    }

    fn cancel(&mut self, request: u64) -> io::Result<()> {
        // Flush first so cancellation always has an SQ slot available.
        self.reactor.submit_all()?;
        self.reactor.ring_cancel(request)
    }

    fn wait(&self, deadline: Option<Instant>) -> io::Result<bool> {
        let timeout = deadline.map_or(-1, |end| {
            let remaining = end.saturating_duration_since(Instant::now());
            // Round upward: poll's millisecond resolution must not turn a
            // positive sub-millisecond wait into an accidental busy poll.
            remaining
                .as_nanos()
                .div_ceil(1_000_000)
                .min(i32::MAX as u128) as i32
        });
        let mut fd = libc::pollfd {
            fd: self.reactor.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: fd is initialized and only borrowed for this syscall.
        let result = unsafe { libc::poll(&mut fd, 1, timeout) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        if fd.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(io::Error::other("io_uring completion poll failed"));
        }
        Ok(result > 0)
    }
}

impl Driver for IoUringDriver {
    fn register(&mut self, fd: i32, token: u64, interest: Interest) -> io::Result<()> {
        if fd < 0 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        if self.registrations.contains_key(&token) || self.requests.values().any(|r| r.fd == fd) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        if self.requests.len() >= self.capacity {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        self.arm(Registration {
            fd,
            token,
            interest,
        })?;
        Ok(())
    }

    fn modify(&mut self, fd: i32, token: u64, interest: Interest) -> io::Result<()> {
        let request = *self
            .registrations
            .get(&token)
            .ok_or(io::ErrorKind::NotFound)?;
        if self.requests[&request].fd != fd {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        self.cancel(request)?;
        self.requests.remove(&request);
        self.registrations.remove(&token);
        self.arm(Registration {
            fd,
            token,
            interest,
        })?;
        Ok(())
    }

    fn unregister(&mut self, fd: i32) -> io::Result<()> {
        let Some((&request, &registration)) = self.requests.iter().find(|(_, r)| r.fd == fd) else {
            return Ok(());
        };
        self.cancel(request)?;
        self.requests.remove(&request);
        self.registrations.remove(&registration.token);
        Ok(())
    }

    fn poll(&mut self, timeout: Option<Duration>) -> io::Result<usize> {
        self.events.clear();
        let deadline = timeout
            .map(|duration| {
                Instant::now()
                    .checked_add(duration)
                    .ok_or(io::ErrorKind::InvalidInput)
            })
            .transpose()?;
        loop {
            self.reactor.submit_all()?;
            self.wait(deadline)?;
            self.completions.clear();
            let completions = &mut self.completions;
            self.reactor
                .drain(|request, result| completions.push((request, result)));
            for i in 0..self.completions.len() {
                let (request, result) = &self.completions[i];
                let Some(registration) = self.requests.remove(request) else {
                    continue; // cancellation CQE or stale request generation
                };
                self.registrations.remove(&registration.token);
                let flags = result.as_ref().copied().unwrap_or(0);
                self.events.push(Event {
                    token: registration.token,
                    readable: flags & libc::POLLIN as u32 != 0,
                    writable: flags & libc::POLLOUT as u32 != 0,
                    hang_up: flags & (libc::POLLHUP | libc::POLLRDHUP) as u32 != 0,
                    error: result.is_err() || flags & libc::POLLERR as u32 != 0,
                });
                if result.is_ok() {
                    self.arm(registration)?;
                }
            }
            if !self.events.is_empty() || deadline.is_some_and(|end| Instant::now() >= end) {
                return Ok(self.events.len());
            }
            // A batch containing only cancellation acknowledgements is
            // not application readiness. Keep waiting within the deadline.
        }
    }

    fn events(&self) -> &[Event] {
        &self.events
    }
    fn clear_events(&mut self) {
        self.events.clear();
    }
}
