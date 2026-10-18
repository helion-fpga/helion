//! IEEE 1800 SystemVerilog testbench synchronization & verification primitives:
//! - [`SimMailbox<T>`]: Bounded or unbounded FIFO message queue (`mailbox #(T)`).
//! - [`SimSemaphore`]: Counting resource semaphore (`semaphore`).
//! - [`SimScoreboard<T>`]: Golden reference vs. DUT transaction comparator with mismatch pinpointing.

use std::collections::VecDeque;

/// IEEE 1800 SystemVerilog `mailbox #(T)` FIFO channel.
///
/// A `bound` of `0` creates an unbounded mailbox; any `bound > 0` enforces a maximum
/// capacity of `bound` messages.
#[derive(Clone, Debug)]
pub struct SimMailbox<T> {
    bound: usize,
    queue: VecDeque<T>,
    total_sent: u64,
    total_received: u64,
}

impl<T> Default for SimMailbox<T> {
    fn default() -> Self {
        Self::new(0)
    }
}

impl<T> SimMailbox<T> {
    /// Create a mailbox with capacity `bound` (`0` = unbounded).
    pub fn new(bound: usize) -> Self {
        Self {
            bound,
            queue: VecDeque::new(),
            total_sent: 0,
            total_received: 0,
        }
    }

    /// Create an unbounded mailbox (`bound == 0`).
    pub fn unbounded() -> Self {
        Self::new(0)
    }

    /// Configured capacity bound (`0` means unbounded).
    pub fn bound(&self) -> usize {
        self.bound
    }

    /// Number of messages currently in the mailbox (SystemVerilog `num()`).
    pub fn num(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.bound > 0 && self.queue.len() >= self.bound
    }

    /// Non-blocking put (SystemVerilog `try_put(msg)`): returns `true` if enqueued,
    /// or `false` if the bounded mailbox is full.
    pub fn try_put(&mut self, msg: T) -> bool {
        if self.is_full() {
            return false;
        }
        self.queue.push_back(msg);
        self.total_sent += 1;
        true
    }

    /// Enqueue a message, returning `Err` if bounded and full.
    pub fn put(&mut self, msg: T) -> Result<(), String> {
        if self.is_full() {
            return Err(format!(
                "mailbox put: bounded mailbox (bound={}) is full",
                self.bound
            ));
        }
        self.queue.push_back(msg);
        self.total_sent += 1;
        Ok(())
    }

    /// Non-blocking get (SystemVerilog `try_get(ref msg)`): pops and returns the oldest message.
    pub fn try_get(&mut self) -> Option<T> {
        let msg = self.queue.pop_front()?;
        self.total_received += 1;
        Some(msg)
    }

    /// Pop the oldest message, returning `Err` if the mailbox is empty.
    pub fn get(&mut self) -> Result<T, String> {
        self.try_get()
            .ok_or_else(|| "mailbox get: mailbox is empty".to_string())
    }

    /// Non-blocking peek at the oldest message without removing it (SystemVerilog `try_peek`).
    pub fn try_peek(&self) -> Option<&T> {
        self.queue.front()
    }

    /// Peek at the oldest message without removing it, returning `Err` if empty.
    pub fn peek(&self) -> Result<&T, String> {
        self.try_peek()
            .ok_or_else(|| "mailbox peek: mailbox is empty".to_string())
    }

    pub fn total_sent(&self) -> u64 {
        self.total_sent
    }

    pub fn total_received(&self) -> u64 {
        self.total_received
    }
}

/// IEEE 1800 SystemVerilog counting `semaphore`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SimSemaphore {
    keys: u64,
    acquisitions: u64,
    releases: u64,
}

impl SimSemaphore {
    /// Create a semaphore initialized with `initial_keys` keys (SystemVerilog `new(keyCount)`).
    pub fn new(initial_keys: u64) -> Self {
        Self {
            keys: initial_keys,
            acquisitions: 0,
            releases: 0,
        }
    }

    /// Number of currently available keys.
    pub fn available_keys(&self) -> u64 {
        self.keys
    }

    /// Non-blocking key acquisition (SystemVerilog `try_get(keyCount)`).
    pub fn try_get(&mut self, count: u64) -> bool {
        if self.keys >= count {
            self.keys -= count;
            self.acquisitions += 1;
            true
        } else {
            false
        }
    }

    /// Acquire `count` keys or return a descriptive error if insufficient keys are available.
    pub fn get(&mut self, count: u64) -> Result<(), String> {
        if self.try_get(count) {
            Ok(())
        } else {
            Err(format!(
                "semaphore get: requested {count} keys, only {} available",
                self.keys
            ))
        }
    }

    /// Return `count` keys to the semaphore (SystemVerilog `put(keyCount)`).
    pub fn put(&mut self, count: u64) {
        self.keys = self.keys.saturating_add(count);
        self.releases += 1;
    }

    pub fn acquisitions(&self) -> u64 {
        self.acquisitions
    }

    pub fn releases(&self) -> u64 {
        self.releases
    }
}

/// One recorded transaction mismatch in a [`SimScoreboard<T>`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreboardMismatch<T> {
    pub index: u64,
    pub time_ps: u64,
    pub cycle: u64,
    pub expected: T,
    pub actual: T,
}

/// Self-checking verification scoreboard comparing expected golden transactions against
/// actual observed DUT transactions in FIFO order.
#[derive(Clone, Debug)]
pub struct SimScoreboard<T> {
    pub name: String,
    expected_q: VecDeque<T>,
    actual_q: VecDeque<(u64, u64, T)>,
    compared: u64,
    matches: u64,
    mismatches: Vec<ScoreboardMismatch<T>>,
}

impl<T: PartialEq + Clone + std::fmt::Debug> Default for SimScoreboard<T> {
    fn default() -> Self {
        Self::new("scoreboard")
    }
}

impl<T: PartialEq + Clone + std::fmt::Debug> SimScoreboard<T> {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            expected_q: VecDeque::new(),
            actual_q: VecDeque::new(),
            compared: 0,
            matches: 0,
            mismatches: Vec::new(),
        }
    }

    /// Enqueue an expected transaction from the golden reference model.
    pub fn push_expected(&mut self, item: T) {
        self.expected_q.push_back(item);
        self.drain_comparisons();
    }

    /// Enqueue an actual transaction observed from the DUT at `(time_ps, cycle)`.
    pub fn push_actual(&mut self, time_ps: u64, cycle: u64, item: T) {
        self.actual_q.push_back((time_ps, cycle, item));
        self.drain_comparisons();
    }

    fn drain_comparisons(&mut self) {
        while !self.expected_q.is_empty() && !self.actual_q.is_empty() {
            let exp = self.expected_q.pop_front().unwrap();
            let (time_ps, cycle, act) = self.actual_q.pop_front().unwrap();
            let idx = self.compared;
            self.compared += 1;
            if exp == act {
                self.matches += 1;
            } else {
                self.mismatches.push(ScoreboardMismatch {
                    index: idx,
                    time_ps,
                    cycle,
                    expected: exp,
                    actual: act,
                });
            }
        }
    }

    /// Total pairs compared so far.
    pub fn compared(&self) -> u64 {
        self.compared
    }

    /// Number of matching transactions.
    pub fn matches(&self) -> u64 {
        self.matches
    }

    /// All recorded transaction mismatches.
    pub fn mismatches(&self) -> &[ScoreboardMismatch<T>] {
        &self.mismatches
    }

    /// Number of expected items still awaiting a matching DUT output.
    pub fn pending_expected(&self) -> usize {
        self.expected_q.len()
    }

    /// Number of actual DUT items still awaiting an expected reference item.
    pub fn pending_actual(&self) -> usize {
        self.actual_q.len()
    }

    /// True if all compared transactions matched and no orphan items remain in either queue.
    pub fn is_clean(&self) -> bool {
        self.mismatches.is_empty() && self.expected_q.is_empty() && self.actual_q.is_empty()
    }

    /// Single-line machine-readable summary for logs and Tcl testbench reports.
    pub fn summary_line(&self) -> String {
        let verdict = if self.is_clean() { "PASS" } else { "FAIL" };
        format!(
            "scoreboard {}: {verdict} compared={} matches={} mismatches={} pending_exp={} pending_act={}",
            self.name,
            self.compared,
            self.matches,
            self.mismatches.len(),
            self.expected_q.len(),
            self.actual_q.len()
        )
    }
}
