//! Approximate summaries of an unbounded stream of values.
//!
//! A profile has to describe a column of any size in a fixed amount of space,
//! which rules out an exact map of every distinct value. Each sketch here buys
//! that bound with some accuracy, and needs to know nothing about the data in
//! advance: [`SpaceSaving`] for the frequent values, [`HyperLogLog`] for the
//! distinct count once the tracker is full, and [`BottomK`] for a sample of the
//! distinct values to draw examples from. All three share one hash per value.
//!
//! A histogram is a bounded summary too, but an exact one, and it can't count a
//! single value until it knows the range to divide — so `profile::BinCounts`
//! sits beside the code that establishes that range instead of here.

use std::collections::BTreeSet;
use std::collections::BinaryHeap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use hashbrown::{HashMap, HashSet};

use crate::value::Value;

/// Hash a value for the sketches. `DefaultHasher` is fixed-keyed, so a profile
/// of the same data is reproducible — unlike the randomly-seeded hashers behind
/// `HashMap`, which would make sampled examples differ run to run.
pub(crate) fn hash_value(value: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Registers are indexed by the top [`HyperLogLog::PRECISION`] bits of a hash.
const PRECISION: u32 = 14;
const REGISTERS: usize = 1 << PRECISION;

/// Estimates how many distinct values a stream held, in fixed space
/// (16 KB here, for roughly 1% relative error).
///
/// Each hash picks a register and contributes the position of the first 1 bit
/// in the rest of its bits; a register keeps the largest such position it has
/// seen. Rare long runs of leading zeros imply many distinct values, and
/// averaging across registers turns that into an estimate.
pub(crate) struct HyperLogLog {
    registers: Vec<u8>,
}

impl HyperLogLog {
    pub(crate) fn new() -> Self {
        HyperLogLog {
            registers: vec![0; REGISTERS],
        }
    }

    pub(crate) fn insert(&mut self, hash: u64) {
        let index = (hash >> (64 - PRECISION)) as usize;
        // The sentinel bit bounds the run length when the remaining bits are all
        // zero, which would otherwise report a rank past the end of the hash.
        let rest = (hash << PRECISION) | (1 << (PRECISION - 1));
        let rank = rest.leading_zeros() as u8 + 1;
        self.registers[index] = self.registers[index].max(rank);
    }

    pub(crate) fn estimate(&self) -> usize {
        let m = REGISTERS as f64;
        let harmonic: f64 = self
            .registers
            .iter()
            .map(|&rank| 1.0 / (1u64 << rank) as f64)
            .sum();
        let alpha = 0.7213 / (1.0 + 1.079 / m);
        let estimate = alpha * m * m / harmonic;

        // With few distinct values most registers are still empty, where the
        // estimator above is biased; counting the empty ones is exact enough.
        let empty = self.registers.iter().filter(|&&rank| rank == 0).count();
        if estimate <= 2.5 * m && empty > 0 {
            return (m * (m / empty as f64).ln()).round() as usize;
        }
        estimate.round() as usize
    }
}

/// The most frequent values of a stream, tracked in bounded space.
///
/// Below `capacity` distinct values this is an exact counter. Once full, a new
/// value evicts the least frequent entry and inherits its count, so a heavy
/// hitter that first appears late still climbs into the top-k rather than being
/// dropped. The inherited count is kept as that entry's error bound: its true
/// count lies in `count - error ..= count`.
pub(crate) struct SpaceSaving {
    capacity: usize,
    slots: Vec<Slot>,
    index: HashMap<Value, usize>,
    /// `(count, slot)`, so the eviction candidate is the first entry. Built when
    /// the tracker saturates; while there is room to grow, nothing is evicted
    /// and maintaining the order would be wasted work.
    order: BTreeSet<(usize, usize)>,
    saturated: bool,
}

struct Slot {
    value: Value,
    count: usize,
    error: usize,
}

/// How often one value occurs, as [`SpaceSaving`] counts it.
///
/// `count` is exact while the tracker has room. Once it is full, a value first
/// seen after that point inherits the count of the entry it displaced, so
/// `count` becomes an upper bound and `error` is how much of it was inherited:
/// the true count lies in `count - error ..= count`.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueCount {
    pub value: Value,
    pub count: usize,
    pub error: usize,
}

impl SpaceSaving {
    pub(crate) fn new(capacity: usize) -> Self {
        SpaceSaving {
            capacity,
            slots: Vec::new(),
            index: HashMap::new(),
            order: BTreeSet::new(),
            saturated: false,
        }
    }

    /// Record `count` more occurrences of `value`.
    pub(crate) fn add(&mut self, value: &Value, count: usize) {
        if let Some(&slot) = self.index.get(value) {
            let previous = self.slots[slot].count;
            self.slots[slot].count = previous + count;
            if self.saturated {
                self.order.remove(&(previous, slot));
                self.order.insert((previous + count, slot));
            }
            return;
        }
        if self.slots.len() < self.capacity {
            let slot = self.slots.len();
            self.slots.push(Slot {
                value: value.clone(),
                count,
                error: 0,
            });
            self.index.insert(value.clone(), slot);
            return;
        }
        if !self.saturated {
            self.order = self
                .slots
                .iter()
                .enumerate()
                .map(|(slot, entry)| (entry.count, slot))
                .collect();
            self.saturated = true;
        }
        let (smallest, slot) = *self.order.iter().next().expect("a full tracker has slots");
        self.order.remove(&(smallest, slot));
        self.index.remove(&self.slots[slot].value);
        self.slots[slot] = Slot {
            value: value.clone(),
            count: smallest + count,
            error: smallest,
        };
        self.index.insert(value.clone(), slot);
        self.order.insert((smallest + count, slot));
    }

    /// Distinct values tracked, which is exact only while unsaturated.
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether any value has been evicted, making the counts upper bounds.
    pub(crate) fn is_saturated(&self) -> bool {
        self.saturated
    }

    /// The `k` most frequent values, most frequent first and ties broken by
    /// value so the result is deterministic.
    pub(crate) fn top(&self, k: usize) -> Vec<ValueCount> {
        let mut top: Vec<&Slot> = self.slots.iter().collect();
        top.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
        top.into_iter()
            .take(k)
            .map(|slot| ValueCount {
                value: slot.value.clone(),
                count: slot.count,
                error: slot.error,
            })
            .collect()
    }
}

/// A uniform random sample of a stream's *distinct* values, by keeping the `k`
/// values with the smallest hashes.
///
/// Repeats of a value hash identically and are rejected, so a billion rows of
/// `"US"` make `"US"` no more likely to be sampled than a value occurring once
/// — which is what examples should show.
pub(crate) struct BottomK {
    k: usize,
    /// Ordered by hash, so the root is the sampled value furthest from
    /// qualifying and the first to be displaced.
    heap: BinaryHeap<(u64, Value)>,
    held: HashSet<u64>,
}

impl BottomK {
    pub(crate) fn new(k: usize) -> Self {
        BottomK {
            k,
            heap: BinaryHeap::new(),
            held: HashSet::new(),
        }
    }

    pub(crate) fn insert(&mut self, hash: u64, value: &Value) {
        if self.held.contains(&hash) {
            return;
        }
        if self.heap.len() == self.k {
            let &(largest, _) = self.heap.peek().expect("a full sample is non-empty");
            if hash >= largest {
                return;
            }
            self.heap.pop();
            self.held.remove(&largest);
        }
        self.held.insert(hash);
        self.heap.push((hash, value.clone()));
    }

    /// Up to `n` values spread evenly along the sorted sample — the baseline the
    /// spec recommends for a column's `examples` (see `site/spec.md`).
    pub(crate) fn examples(&self, n: usize) -> Vec<Value> {
        let mut sample: Vec<Value> = self.heap.iter().map(|(_, value)| value.clone()).collect();
        sample.sort();
        if sample.len() <= n {
            return sample;
        }
        if n <= 1 {
            return sample.into_iter().take(n).collect();
        }
        let last = (sample.len() - 1) as f64;
        (0..n)
            .map(|i| {
                let at = (i as f64 * last / (n - 1) as f64).round() as usize;
                sample[at].clone()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{BottomK, HyperLogLog, SpaceSaving, ValueCount, hash_value};
    use crate::value::Value;

    fn int(value: i64) -> Value {
        Value::Int(value)
    }

    fn counted(value: i64, count: usize, error: usize) -> ValueCount {
        ValueCount {
            value: int(value),
            count,
            error,
        }
    }

    #[test]
    fn hyperloglog_is_close_at_both_ends_of_its_range() {
        for distinct in [0usize, 1, 10, 1_000] {
            let mut sketch = HyperLogLog::new();
            for value in 0..distinct {
                sketch.insert(hash_value(&int(value as i64)));
            }
            let estimate = sketch.estimate() as f64;
            let error = (estimate - distinct as f64).abs() / (distinct.max(1) as f64);
            assert!(error < 0.05, "{distinct} distinct estimated as {estimate}");
        }

        let mut sketch = HyperLogLog::new();
        for value in 0..100_000i64 {
            // Repeats must not inflate the estimate.
            sketch.insert(hash_value(&int(value)));
            sketch.insert(hash_value(&int(value)));
        }
        let estimate = sketch.estimate() as f64;
        let error = (estimate - 100_000.0).abs() / 100_000.0;
        assert!(error < 0.03, "100k distinct estimated as {estimate}");
    }

    #[test]
    fn space_saving_counts_exactly_until_it_fills() {
        let mut tracker = SpaceSaving::new(10);
        for value in 0..10i64 {
            tracker.add(&int(value), value as usize + 1);
        }
        assert!(!tracker.is_saturated());
        assert_eq!(tracker.len(), 10);
        assert_eq!(tracker.top(2), vec![counted(9, 10, 0), counted(8, 9, 0)]);
    }

    #[test]
    fn a_late_heavy_hitter_displaces_a_rare_value() {
        let mut tracker = SpaceSaving::new(4);
        for value in 0..4i64 {
            tracker.add(&int(value), 5);
        }
        // Arrives only after the tracker is full, but dominates from there on.
        for _ in 0..100 {
            tracker.add(&int(99), 1);
        }
        assert!(tracker.is_saturated());
        assert_eq!(tracker.len(), 4);
        let top = tracker.top(1);
        let hitter = &top[0];
        assert_eq!(hitter.value, int(99));
        // The evicted entry's count is inherited, so the true 100 is bracketed.
        assert!(
            hitter.count - hitter.error <= 100 && 100 <= hitter.count,
            "{} ± {}",
            hitter.count,
            hitter.error
        );
    }

    #[test]
    fn bottom_k_samples_distinct_values_not_occurrences() {
        let mut sample = BottomK::new(8);
        for _ in 0..1_000 {
            sample.insert(hash_value(&int(0)), &int(0));
        }
        for value in 1..100i64 {
            sample.insert(hash_value(&int(value)), &int(value));
        }
        let examples = sample.examples(5);
        assert_eq!(examples.len(), 5);
        let mut sorted = examples.clone();
        sorted.sort();
        assert_eq!(examples, sorted, "examples come out in value order");
        assert!(
            examples.iter().filter(|value| **value == int(0)).count() <= 1,
            "a repeated value is sampled at most once"
        );
    }

    #[test]
    fn examples_spread_evenly_along_the_sorted_sample() {
        let mut sample = BottomK::new(128);
        for value in 1..=101i64 {
            sample.insert(hash_value(&int(value)), &int(value));
        }
        assert_eq!(
            sample.examples(5),
            vec![int(1), int(26), int(51), int(76), int(101)]
        );
        let mut few = BottomK::new(128);
        for value in 1..=3i64 {
            few.insert(hash_value(&int(value)), &int(value));
        }
        assert_eq!(few.examples(5), vec![int(1), int(2), int(3)]);
    }
}
