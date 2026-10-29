//! Background decode of upcoming forcing epochs — the [`PrefetchGroup`] of
//! `spec/prefetch.md` §3.
//!
//! A group holds several DISCRETE [`Provider`]s ("members"), each keyed by a
//! string, and the sorted union of their refresh times ([`boundaries`]). Epoch
//! `k` is `[B[k], B[k+1])`, and what epoch `k` IS, per member, is
//! `member.sample_at(B[k])`. The caller declares the order in which it will take
//! epochs ([`Order`]: forward, reverse — the adjoint's backward sweep — or an
//! explicit revolve schedule), and worker threads decode ahead of it under a
//! slot count (`depth`) and a byte budget (`max_bytes`) while the solver
//! integrates the epoch it already has.
//!
//! # The gate
//!
//! `take(k)[key].data` is BITWISE what `members[key].sample_at(B[k])` returns,
//! which is what `refresh(B[k])` returns (variables, and the `coords()` it
//! leaves), for every order, depth, worker count, budget and decode timing
//! (§3.5). Scheduling changes WHEN a result is ready, never WHAT it is: workers
//! finish in any order, results are keyed by (member, cadence anchor), and only
//! [`take`](PrefetchGroup::take) delivers them, in the declared order. This is
//! not a new decode path — a worker runs the Provider's own sampler
//! ([`Provider::sample_at`]'s), fed from a decoded-file cache instead of a
//! fresh read.
//!
//! # Threads, and what is shared
//!
//! Workers are plain `std::thread`s; no async runtime is needed. The pure-Rust
//! NetCDF reader takes no lock, so members — and several epochs of one member —
//! decode in parallel. Everything the consumer calls (`start`, `take`, `ready`,
//! `cancel`) is meant for ONE thread, which `&mut self` enforces.
//!
//! - **Samples** are deduplicated by (member, cadence anchor): a member whose
//!   cadence is coarser than the union (a 3-hourly field beside hourly ones)
//!   repeats across epochs, and is decoded once per anchor and handed to every
//!   epoch that shares it as the same [`Arc`] — which is why
//!   [`Sample::data`] is read-only.
//! - **Decoded files** are reused per member exactly as the Provider's own
//!   [`refresh`](Provider::refresh) reuses them: at most two whole decoded files
//!   per member (the Provider's LRU size), shared across workers, and a file one
//!   worker is decoding is awaited by the others rather than decoded twice
//!   (§3.6). This sits outside `max_bytes`.
//! - **Disk warm-up** (`fetch_ahead`) runs on one extra thread of its own, so a
//!   download for an epoch far ahead never occupies a decode worker that the
//!   next `take` is waiting on.
//!
//! [`boundaries`]: PrefetchGroup::boundaries

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use time::OffsetDateTime;

use crate::error::{Error, Result};
use crate::format::{NativeDataset, Selection};
use crate::provider::{FileSource, Provider, WindowPlan};

/// The order in which epochs will be taken (`spec/prefetch.md` §3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Order {
    /// `0, 1, …, n-1`.
    Forward,
    /// `n-1, …, 0` — the adjoint's backward sweep.
    Reverse,
    /// An explicit list of 0-based epochs — a revolve schedule. Repeats are
    /// legal; an empty list or an out-of-range epoch is an error at `start`.
    Explicit(Vec<usize>),
}

/// One member's part of an epoch (`spec/prefetch.md` §3.4).
#[derive(Debug)]
pub struct Sample<T> {
    /// The member's `sample_at(B[k])` — or, with `windows`, its
    /// `sample_windows_at(B[k], windows)`; with `transform`, `transform(key, …)`
    /// of that. Shared, and **read-only**: the same object may be handed to
    /// several epochs (a repeated anchor), so a caller that mutates copies first.
    pub data: Arc<T>,
    /// The member's cadence anchor for `B[k]`, in Unix seconds (as
    /// [`Provider::refresh_times`]).
    pub anchor: f64,
    /// Whether `anchor` differs from this member's anchor at the previous `take`
    /// of the current order — always `true` on the first `take` after `start`.
    /// A caller copies only the changed samples into its forcing buffers.
    pub changed: bool,
}

impl<T> Clone for Sample<T> {
    fn clone(&self) -> Self {
        Sample {
            data: self.data.clone(),
            anchor: self.anchor,
            changed: self.changed,
        }
    }
}

/// What [`PrefetchGroup::take`] returns: one [`Sample`] per member key.
#[derive(Debug)]
pub struct EpochSample<T> {
    epoch: usize,
    samples: BTreeMap<String, Sample<T>>,
}

impl<T> Clone for EpochSample<T> {
    fn clone(&self) -> Self {
        EpochSample {
            epoch: self.epoch,
            samples: self.samples.clone(),
        }
    }
}

impl<T> EpochSample<T> {
    /// The epoch (0-based index into [`PrefetchGroup::boundaries`]).
    pub fn epoch(&self) -> usize {
        self.epoch
    }

    /// The sample of member `key`.
    pub fn get(&self, key: &str) -> Option<&Sample<T>> {
        self.samples.get(key)
    }

    /// Every member's sample, in byte-wise key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Sample<T>)> {
        self.samples.iter().map(|(k, s)| (k.as_str(), s))
    }

    /// The number of members.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// True for a group with no members (which construction refuses, so never).
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The samples by member key.
    pub fn into_samples(self) -> BTreeMap<String, Sample<T>> {
        self.samples
    }
}

/// Cumulative statistics, from construction (`spec/prefetch.md` §3.9); `start`
/// does not reset them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PrefetchStats {
    /// Seconds [`take`](PrefetchGroup::take) spent blocked waiting for a slot.
    pub waited_s: f64,
    /// Worker seconds spent producing samples (`sample_at` /
    /// `sample_windows_at` / `transform`).
    pub decode_s: f64,
    /// The largest number of decoded slot bytes (§3.6) held at once.
    pub peak_bytes: u64,
    /// Slots whose samples were produced (decoded or reused).
    pub epochs_decoded: u64,
    /// Member samples decoded.
    pub samples_decoded: u64,
    /// Member samples served from one already held or in flight (a repeated
    /// anchor).
    pub samples_reused: u64,
    /// Slots admitted although larger than `max_bytes` (the head slot always is).
    pub oversize_epochs: u64,
    /// Whether decoding ran beside the consumer. Always `true` in Rust: every
    /// decode runs on a worker thread, and there is no in-`take` fallback.
    pub overlap: bool,
}

/// How long a test wants each decode to take: `(member key, anchor seconds)`.
type LatencyFn = Arc<dyn Fn(&str, f64) -> Duration + Send + Sync>;
/// A member's decoded data turned into what its [`Sample`] carries.
type TransformFn<D, T> = Arc<dyn Fn(&str, D) -> T + Send + Sync>;
/// One member sample, produced by a worker: the data and its §3.6 bytes.
type ProduceFn<T> = Arc<dyn Fn(&Member, OffsetDateTime) -> Result<(T, u64)> + Send + Sync>;

/// Builds a [`PrefetchGroup`]. `D` is what a member decodes to
/// ([`NativeDataset`], or `Vec<NativeDataset>` with [`windows`](Self::windows));
/// `T` is what a [`Sample`] carries (`D`, or a [`transform`](Self::transform)'s
/// output).
pub struct PrefetchBuilder<D, T> {
    members: Vec<(String, Arc<Provider>)>,
    depth: usize,
    max_bytes: Option<u64>,
    fetch_ahead: Option<usize>,
    workers: usize,
    windows: Option<Vec<Selection>>,
    decode: fn(&Member, OffsetDateTime) -> Result<D>,
    bytes: fn(&D) -> u64,
    transform: TransformFn<D, T>,
    latency: Option<LatencyFn>,
}

impl PrefetchGroup<NativeDataset> {
    /// Start building a group: `depth` 2, no byte budget, `fetch_ahead` =
    /// `depth`, one worker, member data = the member's [`NativeDataset`].
    pub fn builder() -> PrefetchBuilder<NativeDataset, NativeDataset> {
        PrefetchBuilder {
            members: Vec::new(),
            depth: 2,
            max_bytes: None,
            fetch_ahead: None,
            workers: 1,
            windows: None,
            decode: decode_sample,
            bytes: NativeDataset::budget_bytes,
            transform: Arc::new(|_: &str, d: NativeDataset| d),
            latency: None,
        }
    }
}

impl<D, T> PrefetchBuilder<D, T> {
    /// Add member `key`. It must be DISCRETE with an enumerable schedule
    /// (non-empty [`Provider::refresh_times`]); a CONST source is materialized
    /// once, outside the group. A window bound with
    /// [`Provider::with_window`] travels with the provider.
    pub fn member(mut self, key: impl Into<String>, provider: Arc<Provider>) -> Self {
        self.members.push((key.into(), provider));
        self
    }

    /// Epoch slots held ahead of the consumer, decoded or decoding, counting the
    /// one `take` returns next (default 2, at least 1).
    pub fn depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }

    /// Bound the decoded bytes of the held slots (§3.6). The head slot is always
    /// admitted, alone if it alone exceeds the budget.
    pub fn max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = Some(max_bytes);
        self
    }

    /// Warm the disk cache for this many slots past the decode window (default
    /// `depth`; `0` disables). Downloads only, never decodes; best-effort.
    pub fn fetch_ahead(mut self, slots: usize) -> Self {
        self.fetch_ahead = Some(slots);
        self
    }

    /// Decode worker threads (default 1, at least 1).
    pub fn workers(mut self, workers: usize) -> Self {
        self.workers = workers;
        self
    }

    /// Test hook: sleep `f(key, anchor_seconds)` on the worker before each
    /// member decode, so a test can randomize completion order (the scenario
    /// `prefetch-equivalence`) or make decode slower than synthetic compute.
    /// Counted in `decode_s`. Not part of the contract.
    #[doc(hidden)]
    pub fn decode_latency<F>(mut self, f: F) -> Self
    where
        F: Fn(&str, f64) -> Duration + Send + Sync + 'static,
    {
        self.latency = Some(Arc::new(f));
        self
    }
}

impl PrefetchBuilder<NativeDataset, NativeDataset> {
    /// Decode `windows` per member in one pass: each member's data becomes
    /// [`Provider::sample_windows_at`]`(B[k], windows)`, one dataset per window
    /// (`spec/prefetch.md` §2.4), its decoded files read once under the windows'
    /// bounding union. Every window must satisfy [`Provider::with_window`]'s
    /// preconditions for every member, checked at [`build`](Self::build).
    pub fn windows<W>(
        self,
        windows: impl IntoIterator<Item = W>,
    ) -> PrefetchBuilder<Vec<NativeDataset>, Vec<NativeDataset>>
    where
        W: Into<Selection>,
    {
        PrefetchBuilder {
            members: self.members,
            depth: self.depth,
            max_bytes: self.max_bytes,
            fetch_ahead: self.fetch_ahead,
            workers: self.workers,
            windows: Some(windows.into_iter().map(Into::into).collect()),
            decode: decode_windows,
            bytes: |ds: &Vec<NativeDataset>| ds.iter().map(NativeDataset::budget_bytes).sum(),
            transform: Arc::new(|_: &str, d: Vec<NativeDataset>| d),
            latency: self.latency,
        }
    }
}

impl<D: 'static> PrefetchBuilder<D, D> {
    /// Apply `f(key, data)` on the worker, so a [`Sample`] carries `U` — a
    /// device-ready layout, a unit conversion, anything that should overlap the
    /// solver too. Its time counts in `decode_s`; the byte budget counts the
    /// data BEFORE `f`. A panic in `f` is stored and raised by `take` like a
    /// decode error.
    pub fn transform<U, F>(self, f: F) -> PrefetchBuilder<D, U>
    where
        F: Fn(&str, D) -> U + Send + Sync + 'static,
    {
        PrefetchBuilder {
            members: self.members,
            depth: self.depth,
            max_bytes: self.max_bytes,
            fetch_ahead: self.fetch_ahead,
            workers: self.workers,
            windows: self.windows,
            decode: self.decode,
            bytes: self.bytes,
            transform: Arc::new(f),
            latency: self.latency,
        }
    }
}

impl<D: 'static, T: Send + Sync + 'static> PrefetchBuilder<D, T> {
    /// Validate the members and start the workers. Errors (as
    /// [`Error::Prefetch`], or the window precondition's own error) for: no
    /// members, a repeated key, a CONST or unbounded member, `depth` or
    /// `workers` of 0, or a window a member cannot take.
    pub fn build(self) -> Result<PrefetchGroup<T>> {
        let refuse = |detail: String| Error::Prefetch { detail };
        if self.members.is_empty() {
            return Err(refuse("a PrefetchGroup needs at least one member".into()));
        }
        if self.depth == 0 {
            return Err(refuse("depth must be at least 1 (it counts the slot take returns next)".into()));
        }
        if self.workers == 0 {
            return Err(refuse("workers must be at least 1".into()));
        }
        let mut members = self.members;
        // Byte-wise key order: what EpochSample iterates in, and what decides
        // which of several failing members' errors `take` raises (§3.7).
        members.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(w) = members.windows(2).find(|w| w[0].0 == w[1].0) {
            return Err(refuse(format!("member key {:?} is given twice", w[0].0)));
        }

        let mut boundaries: Vec<f64> = Vec::new();
        let mut built = Vec::with_capacity(members.len());
        for (key, provider) in members {
            if !provider.is_discrete() {
                return Err(refuse(format!(
                    "member {key:?} is CONST: it has no epochs; materialize it once, \
                     outside the group"
                )));
            }
            let times = provider.refresh_times();
            if times.is_empty() {
                return Err(refuse(format!(
                    "member {key:?} has no enumerable schedule (an open-ended loader \
                     needs a run window or a temporal end)"
                )));
            }
            boundaries.extend(times);
            let plan = match &self.windows {
                Some(ws) => Some(provider.plan_windows(ws)?),
                None => None,
            };
            built.push(Member {
                key,
                provider,
                files: SharedFiles::default(),
                plan,
            });
        }
        boundaries.sort_by(f64::total_cmp);
        boundaries.dedup();

        let mut times = Vec::with_capacity(boundaries.len());
        for &b in &boundaries {
            times.push(OffsetDateTime::from_unix_timestamp(b as i64).map_err(|e| {
                refuse(format!("boundary {b} is not a representable instant: {e}"))
            })?);
        }
        let anchors: Vec<Vec<OffsetDateTime>> = built
            .iter()
            .map(|m| {
                times
                    .iter()
                    .map(|&t| {
                        m.provider
                            .cadence_anchor(t)
                            .expect("a member with a schedule has a positive cadence")
                    })
                    .collect()
            })
            .collect();

        let (decode, bytes, transform) = (self.decode, self.bytes, self.transform);
        let produce: ProduceFn<T> = Arc::new(move |m: &Member, t: OffsetDateTime| {
            let d = decode(m, t)?;
            let b = bytes(&d);
            Ok((transform(&m.key, d), b))
        });

        let n_members = built.len();
        let shared = Arc::new(Shared {
            config: Config {
                members: built,
                boundaries,
                anchors,
                depth: self.depth,
                max_bytes: self.max_bytes,
                fetch_ahead: self.fetch_ahead.unwrap_or(self.depth),
                produce,
                latency: self.latency,
            },
            state: Mutex::new(State::new(n_members)),
            work: Condvar::new(),
            done: Condvar::new(),
        });

        let mut group = PrefetchGroup {
            shared: shared.clone(),
            threads: Vec::new(),
        };
        for i in 0..self.workers {
            let s = shared.clone();
            group.threads.push(spawn(format!("esio-prefetch-{i}"), move || worker(s))?);
        }
        if group.shared.config.fetch_ahead > 0 {
            let s = shared.clone();
            group.threads.push(spawn("esio-prefetch-fetch".into(), move || fetcher(s))?);
        }
        Ok(group)
    }
}

/// Background decode of upcoming epochs across several providers, bitwise
/// identical to synchronous [`Provider::refresh`]. See the module documentation
/// and `spec/prefetch.md` §3.
///
/// ```no_run
/// # fn main() -> earthsciio::Result<()> {
/// # let (p_u, p_v): (earthsciio::Provider, earthsciio::Provider) = unimplemented!();
/// use std::sync::Arc;
/// use earthsciio::{DimWindow, Order, PrefetchGroup};
///
/// let w = DimWindow::new().range("lat", 120, 218).range("lon", 300, 497);
/// let mut g = PrefetchGroup::builder()
///     .member("U", Arc::new(p_u.with_window(w.clone())?))
///     .member("V", Arc::new(p_v.with_window(w)?))
///     .depth(2).max_bytes(8 << 30).workers(4)
///     .build()?;
/// let b: Vec<f64> = g.boundaries();          // Unix seconds, as refresh_times
/// g.start(Order::Forward, None)?;
/// for k in 0..b.len() {
///     let ep = g.take(k)?;
///     let u = ep.get("U").unwrap();
///     if u.changed { /* copy u.data into the forcing buffer */ }
/// }
/// // Drop cancels and joins.
/// # Ok(()) }
/// ```
pub struct PrefetchGroup<T = NativeDataset> {
    shared: Arc<Shared<T>>,
    threads: Vec<JoinHandle<()>>,
}

impl<T: Send + Sync + 'static> PrefetchGroup<T> {
    /// The sorted, de-duplicated union of the members' refresh times, in Unix
    /// seconds. Epoch `k` is `[B[k], B[k+1])`; the last is open on the right.
    pub fn boundaries(&self) -> Vec<f64> {
        self.shared.config.boundaries.clone()
    }

    /// Declare the order epochs will be taken in, cancelling any previous order
    /// (in-flight decodes run on; their results are discarded). `from` trims the
    /// order to begin at its first entry equal to `from` — how a restart
    /// resumes. An empty or out-of-range explicit order, or a `from` absent from
    /// the order, is an error, and leaves the previous order running.
    ///
    /// Never raises a decode error: those are stored and raised by
    /// [`take`](Self::take).
    pub fn start(&mut self, order: Order, from: Option<usize>) -> Result<()> {
        let n = self.shared.config.boundaries.len();
        let mut seq: Vec<usize> = match order {
            Order::Forward => (0..n).collect(),
            Order::Reverse => (0..n).rev().collect(),
            Order::Explicit(v) => {
                if v.is_empty() {
                    return Err(prefetch_err("an explicit order must name at least one epoch".into()));
                }
                if let Some(&bad) = v.iter().find(|&&e| e >= n) {
                    return Err(prefetch_err(format!(
                        "epoch {bad} is out of range: the group has {n} epochs (0..{n})"
                    )));
                }
                v
            }
        };
        if let Some(from) = from {
            let Some(at) = seq.iter().position(|&e| e == from) else {
                return Err(prefetch_err(format!(
                    "start(from = {from}): epoch {from} is not in the order"
                )));
            };
            seq.drain(..at);
        }
        let shared = &*self.shared;
        let mut st = lock(&shared.state);
        if st.phase == Phase::Closed {
            return Err(closed());
        }
        st.end_order(Phase::Active);
        st.order = seq;
        st.admit(&shared.config);
        shared.work.notify_all();
        Ok(())
    }

    /// The next epoch's sample: blocks until every member's sample for it is
    /// decoded. `k` must be the next entry of the order; anything else is an
    /// error naming the expected epoch, and consumes nothing.
    ///
    /// A stored decode (or transform) error of this slot is raised here — the
    /// first by member key order when several failed, and exactly the error
    /// `refresh(B[k])` would have raised. It ends the order: the next `take` is
    /// an error until [`start`](Self::start) is called again, and
    /// `start(order, Some(e))` resumes at entry `e`.
    pub fn take(&mut self, k: usize) -> Result<EpochSample<T>> {
        let shared = &*self.shared;
        let mut st = lock(&shared.state);
        match st.phase {
            Phase::Active => {}
            Phase::Idle => {
                return Err(prefetch_err(format!(
                    "take({k}) before any start: declare the order with start first"
                )))
            }
            Phase::Cancelled => {
                return Err(prefetch_err(format!(
                    "take({k}) after cancel: the order was dropped; call start again"
                )))
            }
            Phase::Failed { epoch } => {
                return Err(prefetch_err(format!(
                    "take({k}): the order ended when take({epoch}) raised its stored \
                     error; call start again (start(order, Some(epoch)) resumes there)"
                )))
            }
            Phase::Closed => return Err(closed()),
        }
        let Some(&expected) = st.order.get(st.next) else {
            return Err(prefetch_err(format!(
                "take({k}): the order is exhausted (all {} entries taken); call start again",
                st.order.len()
            )));
        };
        if k != expected {
            return Err(prefetch_err(format!(
                "take({k}): the next epoch in the current order is {expected}"
            )));
        }

        if !st.head_done() {
            let waiting = Instant::now();
            while !st.head_done() {
                st = wait(&shared.done, st);
            }
            st.stats.waited_s += waiting.elapsed().as_secs_f64();
        }
        let head = st.slots.pop_front().expect("the head slot is always admitted");
        debug_assert_eq!(head.pos, st.next);

        // A failed member: the first by key order (`keys` is in member order,
        // and members are sorted), moved out of its cell — an `Error` is not
        // `Clone`, and the order ends here, so it is raised exactly once.
        if let Some(failed) = head
            .keys
            .iter()
            .find(|key| matches!(st.cells[*key].status, CellStatus::Failed(_)))
        {
            let err = match &mut st.cells.get_mut(failed).expect("live cell").status {
                CellStatus::Failed(e) => e.take(),
                _ => None,
            };
            st.end_order(Phase::Failed { epoch: k });
            return Err(err.unwrap_or_else(|| {
                prefetch_err(format!("take({k}): a member decode failed"))
            }));
        }

        let mut samples = BTreeMap::new();
        for (i, key) in head.keys.iter().enumerate() {
            let cell = st.cells.get_mut(key).expect("live cell");
            let CellStatus::Ready(data, bytes) = &cell.status else {
                unreachable!("head_done and no failure means every cell is ready");
            };
            let (data, bytes) = (data.clone(), *bytes);
            cell.slots -= 1;
            if cell.slots == 0 {
                st.cells.remove(key);
            }
            let changed = st.prev_anchor[i] != Some(key.1);
            st.prev_anchor[i] = Some(key.1);
            st.last_taken[i] = Some((key.1, data.clone(), bytes));
            samples.insert(
                shared.config.members[i].key.clone(),
                Sample {
                    data,
                    anchor: key.1.unix_timestamp() as f64,
                    changed,
                },
            );
        }
        st.next += 1;
        st.admit(&shared.config);
        shared.work.notify_all();
        Ok(EpochSample { epoch: k, samples })
    }

    /// Whether `take(k)` would return at once: `k` is the next entry of the
    /// current order and its slot is finished (decoded or failed). Never blocks
    /// and never raises.
    pub fn ready(&self, k: usize) -> bool {
        let st = lock(&self.shared.state);
        st.phase == Phase::Active && st.order.get(st.next) == Some(&k) && st.head_done()
    }

    /// Drop the queued work at once; a decode already running finishes and its
    /// result is discarded. With `wait`, return only after those decodes finish.
    /// `take` is an error until the next [`start`](Self::start).
    pub fn cancel(&mut self, wait_for_running: bool) {
        let shared = &*self.shared;
        let mut st = lock(&shared.state);
        if st.phase == Phase::Closed {
            return;
        }
        st.end_order(Phase::Cancelled);
        if wait_for_running {
            while st.running > 0 {
                st = wait(&shared.done, st);
            }
        }
    }

    /// `cancel(true)`, then stop and join every worker. The group is unusable
    /// afterwards (`start`/`take` are errors). Idempotent; [`Drop`] calls it.
    pub fn close(&mut self) {
        {
            let shared = &*self.shared;
            let mut st = lock(&shared.state);
            if st.phase != Phase::Closed {
                st.end_order(Phase::Closed);
            }
            while st.running > 0 {
                st = wait(&shared.done, st);
            }
            st.shutdown = true;
            shared.work.notify_all();
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }

    /// The cumulative statistics (§3.9).
    pub fn stats(&self) -> PrefetchStats {
        lock(&self.shared.state).stats
    }
}

impl<T> std::fmt::Debug for PrefetchGroup<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cfg = &self.shared.config;
        let keys: Vec<&str> = cfg.members.iter().map(|m| m.key.as_str()).collect();
        f.debug_struct("PrefetchGroup")
            .field("members", &keys)
            .field("epochs", &cfg.boundaries.len())
            .field("depth", &cfg.depth)
            .field("max_bytes", &cfg.max_bytes)
            .field("threads", &self.threads.len())
            .finish_non_exhaustive()
    }
}

impl<T> Drop for PrefetchGroup<T> {
    fn drop(&mut self) {
        // `close` without the `T: Send + Sync` bound `impl Drop` cannot carry.
        {
            let shared = &*self.shared;
            let mut st = lock(&shared.state);
            if st.phase != Phase::Closed {
                st.end_order(Phase::Closed);
            }
            st.shutdown = true;
            shared.work.notify_all();
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

// --- internals --------------------------------------------------------------

/// One member, as the workers see it.
struct Member {
    key: String,
    provider: Arc<Provider>,
    /// This member's decoded files, shared by every worker.
    files: SharedFiles,
    /// With `windows`: the windows and the union their files are read under.
    plan: Option<WindowPlan>,
}

/// A member's sample without windows: [`Provider::sample_at`]'s path, fed from
/// the shared decoded files.
fn decode_sample(m: &Member, t: OffsetDateTime) -> Result<NativeDataset> {
    m.provider.sample_from(t, m.provider.baked_select(), &mut &m.files)
}

/// A member's sample with windows: [`Provider::sample_windows_at`]'s path.
fn decode_windows(m: &Member, t: OffsetDateTime) -> Result<Vec<NativeDataset>> {
    let plan = m.plan.as_ref().expect("a windowed group plans every member");
    m.provider.sample_windows_from(t, plan, &mut &m.files)
}

/// The decoded files a member's samples share: at most [`FILES_PER_MEMBER`]
/// whole decoded files, most-recently-used at the tail, plus the files some
/// worker is decoding right now. A worker that needs a file another is decoding
/// waits for it instead of decoding it again (§3.6).
///
/// Each member reads every file under ONE select — its baked window, or the
/// union of its group windows — so the file anchor alone is the key.
#[derive(Default)]
struct SharedFiles {
    state: Mutex<FilesState>,
    published: Condvar,
}

#[derive(Default)]
struct FilesState {
    ready: Vec<(OffsetDateTime, Arc<NativeDataset>)>,
    loading: HashSet<OffsetDateTime>,
}

/// The Provider's own LRU size (`records_per_sample = 2` holds two adjacent
/// files at a seam), and so the bound §3.6 documents for this binding.
const FILES_PER_MEMBER: usize = 2;

impl FileSource for &SharedFiles {
    fn file(
        &mut self,
        provider: &Provider,
        file_anchor: OffsetDateTime,
        select: &Selection,
    ) -> Result<Arc<NativeDataset>> {
        let shared: &SharedFiles = self;
        {
            let mut st = lock(&shared.state);
            loop {
                if let Some(pos) = st.ready.iter().position(|(a, _)| *a == file_anchor) {
                    let entry = st.ready.remove(pos);
                    let ds = entry.1.clone();
                    st.ready.push(entry);
                    return Ok(ds);
                }
                if st.loading.insert(file_anchor) {
                    break;
                }
                st = wait(&shared.published, st);
            }
        }
        let mut loading = Loading {
            shared,
            file_anchor,
            decoded: None,
        };
        let ds = provider.read_file_at(file_anchor, select).map(Arc::new)?;
        loading.decoded = Some(ds.clone());
        Ok(ds)
    }
}

/// Marks a file as being decoded, and on drop — success, error or panic alike —
/// publishes the result (if any) and wakes the workers waiting on it, so a
/// failed decode never leaves them waiting forever. A waiter whose file failed
/// decodes it itself and meets the error on its own.
struct Loading<'a> {
    shared: &'a SharedFiles,
    file_anchor: OffsetDateTime,
    decoded: Option<Arc<NativeDataset>>,
}

impl Drop for Loading<'_> {
    fn drop(&mut self) {
        let mut st = lock(&self.shared.state);
        st.loading.remove(&self.file_anchor);
        if let Some(ds) = self.decoded.take() {
            st.ready.push((self.file_anchor, ds));
            while st.ready.len() > FILES_PER_MEMBER {
                st.ready.remove(0);
            }
        }
        self.shared.published.notify_all();
    }
}

/// What the consumer and the workers share.
struct Shared<T> {
    config: Config<T>,
    state: Mutex<State<T>>,
    /// Workers and the fetcher wait here for work (or shutdown).
    work: Condvar,
    /// `take` waits here for results, `cancel(true)`/`close` for running decodes.
    done: Condvar,
}

/// Fixed at construction.
struct Config<T> {
    /// Sorted by key, byte-wise.
    members: Vec<Member>,
    boundaries: Vec<f64>,
    /// `anchors[member][epoch]`: the member's cadence anchor for `B[epoch]`.
    anchors: Vec<Vec<OffsetDateTime>>,
    depth: usize,
    max_bytes: Option<u64>,
    fetch_ahead: usize,
    produce: ProduceFn<T>,
    latency: Option<LatencyFn>,
}

/// A member sample is identified by its member and its cadence anchor: two
/// epochs that snap to one anchor share one sample (`spec/prefetch.md` §1).
type CellKey = (usize, OffsetDateTime);

struct Cell<T> {
    status: CellStatus<T>,
    /// Live slots referencing this sample; at 0 it is released.
    slots: usize,
    /// The member's last decoded size when this was queued, for admission.
    estimate: Option<u64>,
}

enum CellStatus<T> {
    Queued,
    Running,
    Ready(Arc<T>, u64),
    /// The stored error, moved out by the `take` that raises it.
    Failed(Option<Error>),
}

impl<T> CellStatus<T> {
    fn finished(&self) -> bool {
        matches!(self, CellStatus::Ready(..) | CellStatus::Failed(_))
    }
}

/// One admitted position of the order: one sample per member.
struct Slot {
    pos: usize,
    /// One key per member, in member order.
    keys: Vec<CellKey>,
    /// Counted in `epochs_decoded` (and `oversize_epochs`) already.
    settled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Active,
    Cancelled,
    Failed { epoch: usize },
    Closed,
}

struct State<T> {
    phase: Phase,
    /// Bumped by every start/cancel; a result from an older generation is
    /// discarded, which is how "a running decode finishes and is dropped" works.
    generation: u64,
    order: Vec<usize>,
    /// The position `take` returns next.
    next: usize,
    /// Positions `next..admitted` hold the live `slots`.
    admitted: usize,
    slots: VecDeque<Slot>,
    cells: HashMap<CellKey, Cell<T>>,
    queue: VecDeque<(u64, CellKey)>,
    fetch_queue: VecDeque<(u64, usize, String)>,
    /// Positions below this have had their files queued for warm-up.
    fetch_end: usize,
    fetched: HashSet<(usize, String)>,
    /// Per member: the anchor at the previous `take` of the current order.
    prev_anchor: Vec<Option<OffsetDateTime>>,
    /// Per member: the sample the last `take` delivered, so an epoch right
    /// behind it that repeats the anchor reuses it instead of decoding again.
    last_taken: Vec<Option<(OffsetDateTime, Arc<T>, u64)>>,
    /// Per member: the size of its last decoded sample — the admission estimate.
    last_size: Vec<Option<u64>>,
    /// Decodes running on workers right now (any generation).
    running: usize,
    shutdown: bool,
    stats: PrefetchStats,
}

impl<T> State<T> {
    fn new(members: usize) -> Self {
        State {
            phase: Phase::Idle,
            generation: 0,
            order: Vec::new(),
            next: 0,
            admitted: 0,
            slots: VecDeque::new(),
            cells: HashMap::new(),
            queue: VecDeque::new(),
            fetch_queue: VecDeque::new(),
            fetch_end: 0,
            fetched: HashSet::new(),
            prev_anchor: vec![None; members],
            last_taken: (0..members).map(|_| None).collect(),
            last_size: vec![None; members],
            running: 0,
            shutdown: false,
            stats: PrefetchStats {
                overlap: true,
                ..PrefetchStats::default()
            },
        }
    }

    /// Drop the current order and everything queued or held for it, entering
    /// `phase`. Running decodes are not interrupted; the generation bump makes
    /// their results land nowhere.
    fn end_order(&mut self, phase: Phase) {
        self.phase = phase;
        self.generation += 1;
        self.order.clear();
        self.next = 0;
        self.admitted = 0;
        self.slots.clear();
        self.cells.clear();
        self.queue.clear();
        self.fetch_queue.clear();
        self.fetch_end = 0;
        self.fetched.clear();
        self.prev_anchor.iter_mut().for_each(|a| *a = None);
    }

    fn head_done(&self) -> bool {
        self.slots
            .front()
            .is_some_and(|s| s.keys.iter().all(|k| self.cells[k].status.finished()))
    }

    fn any_failed(&self) -> bool {
        self.slots.iter().any(|s| {
            s.keys
                .iter()
                .any(|k| matches!(self.cells[k].status, CellStatus::Failed(_)))
        })
    }

    /// Bytes held or in flight across the live slots, each sample once; `None`
    /// while an in-flight sample's size is unknown.
    fn committed_bytes(&self) -> Option<u64> {
        let mut total = 0;
        for cell in self.cells.values() {
            total += match &cell.status {
                CellStatus::Ready(_, b) => *b,
                CellStatus::Failed(_) => 0,
                CellStatus::Queued | CellStatus::Running => cell.estimate?,
            };
        }
        Some(total)
    }

    /// Admit slots behind the consumer while `depth` and `max_bytes` allow
    /// (§3.6), then queue the disk warm-up past them. Scheduling stops past a
    /// failed slot (§3.7).
    fn admit(&mut self, cfg: &Config<T>) {
        if self.phase != Phase::Active {
            return;
        }
        while self.admitted < self.order.len() && self.admitted - self.next < cfg.depth {
            if self.any_failed() {
                break;
            }
            let epoch = self.order[self.admitted];
            let keys: Vec<CellKey> = (0..cfg.members.len()).map(|i| (i, cfg.anchors[i][epoch])).collect();
            let held = |st: &Self, k: &CellKey| {
                st.cells.contains_key(k)
                    || matches!(&st.last_taken[k.0], Some((a, ..)) if *a == k.1)
            };
            // The head slot is always admitted; behind it, a slot must fit.
            if self.admitted > self.next {
                if let Some(max) = cfg.max_bytes {
                    let mut estimate = Some(0u64);
                    for k in keys.iter().filter(|k| !held(self, k)) {
                        estimate = estimate.zip(self.last_size[k.0]).map(|(a, b)| a + b);
                    }
                    // An unknown size admits a slot only when nothing else is
                    // held or in flight — never true behind a live head.
                    match (estimate, self.committed_bytes()) {
                        (Some(e), Some(c)) if c + e <= max => {}
                        _ => break,
                    }
                }
            }
            for &k in &keys {
                if let Some(cell) = self.cells.get_mut(&k) {
                    cell.slots += 1;
                    self.stats.samples_reused += 1;
                    continue;
                }
                if let Some((a, data, bytes)) = &self.last_taken[k.0] {
                    if *a == k.1 {
                        let status = CellStatus::Ready(data.clone(), *bytes);
                        self.cells.insert(k, Cell { status, slots: 1, estimate: Some(*bytes) });
                        self.stats.samples_reused += 1;
                        continue;
                    }
                }
                let estimate = self.last_size[k.0];
                self.cells.insert(k, Cell { status: CellStatus::Queued, slots: 1, estimate });
                self.queue.push_back((self.generation, k));
            }
            self.slots.push_back(Slot {
                pos: self.admitted,
                keys,
                settled: false,
            });
            self.admitted += 1;
        }
        self.settle(cfg);

        if cfg.fetch_ahead > 0 && !self.any_failed() {
            let end = (self.admitted + cfg.fetch_ahead).min(self.order.len());
            for pos in self.fetch_end.max(self.admitted)..end {
                let t = OffsetDateTime::from_unix_timestamp(cfg.boundaries[self.order[pos]] as i64)
                    .expect("boundaries were validated at build");
                for (i, m) in cfg.members.iter().enumerate() {
                    for url in m.provider.warm_urls(t) {
                        if self.fetched.insert((i, url.clone())) {
                            self.fetch_queue.push_back((self.generation, i, url));
                        }
                    }
                }
            }
            self.fetch_end = self.fetch_end.max(end);
        }
    }

    /// Count newly finished slots and update the byte peak.
    fn settle(&mut self, cfg: &Config<T>) {
        for i in 0..self.slots.len() {
            if self.slots[i].settled {
                continue;
            }
            let mut bytes = 0u64;
            let mut ready = true;
            for k in &self.slots[i].keys {
                match &self.cells[k].status {
                    CellStatus::Ready(_, b) => bytes += b,
                    _ => ready = false,
                }
            }
            if ready {
                self.slots[i].settled = true;
                self.stats.epochs_decoded += 1;
                if cfg.max_bytes.is_some_and(|max| bytes > max) {
                    self.stats.oversize_epochs += 1;
                }
            }
        }
        let held: u64 = self
            .cells
            .values()
            .map(|c| match &c.status {
                CellStatus::Ready(_, b) => *b,
                _ => 0,
            })
            .sum();
        self.stats.peak_bytes = self.stats.peak_bytes.max(held);
    }
}

/// A decode worker: take the oldest queued sample, produce it with no lock
/// held, store it if its generation is still current.
fn worker<T: Send + Sync + 'static>(shared: Arc<Shared<T>>) {
    let cfg = &shared.config;
    let mut st = lock(&shared.state);
    loop {
        if st.shutdown {
            return;
        }
        let Some((generation, key)) = st.queue.pop_front() else {
            st = wait(&shared.work, st);
            continue;
        };
        if generation != st.generation {
            continue;
        }
        match st.cells.get_mut(&key) {
            Some(cell) => cell.status = CellStatus::Running,
            None => continue,
        }
        st.running += 1;
        drop(st);

        let member = &cfg.members[key.0];
        let started = Instant::now();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            if let Some(latency) = &cfg.latency {
                std::thread::sleep(latency(&member.key, key.1.unix_timestamp() as f64));
            }
            (cfg.produce)(member, key.1)
        }))
        .unwrap_or_else(|panic| {
            Err(prefetch_err(format!(
                "producing member {:?}'s sample at anchor {} panicked: {}",
                member.key,
                key.1,
                panic_text(&*panic)
            )))
        });
        let elapsed = started.elapsed().as_secs_f64();

        st = lock(&shared.state);
        st.running -= 1;
        st.stats.decode_s += elapsed;
        if generation == st.generation && st.cells.contains_key(&key) {
            let status = match outcome {
                Ok((data, bytes)) => {
                    st.last_size[key.0] = Some(bytes);
                    st.stats.samples_decoded += 1;
                    CellStatus::Ready(Arc::new(data), bytes)
                }
                Err(e) => CellStatus::Failed(Some(e)),
            };
            st.cells.get_mut(&key).expect("checked above").status = status;
            st.settle(cfg);
            st.admit(cfg);
            shared.work.notify_all();
        }
        shared.done.notify_all();
    }
}

/// The disk warm-up thread: downloads files of slots past the decode window
/// (§3.6 `fetch_ahead`). Its errors are dropped — the decode raises the real one
/// at `take` — and it never holds a decode worker.
fn fetcher<T>(shared: Arc<Shared<T>>) {
    let mut st = lock(&shared.state);
    loop {
        if st.shutdown {
            return;
        }
        let Some((generation, member, url)) = st.fetch_queue.pop_front() else {
            st = wait(&shared.work, st);
            continue;
        };
        if generation != st.generation {
            continue;
        }
        drop(st);
        let provider = &shared.config.members[member].provider;
        let _ = catch_unwind(AssertUnwindSafe(|| provider.warm(&url)));
        st = lock(&shared.state);
    }
}

fn spawn(name: String, f: impl FnOnce() + Send + 'static) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(name)
        .spawn(f)
        .map_err(|e| Error::io(None, e))
}

fn prefetch_err(detail: String) -> Error {
    Error::Prefetch { detail }
}

fn closed() -> Error {
    prefetch_err("the group is closed".into())
}

fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a non-string panic payload".to_string())
}

/// Lock, recovering from a poisoned mutex: every critical section here leaves
/// the state consistent before it can panic, and a poisoned group that refused
/// every later call would turn one panicking transform into a hung solver.
fn lock<S>(m: &Mutex<S>) -> MutexGuard<'_, S> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn wait<'a, S>(cv: &Condvar, guard: MutexGuard<'a, S>) -> MutexGuard<'a, S> {
    cv.wait(guard).unwrap_or_else(|e| e.into_inner())
}
