//! Bounded LIFO stack: the content model of the stack theory.
//!
//! Pipeline equations of that theory (thesis ch. 4):
//! `push; pop = id`, and `pop; push = id` on a nonempty stack;
//! `peek; pop = pop`; `peek; peek = peek`.
//! A FIFO ring does **not** satisfy `push; pop = id` on nonempty
//! content (same chapter). Occupancy of this stack may reach `CAP`;
//! the bitmask occupancy bound `CAP − 1` is a FIFO-ring fact, not a
//! stack fact.

use core::mem::MaybeUninit;

/// Bounded last-in first-out stack. Inserts and removes at the same end.
///
/// Not thread-safe: the stack theory is a sequential content model.
/// Capacity need not be a power of two; a full stack holds `CAP` items.
pub struct Stack<T, const CAP: usize> {
    buf: [MaybeUninit<T>; CAP],
    len: usize,
}

impl<T, const CAP: usize> Stack<T, CAP> {
    /// An empty stack.
    pub const fn new() -> Self {
        Stack {
            // SAFETY: MaybeUninit array, no reads before writes.
            buf: unsafe { MaybeUninit::uninit().assume_init() },
            len: 0,
        }
    }

    /// Push one item; returns it back if the stack is full.
    pub fn try_push(&mut self, value: T) -> Result<(), T> {
        if self.len == CAP {
            return Err(value);
        }
        self.buf[self.len].write(value);
        self.len += 1;
        Ok(())
    }

    /// Pop the most recently pushed item, or `None` if empty.
    pub fn try_pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        // SAFETY: slot `len` was written by a matching push and has not
        // been read since; we own it.
        Some(unsafe { self.buf[self.len].assume_init_read() })
    }

    /// The most recently pushed item, without removing it.
    pub fn peek(&self) -> Option<&T> {
        if self.len == 0 {
            return None;
        }
        // SAFETY: slot `len - 1` is occupied.
        Some(unsafe { self.buf[self.len - 1].assume_init_ref() })
    }

    /// Number of items currently stored (`0..=CAP`).
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Capacity (always `CAP`).
    pub const fn capacity(&self) -> usize {
        CAP
    }

    /// Empty check.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Full check: occupancy may equal `CAP`.
    pub const fn is_full(&self) -> bool {
        self.len == CAP
    }
}

impl<T, const CAP: usize> Default for Stack<T, CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const CAP: usize> Drop for Stack<T, CAP> {
    fn drop(&mut self) {
        while self.try_pop().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_pop_is_identity() {
        let mut s = Stack::<u32, 4>::new();
        assert!(s.try_push(7).is_ok());
        assert_eq!(s.try_pop(), Some(7));
        assert!(s.is_empty());
    }

    #[test]
    fn pop_push_is_identity_when_nonempty() {
        let mut s = Stack::<u32, 4>::new();
        assert!(s.try_push(1).is_ok());
        assert!(s.try_push(2).is_ok());
        let v = s.try_pop().unwrap();
        assert_eq!(v, 2);
        assert!(s.try_push(v).is_ok());
        assert_eq!(s.peek().copied(), Some(2));
        assert_eq!(s.len(), 2);
        assert_eq!(s.try_pop(), Some(2));
        assert_eq!(s.try_pop(), Some(1));
    }

    #[test]
    fn peek_equations() {
        let mut s = Stack::<u32, 4>::new();
        assert!(s.try_push(1).is_ok());
        assert!(s.try_push(2).is_ok());
        assert_eq!(s.peek().copied(), s.peek().copied());
        let peeked = s.peek().copied();
        let popped = s.try_pop();
        assert_eq!(peeked, popped);
        assert_eq!(s.try_pop(), Some(1));
    }

    #[test]
    fn occupancy_reaches_cap() {
        let mut s = Stack::<u32, 4>::new();
        for i in 0..4 {
            assert!(s.try_push(i).is_ok());
        }
        assert!(s.is_full());
        assert!(s.try_push(99).is_err());
        assert_eq!(s.len(), 4);
        assert_eq!(s.try_pop(), Some(3));
    }
}
