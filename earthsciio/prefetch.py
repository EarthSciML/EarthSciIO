"""Epoch prefetch — :class:`PrefetchGroup` (``spec/prefetch.md`` §3).

A solver integrating against DISCRETE forcing stops at every refresh boundary
and calls :meth:`~earthsciio.provider.Provider.refresh`, which decodes while the
solver waits. A :class:`PrefetchGroup` decodes the upcoming EPOCHS of several
providers on background workers, in an order the caller declares up front
(forward, the adjoint's reverse sweep, or a revolve schedule), under a memory
budget, and hands each epoch over with :meth:`PrefetchGroup.take`.

**The one rule** (§3.5): scheduling changes WHEN a result is ready, never WHAT
it is. ``take(k)[key].data`` is bitwise ``members[key].sample_at(B[k])`` — and so
what ``refresh(B[k])`` returns — for every order, depth, worker count, budget and
completion order. That holds by construction, not by care: every decode runs the
Provider's own :meth:`~earthsciio.provider.Provider._sample`, the one path behind
``refresh`` and ``sample_at``; results are keyed by order POSITION and member,
never by completion; and the only thing workers share is decoded files, which
are deterministic.

What the group adds around that path:

* **Dedupe by anchor.** A member's sample depends on ``B[k]`` only through the
  cadence anchor ``B[k]`` snaps to (§1), so a coarse-cadence member is decoded
  once per anchor, not once per epoch: a held ``(member, anchor)`` sample is
  shared by every slot that needs it, and the sample last handed to the consumer
  is reused when the next slot asks for the same anchor (``samples_reused``).
* **Decoded-file reuse** (§3.6, a binding choice outside ``max_bytes``): each
  member keeps at most TWO decoded files — the Provider's own LRU size — shared
  by every worker thread, and a file one worker is decoding is awaited by the
  others rather than decoded twice (:class:`_FileShare`). Process workers keep
  their own two-file LRU per member each (decoded data cannot be shared across
  processes), so ``executor="process"`` holds at most ``2 x workers`` decoded
  files per member.
* **Budget.** ``depth`` slots ahead of the consumer (the next one included), and
  optionally ``max_bytes`` of decoded member samples; the head slot is always
  admitted. ``fetch_ahead`` warms the disk cache past the decode window.
* **Errors are deferred.** A decode error is stored against its (position,
  member) and raised by the ``take`` of that position — never by ``start`` or
  ``ready``, and never earlier — as the very exception ``refresh`` would raise
  (an offline cold cache raises :class:`~earthsciio.errors.CacheMiss`).

Threads, the GIL and processes (§3.10). ``executor="thread"`` (the default)
decodes on a thread pool. NetCDF decodes serialise on one process-wide lock
(:data:`earthsciio.readers._NETCDF_LOCK`; netCDF4/HDF5 is not assumed
thread-safe), so for NetCDF the overlap won is decode BESIDE the consumer's
compute, not two decodes at once; zarr decodes run concurrently.
``executor="process"`` decodes in ``spawn``-started worker processes for when the
GIL starves that overlap; its members and ``transform`` must pickle, which is
checked at construction. A :class:`~earthsciio.cache.Cache` pickles without its
per-process rung-0 memo and a :class:`~earthsciio.provider.Provider` without its
decode buffers (both are rebuilt in the worker), and every
:class:`~earthsciio.errors.EarthSciIOError` pickles with its fields, so a worker's
``CacheMiss`` reaches ``take`` as a ``CacheMiss``. ``spawn`` rather than the
platform default because ``fork`` copies whatever locks other threads of the
consumer hold at that instant — the NetCDF lock included.

``take``, ``start``, ``ready`` and ``cancel`` are the consumer's: call them from
one thread. Only decoding runs elsewhere.
"""

from __future__ import annotations

import concurrent.futures as _cf
import datetime as _dt
import multiprocessing as _mp
import operator
import pickle
import queue
import threading
import time
from collections import OrderedDict
from collections.abc import Mapping
from typing import Any, Callable, Dict, Iterator, List, NamedTuple, Optional, Sequence, Tuple

import numpy as np

from .native import NativeDataset
from .provider import Provider
from .selection import _WindowUnion

__all__ = ["PrefetchGroup", "Sample", "EpochSample"]


# --------------------------------------------------------------------------- #
# What take() returns (spec/prefetch.md §3.4).
# --------------------------------------------------------------------------- #


class Sample(NamedTuple):
    """One member's data for one epoch.

    ``data`` is ``members[key].sample_at(B[k])`` (with ``windows``, the list
    ``sample_windows_at(B[k], windows)``; with ``transform``, ``transform(key,
    <that>)``). It is **read-only** — the same object may be handed to several
    epochs when the member's anchor repeats, so its numpy arrays are marked
    non-writeable; copy before mutating. ``anchor`` is the member's cadence
    anchor for ``B[k]``. ``changed`` says whether ``anchor`` differs from this
    member's anchor at the previous ``take`` of the current order (always true
    on the first ``take`` after ``start``): copy only the changed samples into
    the forcing buffers.
    """

    data: Any
    anchor: _dt.datetime
    changed: bool


class EpochSample(Mapping):
    """Every member's :class:`Sample` for one epoch: ``ep["U"].data``.

    A read-only mapping keyed by member, plus :attr:`epoch` (the epoch index
    ``k``) and :attr:`time` (its start boundary ``B[k]``).
    """

    __slots__ = ("epoch", "time", "_samples")

    def __init__(self, epoch: int, time: _dt.datetime, samples: Dict[str, Sample]) -> None:
        self.epoch = epoch
        self.time = time
        self._samples = samples

    def __getitem__(self, key: str) -> Sample:
        return self._samples[key]

    def __iter__(self) -> Iterator[str]:
        return iter(self._samples)

    def __len__(self) -> int:
        return len(self._samples)

    def __repr__(self) -> str:  # pragma: no cover - debug aid
        return f"EpochSample(epoch={self.epoch}, time={self.time}, members={list(self._samples)})"


# --------------------------------------------------------------------------- #
# The decode a worker runs, and the decoded files it shares.
# --------------------------------------------------------------------------- #


class _Pending:
    __slots__ = ("event", "result", "error")

    def __init__(self) -> None:
        self.event = threading.Event()
        self.result: Optional[NativeDataset] = None
        self.error: Optional[BaseException] = None


class _FileShare:
    """At most two decoded files of ONE member, shared by every worker.

    The file source a group feeds :meth:`Provider._sample`: the Provider's own
    two-entry LRU, made safe to share. A file one worker is decoding is awaited
    by the others (in-flight dedupe), so a file is decoded once however many
    epochs and workers want it at the same time. A failed decode is not
    remembered — the next request reads again and raises the real error again.
    Sits outside ``max_bytes`` (§3.6); bound: two completed files plus those
    being decoded (at most one per worker).
    """

    CAP = 2

    def __init__(self, provider: Provider, select: Optional[Any]) -> None:
        self._provider = provider
        self._select = select
        self._lock = threading.Lock()
        self._done: "OrderedDict[_dt.datetime, NativeDataset]" = OrderedDict()
        self._pending: Dict[_dt.datetime, _Pending] = {}

    def __call__(self, file_anchor: _dt.datetime) -> NativeDataset:
        with self._lock:
            ds = self._done.get(file_anchor)
            if ds is not None:
                self._done.move_to_end(file_anchor)
                return ds
            pend = self._pending.get(file_anchor)
            owner = pend is None
            if owner:
                pend = self._pending[file_anchor] = _Pending()
        if not owner:
            pend.event.wait()
            if pend.error is not None:
                raise pend.error
            return pend.result  # type: ignore[return-value]
        try:
            pend.result = self._provider._read_file(
                self._provider.loader.resolve_url(file_anchor), select=self._select
            )
            return pend.result
        except BaseException as exc:
            pend.error = exc
            raise
        finally:
            with self._lock:
                self._pending.pop(file_anchor, None)
                if pend.error is None:
                    self._done[file_anchor] = pend.result  # type: ignore[assignment]
                    while len(self._done) > self.CAP:
                        self._done.popitem(last=False)
            pend.event.set()

    def clear(self) -> None:
        with self._lock:
            self._done.clear()


def _str_bytes(values: Sequence[Any]) -> int:
    return sum(
        len(v) if isinstance(v, bytes) else len(str(v).encode("utf-8")) for v in values
    )


def _dataset_nbytes(ds: NativeDataset) -> int:
    """A decoded dataset's array bytes (§3.6): numeric arrays at ``nbytes``,
    string fields at their UTF-8 length."""
    n = 0
    for f in list(ds.variables.values()) + list(ds.coords.values()):
        d = f.data
        if isinstance(d, np.ndarray):
            if d.dtype.kind in "SUO":
                n += _str_bytes(d.ravel().tolist())
            else:
                n += int(d.nbytes)
        else:
            n += _str_bytes(list(d))
    return n


def _freeze(data: Any) -> Any:
    """Mark a sample's numpy arrays read-only (§3.4: ``data`` may be shared by
    several epochs, so an in-place edit would leak into the next one)."""
    items = data if isinstance(data, list) else [data]
    for ds in items:
        if isinstance(ds, NativeDataset):
            for f in list(ds.variables.values()) + list(ds.coords.values()):
                if isinstance(f.data, np.ndarray):
                    f.data.flags.writeable = False
    return data


def _produce(
    provider: Provider,
    files: _FileShare,
    union: Optional[_WindowUnion],
    no_windows: bool,
    transform: Optional[Callable[[str, Any], Any]],
    key: str,
    t: _dt.datetime,
) -> Tuple[Any, int]:
    """One member sample: ``(data, bytes before transform)``.

    The decode is the Provider's own ``_sample`` — the path ``refresh`` and
    ``sample_at`` run — fed from the shared file source. ``no_windows`` is
    ``windows=[]``: ``sample_windows_at(t, [])`` is ``[]`` without a fetch.
    """
    if no_windows:
        data: Any = []
        nbytes = 0
    elif union is None:
        data = provider._sample(t, files)
        nbytes = _dataset_nbytes(data)
    else:
        data = provider._sample_windows(t, union, files)
        nbytes = sum(_dataset_nbytes(d) for d in data)
    _freeze(data)
    if transform is not None:
        data = transform(key, data)
    return data, nbytes


# -- process workers --------------------------------------------------------- #

#: Per-worker-process state, set once by :func:`_proc_init`.
_WORKER: Dict[str, Any] = {}


def _proc_init(payload: bytes) -> None:
    members, windows, transform, latency = pickle.loads(payload)
    union = _WindowUnion(windows) if windows else None
    select = union.select if union is not None else None
    _WORKER.clear()
    _WORKER.update(
        members=members,
        files={k: _FileShare(p, select) for k, p in members.items()},
        union=union,
        no_windows=windows is not None and len(windows) == 0,
        transform=transform,
        latency=latency,
    )


def _proc_run(key: str, t: _dt.datetime) -> Tuple[Any, int, Optional[BaseException], float]:
    w = _WORKER
    if w["latency"] is not None:
        time.sleep(max(0.0, float(w["latency"]())))
    t0 = time.perf_counter()
    try:
        data, nbytes = _produce(w["members"][key], w["files"][key], w["union"],
                                w["no_windows"], w["transform"], key, t)
        err: Optional[BaseException] = None
    except Exception as exc:
        data, nbytes, err = None, 0, exc
        try:
            pickle.dumps(exc)
        except Exception:
            err = RuntimeError(f"{type(exc).__name__}: {exc}")
    return data, nbytes, err, time.perf_counter() - t0


# --------------------------------------------------------------------------- #
# Scheduling state.
# --------------------------------------------------------------------------- #


class _Task:
    """One ``(member, anchor)`` sample, decoded once and shared by every held
    slot that needs it."""

    __slots__ = ("key", "anchor", "t", "gen", "done", "data", "error", "nbytes",
                 "future", "refs")

    def __init__(self, key: str, anchor: _dt.datetime, t: _dt.datetime, gen: int) -> None:
        self.key = key
        self.anchor = anchor
        self.t = t
        self.gen = gen
        self.done = False
        self.data: Any = None
        self.error: Optional[BaseException] = None
        self.nbytes = 0
        self.future: Optional[_cf.Future] = None
        self.refs = 0


class _Slot:
    """One position of the order: its epoch and one task per member."""

    __slots__ = ("pos", "epoch", "tasks", "done", "error")

    def __init__(self, pos: int, epoch: int) -> None:
        self.pos = pos
        self.epoch = epoch
        self.tasks: Dict[str, _Task] = {}
        self.done = False
        self.error: Optional[BaseException] = None


_IDLE, _RUNNING, _CANCELLED, _FAILED, _CLOSED = (
    "idle", "running", "cancelled", "failed", "closed",
)


class PrefetchGroup:
    """Background decode of upcoming forcing epochs (``spec/prefetch.md`` §3).

    Parameters
    ----------
    members:
        ``{key: Provider}``, every one DISCRETE with an enumerable schedule
        (non-empty :meth:`~earthsciio.provider.Provider.refresh_times`). A
        CONST or unbounded member, or an empty map, is a :class:`ValueError`:
        materialize a CONST source once, outside the group.
    depth:
        Epoch slots held ahead of the consumer, decoded or decoding, counting
        the one :meth:`take` returns next (at least 1).
    max_bytes:
        Optional cap on the bytes of the held slots' member samples (array
        bytes before ``transform``; a sample shared by several slots counts
        once). A slot is admitted only if what is held or in flight plus its
        estimate — each member's last decoded size — fits; while some size is
        still unknown, only when nothing else is held. The head slot is always
        admitted, alone if it is oversize (``stats()["oversize_epochs"]``).
    fetch_ahead:
        Slots past the decode window whose files are downloaded (not decoded)
        into the disk cache; defaults to ``depth``, ``0`` disables.
        Best-effort: it never blocks a take, its errors are dropped (the
        decode raises the real one), and store-backed and offline members are
        skipped (there is nothing such a warm could download).
    workers:
        Decode workers (threads, or processes with ``executor="process"``).
    windows:
        Optional named windows: each member's data becomes
        ``sample_windows_at(B[k], windows)``. Every window must satisfy
        :meth:`~earthsciio.provider.Provider.with_window`'s preconditions for
        every member, checked here.
    transform:
        Optional ``transform(key, data) -> T`` run on the worker after the
        decode; its errors are stored like decode errors.
    executor:
        ``"thread"`` (default) or ``"process"`` (members and ``transform`` must
        pickle; checked here).

    ``with PrefetchGroup(...) as g:`` closes the group on exit.
    """

    def __init__(
        self,
        members: Mapping,
        *,
        depth: int = 2,
        max_bytes: Optional[int] = None,
        fetch_ahead: Optional[int] = None,
        workers: int = 1,
        windows: Optional[Sequence[Any]] = None,
        transform: Optional[Callable[[str, Any], Any]] = None,
        executor: str = "thread",
        _decode_latency: Optional[Callable[[], float]] = None,
    ) -> None:
        if not isinstance(members, Mapping) or not members:
            raise ValueError("a PrefetchGroup needs a non-empty {key: Provider} map")
        self._members: Dict[str, Provider] = {}
        for key, prov in members.items():
            if not isinstance(key, str):
                raise TypeError(f"member keys are strings, got {key!r}")
            if not isinstance(prov, Provider):
                raise TypeError(f"member {key!r} is not a Provider: {type(prov).__name__}")
            if prov.is_const:
                raise ValueError(
                    f"member {key!r} is CONST: it has no epochs to prefetch — "
                    "materialize() it once, outside the group"
                )
            if not prov.refresh_times():
                raise ValueError(
                    f"member {key!r} has no enumerable schedule (empty "
                    "refresh_times(): give it a run window or SourceTemporal.end)"
                )
            self._members[key] = prov
        # Byte-wise string order: the order a multi-member failure is reported in.
        self._keys: List[str] = sorted(self._members)
        self._depth = _positive("depth", depth)
        self._workers = _positive("workers", workers)
        if max_bytes is not None:
            max_bytes = operator.index(max_bytes)
            if max_bytes < 0:
                raise ValueError(f"max_bytes must be >= 0, got {max_bytes}")
        self._max_bytes: Optional[int] = max_bytes
        if fetch_ahead is None:
            fetch_ahead = self._depth
        fetch_ahead = operator.index(fetch_ahead)
        if fetch_ahead < 0:
            raise ValueError(f"fetch_ahead must be >= 0, got {fetch_ahead}")
        self._fetch_ahead = fetch_ahead
        if transform is not None and not callable(transform):
            raise TypeError("transform must be callable as transform(key, data)")
        self._transform = transform
        if executor not in ("thread", "process"):
            raise ValueError(f"executor must be 'thread' or 'process', got {executor!r}")
        self._executor = executor
        self._latency = _decode_latency

        self._windows: Optional[List[Any]] = None
        self._union: Optional[_WindowUnion] = None
        if windows is not None:
            self._windows = list(windows)
            for key in self._keys:
                for w in self._windows:
                    self._members[key]._window_select(w)  # raises the with_window error
            if self._windows:
                self._union = _WindowUnion(self._windows)
        self._no_windows = self._windows is not None and not self._windows

        bounds = set()
        for prov in self._members.values():
            bounds.update(prov.refresh_times())
        self._B: List[_dt.datetime] = sorted(bounds)

        select = self._union.select if self._union is not None else None
        self._files = {k: _FileShare(p, select) for k, p in self._members.items()}

        self._cv = threading.Condition(threading.RLock())
        self._state = _IDLE
        self._gen = 0
        self._order: List[int] = []
        self._head = 0
        self._next = 0
        self._stop: Optional[int] = None
        self._slots: Dict[int, _Slot] = {}
        self._tasks: Dict[Tuple[str, _dt.datetime], _Task] = {}
        self._last: Dict[str, _Task] = {}
        self._prev_anchor: Dict[str, _dt.datetime] = {}
        self._size: Dict[str, int] = {}
        self._futures: set = set()
        self._warmed: set = set()
        self._stats: Dict[str, Any] = {
            "waited_s": 0.0, "decode_s": 0.0, "peak_bytes": 0, "epochs_decoded": 0,
            "samples_decoded": 0, "samples_reused": 0, "oversize_epochs": 0,
            "overlap": True,
        }

        if executor == "process":
            try:
                payload = pickle.dumps(
                    (self._members, self._windows, transform, _decode_latency)
                )
            except Exception as exc:
                raise TypeError(
                    "executor='process' decodes in worker processes, so its "
                    "members (their loaders' url callables included) and "
                    f"transform must pickle: {type(exc).__name__}: {exc}"
                ) from exc
            self._pool: _cf.Executor = _cf.ProcessPoolExecutor(
                max_workers=self._workers,
                mp_context=_mp.get_context("spawn"),
                initializer=_proc_init,
                initargs=(payload,),
            )
            # Results come back on the pool's management thread; resubmitting
            # from there can deadlock the pool, so a collector thread takes them.
            self._results: "queue.SimpleQueue" = queue.SimpleQueue()
            self._collector = threading.Thread(
                target=self._collect, name="earthsciio-prefetch-collect", daemon=True
            )
            self._collector.start()
        else:
            self._pool = _cf.ThreadPoolExecutor(
                max_workers=self._workers, thread_name_prefix="earthsciio-prefetch"
            )
        self._warm_pool: Optional[_cf.ThreadPoolExecutor] = None

    # -- the consumer's API ------------------------------------------------- #

    def boundaries(self) -> List[_dt.datetime]:
        """The sorted, de-duplicated union of the members' ``refresh_times``.
        Epoch ``k`` is ``[B[k], B[k+1])``; the last is open on the right."""
        return list(self._B)

    def start(self, order: Any = "forward", from_: Optional[int] = None) -> None:
        """Declare the order epochs will be taken in, cancelling any previous one.

        ``order`` is ``"forward"`` (``0 … n-1``), ``"reverse"`` (``n-1 … 0``, the
        adjoint's sweep) or an explicit list of epochs (a revolve schedule;
        repeats legal). ``from_`` trims the order to begin at its first entry
        equal to ``from_`` — ``start("forward", from_=r)`` resumes a restart.
        (The spec spells it ``from``, a Python keyword.) An empty list, an
        out-of-range epoch or a ``from_`` absent from the order is a
        :class:`ValueError`, raised before the previous order is touched.
        """
        seq = self._resolve_order(order, from_)
        with self._cv:
            if self._state == _CLOSED:
                raise RuntimeError("the PrefetchGroup is closed")
            self._end_order_locked(_IDLE)
            self._order = seq
            self._head = 0
            self._next = 0
            self._stop = None
            self._prev_anchor.clear()
            self._warmed.clear()
            self._state = _RUNNING
            todo = self._admit_locked()
        self._submit(todo)

    def take(self, k: int) -> EpochSample:
        """Block until epoch ``k`` — the next entry of the order — is decoded,
        and return it.

        Asking for any other epoch is a :class:`ValueError` naming the expected
        one, and consumes nothing. A decode error stored for this position is
        raised HERE, as the exception ``refresh`` would raise; the order is then
        over (``start(order, from_=k + 1)`` resumes). Taking before ``start``,
        after ``cancel``, after a raised error or past the end of the order is a
        :class:`RuntimeError`.
        """
        k = operator.index(k)
        with self._cv:
            self._check_takeable_locked()
            pos = self._head
            expected = self._order[pos]
            if k != expected:
                raise ValueError(
                    f"take({k}) is out of order: the next epoch of the current "
                    f"order is {expected} (position {pos} of {len(self._order)}); "
                    "call start() again to re-seek"
                )
            slot = self._slots[pos]  # the head slot is always admitted
            if not slot.done:
                t0 = time.perf_counter()
                while not slot.done and self._state == _RUNNING:
                    self._cv.wait()
                self._stats["waited_s"] += time.perf_counter() - t0
                if not slot.done:
                    raise RuntimeError("the PrefetchGroup was cancelled during take()")
            self._head += 1
            del self._slots[pos]
            if slot.error is not None:
                err = slot.error
                for task in slot.tasks.values():
                    self._release_locked(task)
                self._end_order_locked(_FAILED)
                raise err
            samples: Dict[str, Sample] = {}
            for key in self._keys:
                task = slot.tasks[key]
                changed = self._prev_anchor.get(key) != task.anchor
                self._prev_anchor[key] = task.anchor
                samples[key] = Sample(task.data, task.anchor, changed)
                self._release_locked(task)
                self._last[key] = task
            ep = EpochSample(k, self._B[k], samples)
            todo = self._admit_locked()
        self._submit(todo)
        return ep

    def ready(self, k: int) -> bool:
        """Whether the next occurrence of epoch ``k`` in the order is decoded (or
        failed — its error is still raised only by :meth:`take`). Never blocks,
        never raises for a decode error; ``False`` before ``start`` or after
        ``cancel``."""
        k = operator.index(k)
        with self._cv:
            if self._state != _RUNNING:
                return False
            for pos in range(self._head, self._next):
                if self._order[pos] == k:
                    return self._slots[pos].done
            return False

    def cancel(self, wait: bool = False) -> None:
        """Drop queued work at once. A decode already running (inside a C
        library, it cannot be interrupted) finishes and its result is discarded;
        ``wait=True`` returns only after those decodes finish. :meth:`take` is an
        error until the next :meth:`start`."""
        with self._cv:
            futures = list(self._futures)
            if self._state in (_RUNNING, _FAILED):
                self._end_order_locked(_CANCELLED)
        if wait and futures:
            _cf.wait(futures)

    def close(self) -> None:
        """``cancel(wait=True)``, then join the workers; the group is unusable
        afterwards. Idempotent."""
        with self._cv:
            if self._state == _CLOSED:
                return
        self.cancel(wait=True)
        with self._cv:
            self._state = _CLOSED
        self._pool.shutdown(wait=True, cancel_futures=True)
        if self._warm_pool is not None:
            self._warm_pool.shutdown(wait=True, cancel_futures=True)
        if self._executor == "process":
            self._results.put(None)
            self._collector.join()
        for share in self._files.values():
            share.clear()
        self._last.clear()

    def stats(self) -> Dict[str, Any]:
        """Cumulative statistics since construction (``start`` does not reset
        them): ``waited_s``, ``decode_s``, ``peak_bytes``, ``epochs_decoded``,
        ``samples_decoded``, ``samples_reused``, ``oversize_epochs``,
        ``overlap`` (§3.9)."""
        with self._cv:
            return dict(self._stats)

    def __enter__(self) -> "PrefetchGroup":
        return self

    def __exit__(self, *exc: Any) -> None:
        self.close()

    # -- internals: order + state ------------------------------------------- #

    def _resolve_order(self, order: Any, from_: Optional[int]) -> List[int]:
        n = len(self._B)
        if isinstance(order, str):
            if order == "forward":
                seq = list(range(n))
            elif order == "reverse":
                seq = list(range(n - 1, -1, -1))
            else:
                raise ValueError(
                    f"order must be 'forward', 'reverse' or a list of epochs, got {order!r}"
                )
        else:
            try:
                seq = [_epoch_index(e) for e in order]
            except TypeError as exc:
                raise TypeError(
                    f"order must be 'forward', 'reverse' or a list of epoch indices: {exc}"
                ) from None
            if not seq:
                raise ValueError("an explicit order must name at least one epoch")
            bad = sorted({e for e in seq if not 0 <= e < n})
            if bad:
                raise ValueError(f"epochs {bad} are out of range: the group has {n} epochs (0..{n - 1})")
        if from_ is not None:
            f = _epoch_index(from_)
            try:
                seq = seq[seq.index(f):]
            except ValueError:
                raise ValueError(f"from_={f} is not an epoch of the order") from None
        return seq

    def _check_takeable_locked(self) -> None:
        state = self._state
        if state == _RUNNING and self._head < len(self._order):
            return
        if state == _CLOSED:
            raise RuntimeError("the PrefetchGroup is closed")
        if state == _IDLE:
            raise RuntimeError("take() before start(): declare an order with start() first")
        if state == _CANCELLED:
            raise RuntimeError("take() after cancel(): call start() to declare a new order")
        if state == _FAILED:
            raise RuntimeError(
                "the order ended at an epoch whose decode failed (take() raised "
                "its error); call start() again — start(order, from_=k + 1) "
                "resumes past it"
            )
        raise RuntimeError(
            f"the order is exhausted (all {len(self._order)} entries taken); call "
            "start() again"
        )

    def _end_order_locked(self, state: str) -> None:
        """Forget the current order: queued decodes are cancelled, running ones
        are discarded when they finish (their generation is stale)."""
        self._gen += 1
        for task in self._tasks.values():
            if task.future is not None and not task.done:
                task.future.cancel()
        self._slots.clear()
        self._tasks.clear()
        self._state = state
        self._cv.notify_all()

    def _release_locked(self, task: _Task) -> None:
        task.refs -= 1
        if task.refs <= 0 and self._tasks.get((task.key, task.anchor)) is task:
            del self._tasks[(task.key, task.anchor)]

    # -- internals: admission (spec/prefetch.md §3.6) ----------------------- #

    def _held_bytes_locked(self, in_flight: bool) -> Tuple[int, bool]:
        """Bytes of the samples the held slots reference — decoded at their real
        size, in flight (if asked) at their member's estimate — and whether any
        in-flight size is still unknown."""
        total, unknown = 0, False
        for task in self._tasks.values():
            if task.done:
                total += task.nbytes
            elif in_flight:
                est = self._size.get(task.key)
                if est is None:
                    unknown = True
                else:
                    total += est
        return total, unknown

    def _admit_locked(self) -> List[_Task]:
        """Admit slots, in order, while depth and budget allow; return the new
        tasks for the caller to submit once the lock is released."""
        todo: List[_Task] = []
        if self._state != _RUNNING:
            return todo
        while (self._stop is None and self._next < len(self._order)
               and self._next < self._head + self._depth):
            pos = self._next
            epoch = self._order[pos]
            t = self._B[epoch]
            need: Dict[str, Tuple[_dt.datetime, Optional[_Task]]] = {}
            est, unknown = 0, False
            for key in self._keys:
                anchor = self._members[key]._anchor_of(t)
                task = self._tasks.get((key, anchor))
                if task is None:
                    last = self._last.get(key)
                    if last is not None and last.anchor == anchor:
                        task = last
                        est += last.nbytes
                    elif key in self._size:
                        est += self._size[key]
                    else:
                        unknown = True
                need[key] = (anchor, task)
            if pos != self._head and self._max_bytes is not None:
                held, held_unknown = self._held_bytes_locked(in_flight=True)
                if unknown or held_unknown:
                    if self._slots:
                        break
                elif held + est > self._max_bytes:
                    break
            slot = _Slot(pos, epoch)
            for key in self._keys:
                anchor, task = need[key]
                if task is None:
                    task = _Task(key, anchor, t, self._gen)
                    self._tasks[(key, anchor)] = task
                    todo.append(task)
                else:
                    if (key, anchor) not in self._tasks:
                        self._tasks[(key, anchor)] = task  # the delivered one, revived
                    self._stats["samples_reused"] += 1
                task.refs += 1
                slot.tasks[key] = task
            self._slots[pos] = slot
            self._next += 1
            self._check_slot_locked(slot)
        self._note_peak_locked()
        self._fetch_ahead_locked()
        return todo

    def _check_slot_locked(self, slot: _Slot) -> None:
        if slot.done or not all(t.done for t in slot.tasks.values()):
            return
        slot.done = True
        failed = [slot.tasks[k] for k in self._keys if slot.tasks[k].error is not None]
        if failed:
            # The first failure by key order, and nothing behind it is admitted.
            slot.error = failed[0].error
            if self._stop is None or slot.pos < self._stop:
                self._stop = slot.pos
        else:
            self._stats["epochs_decoded"] += 1
            if self._max_bytes is not None:
                size = sum(t.nbytes for t in {id(t): t for t in slot.tasks.values()}.values())
                if size > self._max_bytes:
                    self._stats["oversize_epochs"] += 1
        self._cv.notify_all()

    def _note_peak_locked(self) -> None:
        held, _ = self._held_bytes_locked(in_flight=False)
        if held > self._stats["peak_bytes"]:
            self._stats["peak_bytes"] = held

    # -- internals: workers -------------------------------------------------- #

    def _submit(self, tasks: List[_Task]) -> None:
        """Hand new tasks to the pool — outside the lock, so a worker finishing
        meanwhile never waits on the consumer."""
        for task in tasks:
            try:
                if self._executor == "process":
                    fut = self._pool.submit(_proc_run, task.key, task.t)
                else:
                    fut = self._pool.submit(self._run_thread, task)
            except RuntimeError as exc:
                # The pool is shut down (close() raced a finishing worker) or
                # broken: record it so a take() waiting on this task raises it
                # rather than blocking — a stale task is discarded anyway.
                self._finish(task, None, 0, exc, 0.0)
                continue
            with self._cv:
                if task.gen != self._gen:
                    fut.cancel()
                else:
                    task.future = fut
                self._futures.add(fut)
            fut.add_done_callback(self._make_done(task))

    def _make_done(self, task: _Task) -> Callable[[_cf.Future], None]:
        def done(fut: _cf.Future) -> None:
            with self._cv:
                self._futures.discard(fut)
            if self._executor == "process" and not fut.cancelled():
                self._results.put((task, fut))
        return done

    def _run_thread(self, task: _Task) -> None:
        with self._cv:
            if task.gen != self._gen:
                return  # its order was cancelled before it started
        if self._latency is not None:
            time.sleep(max(0.0, float(self._latency())))
        t0 = time.perf_counter()
        try:
            data, nbytes = _produce(self._members[task.key], self._files[task.key],
                                    self._union, self._no_windows, self._transform,
                                    task.key, task.t)
            err: Optional[BaseException] = None
        except Exception as exc:
            data, nbytes, err = None, 0, exc
        self._finish(task, data, nbytes, err, time.perf_counter() - t0)

    def _collect(self) -> None:
        while True:
            item = self._results.get()
            if item is None:
                return
            task, fut = item
            try:
                data, nbytes, err, secs = fut.result()
            except BaseException as exc:  # a broken pool, an unpicklable result
                data, nbytes, err, secs = None, 0, exc, 0.0
            if err is None:
                _freeze(data)
            self._finish(task, data, nbytes, err, secs)

    def _finish(self, task: _Task, data: Any, nbytes: int,
                err: Optional[BaseException], seconds: float) -> None:
        with self._cv:
            self._stats["decode_s"] += seconds
            if task.gen != self._gen or task.done:
                return  # discarded: the order it was decoded for is gone
            task.done = True
            task.data = data
            task.error = err
            task.nbytes = nbytes
            if err is None:
                self._stats["samples_decoded"] += 1
                self._size[task.key] = nbytes
            for slot in list(self._slots.values()):
                if any(t is task for t in slot.tasks.values()):
                    self._check_slot_locked(slot)
            todo = self._admit_locked()
            self._cv.notify_all()
        self._submit(todo)

    # -- internals: fetch-ahead --------------------------------------------- #

    def _fetch_ahead_locked(self) -> None:
        """Warm the disk cache for the ``fetch_ahead`` slots past the decode
        window. Downloads only; errors dropped; never blocks."""
        if self._fetch_ahead == 0 or self._stop is not None:
            return
        urls: List[Tuple[Provider, str]] = []
        end = min(len(self._order), self._next + self._fetch_ahead)
        for pos in range(self._next, end):
            t = self._B[self._order[pos]]
            for key in self._keys:
                prov = self._members[key]
                if getattr(prov._reader, "store_backed", False) or prov.cache.offline:
                    continue
                for url in prov._sample_urls(t):
                    if url not in self._warmed:
                        self._warmed.add(url)
                        urls.append((prov, url))
        if not urls:
            return
        if self._warm_pool is None:
            self._warm_pool = _cf.ThreadPoolExecutor(
                max_workers=1, thread_name_prefix="earthsciio-fetch-ahead"
            )
        for prov, url in urls:
            self._warm_pool.submit(_warm, prov, url)


def _warm(provider: Provider, url: str) -> None:
    try:
        provider._fetch(url)
    except Exception:
        pass  # best-effort: the decode raises the real error at take()


def _positive(name: str, value: Any) -> int:
    value = operator.index(value)
    if value < 1:
        raise ValueError(f"{name} must be >= 1, got {value}")
    return value


def _epoch_index(e: Any) -> int:
    if isinstance(e, bool):
        raise TypeError(f"an epoch is an integer index, got {e!r}")
    return operator.index(e)
