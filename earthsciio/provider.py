"""The cadence-aware **Provider** (component (b)) — the sanctioned impure I/O
boundary for one ESS ``DataLoader`` node.

A :class:`Provider` is bound to one loader and created once per simulation. It
resolves a URL (per cadence anchor, for a time-varying source), fetches it
through the content-addressed cache (component (a), :mod:`earthsciio.cache`),
decodes it with the loader's :data:`~earthsciio.registry.format_registry` reader,
and returns RAW native-grid arrays (:class:`~earthsciio.native.NativeDataset`).
Variable-name remap / ``unit_conversion`` stay in ESS; regrid is ESD/C4's job
(Risk R3) — the Provider returns native arrays and nothing more.

It provides DATA, not a solver. The library EXPOSES the provider and its
:meth:`Provider.refresh_times`; the **user/solver** drives the discrete-cadence
update (e.g. a ``DifferentialEquations`` ``PresetTimeCallback`` /
``diffeqpy``/SciPy event that calls :meth:`Provider.refresh` at each anchor). No
solver is embedded here. The refresh is wired to fire **once per cadence
boundary** in the solver callback — NEVER per-RHS — exactly matching campfire C1
(``ess-06y``) terminal-event segmentation; downstream is C1 (the cadence
callback) + C4 (the regrid handoff), integration verified campfire-side.

CONST vs DISCRETE (the cadence contract):

* **CONST** — ``loader.temporal is None``: :meth:`materialize` reads the single
  file once; :meth:`refresh_times` is empty; :meth:`refresh` returns that same
  constant data.
* **DISCRETE** — ``loader.temporal`` set: :meth:`refresh` snaps a time to the
  loader's cadence **anchor** and returns the matching record's native arrays;
  :meth:`refresh_times` is the cadence schedule over the run window (the solver
  tstops). A multi-record file is sliced on its ``time_dim`` axis; a file-per-tick
  source resolves a new URL per file period.

This mirrors the peer Rust ``Provider`` (``rust/src/provider.rs``) and Julia
``Provider`` (``julia/src/provider.jl``); the decoded native arrays are equal
across all three tracks (conformance ``esio-9nb.9``).
"""

from __future__ import annotations

import datetime as _dt
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Optional, Sequence, Tuple, Union

import numpy as np

from .cache import Cache, CacheEntry
from .native import NativeDataset, NativeField
from .registry import Registry, format_registry

__all__ = ["LoaderTemporal", "DataLoader", "Provider", "Window"]

#: A run window ``(start, end)`` — half-open ``[start, end)``.
Window = Tuple[_dt.datetime, _dt.datetime]

#: A URL spec: a literal URL, a ``strftime`` template (contains ``%``), or a
#: callable ``anchor -> url`` (the most general, per-file-anchor resolver).
UrlSpec = Union[str, Callable[[_dt.datetime], str]]

_EPOCH = _dt.datetime(1970, 1, 1, tzinfo=_dt.timezone.utc)
_ZERO = _dt.timedelta(0)


def _snap_down(start: _dt.datetime, t: _dt.datetime, step: _dt.timedelta) -> _dt.datetime:
    """The largest aligned anchor ``start + k*step <= t`` (``k`` an integer).

    Timedelta floor-division floors toward -infinity, so this snaps ``t`` *down*
    to its cadence anchor even before ``start`` (caller validates the range).
    """
    return start + ((t - start) // step) * step


@dataclass(frozen=True)
class LoaderTemporal:
    """The temporal cadence of a DISCRETE loader (absent ⇒ CONST).

    ``frequency`` is the cadence step (e.g. 1 hour for ERA5) — it drives the
    refresh tstops and which record within a file is current. ``file_period`` is
    the span of one file (e.g. 1 day) — it drives URL resolution; a file holds
    ``file_period / frequency`` records along ``time_dim``. Anchors are aligned
    to ``start`` (the loader epoch). ``end`` (optional) is the exclusive end of
    available data and bounds :meth:`Provider.refresh_times` when no run window
    is given.
    """

    start: _dt.datetime
    frequency: _dt.timedelta
    file_period: _dt.timedelta
    end: Optional[_dt.datetime] = None
    time_dim: str = "time"

    def __post_init__(self) -> None:
        if self.frequency <= _ZERO:
            raise ValueError(f"frequency must be positive, got {self.frequency!r}")
        if self.file_period <= _ZERO:
            raise ValueError(f"file_period must be positive, got {self.file_period!r}")


@dataclass
class DataLoader:
    """The I/O-relevant projection of an ESS ``DataLoader`` the Provider needs.

    The full ESM contract (units, variable remap, grid family) lives upstream in
    ESS; this is only what resolves bytes and decodes them.

    Parameters
    ----------
    name:
        Loader name — provenance, recorded in the cache manifest.
    format:
        Format-registry key selecting the reader (e.g. ``"netcdf"``, ``"csv"``).
    url:
        A literal URL (CONST), a ``strftime`` template resolved at each file
        anchor (e.g. ``".../era5/%Y/%m/%Y%m%d.nc"``), or a callable
        ``anchor -> url``.
    variables:
        On-disk ``file_variable`` names to read; empty ⇒ all data variables
        (coordinates are always kept).
    temporal:
        The cadence (``None`` ⇒ CONST/static).
    mirrors:
        Failover URL candidates sharing the same cache identity.
    auth_realm:
        Auth realm to fetch under (resolved by the cache's auth registry).
    reader_kwargs:
        Extra keywords forwarded to the reader's ``read_native`` (e.g. the CSV
        reader's ``numeric_columns``).
    """

    name: str
    format: str
    url: UrlSpec
    variables: Sequence[str] = ()
    temporal: Optional[LoaderTemporal] = None
    mirrors: Sequence[str] = ()
    auth_realm: Optional[str] = None
    reader_kwargs: Dict[str, object] = field(default_factory=dict)

    @property
    def is_const(self) -> bool:
        """True if the loader is time-invariant (no :class:`LoaderTemporal`)."""
        return self.temporal is None

    def resolve_url(self, anchor: _dt.datetime) -> str:
        """Resolve the file URL for a cadence/file anchor."""
        u = self.url
        if callable(u):
            return u(anchor)
        if "%" in u:
            return anchor.strftime(u)
        return u


class Provider:
    """A loader-bound provider of native-grid arrays, refreshed at the loader's
    cadence. See the module docstring for the CONST/DISCRETE contract.

    Parameters
    ----------
    loader:
        The :class:`DataLoader` this provider serves.
    cache:
        The content-addressed :class:`~earthsciio.cache.Cache` (component (a)).
    window:
        Optional run window ``(start, end)`` bounding :meth:`refresh_times` /
        :meth:`prefetch` and priming :meth:`materialize` for a DISCRETE loader.
    formats:
        The format registry to resolve the reader from (defaults to the global
        :data:`~earthsciio.registry.format_registry`) — the seam that lets a new
        format plug in with no Provider change.

    Raises :class:`~earthsciio.errors.BackendNotRegistered` if no reader is
    registered for ``loader.format``.
    """

    def __init__(
        self,
        loader: DataLoader,
        cache: Cache,
        window: Optional[Window] = None,
        *,
        formats: Optional[Registry] = None,
    ) -> None:
        self.loader = loader
        self.cache = cache
        self.window = window
        registry = formats if formats is not None else format_registry
        # Resolve (and construct) the reader by name now, so an unknown format
        # fails at construction, not mid-solve.
        self._reader = registry.create(loader.format)
        # Current state — a file-read cache so stepping within one file decodes
        # it once, and the buffer the solver reads between cadence boundaries.
        self._current: Optional[NativeDataset] = None
        self._current_anchor: Optional[_dt.datetime] = None
        self._current_file_anchor: Optional[_dt.datetime] = None
        self._current_file: Optional[NativeDataset] = None

    # -- introspection ------------------------------------------------------ #

    @property
    def is_const(self) -> bool:
        """True if the bound loader is time-invariant (CONST)."""
        return self.loader.temporal is None

    @property
    def coords(self) -> Dict[str, NativeField]:
        """Native coordinates of the current buffer (lat/lon/…); empty until the
        first :meth:`materialize`/:meth:`refresh`."""
        return self._current.coords if self._current is not None else {}

    # -- materialize / refresh --------------------------------------------- #

    def materialize(self) -> NativeDataset:
        """Materialize the loader's native arrays into the buffer and return them.

        CONST: reads the single file once. DISCRETE: primes the buffer at the
        first cadence anchor of the window (≡ ``refresh(window.start)``), so a
        caller can read an initial state before stepping.
        """
        if self.loader.temporal is None:
            ds = self._read_file(self.loader.resolve_url(_EPOCH))
            self._current = ds
            return ds
        return self.refresh(self._lower_bound())

    def refresh(self, t: _dt.datetime) -> NativeDataset:
        """Refresh the buffer to the cadence anchor for time ``t`` and return it.

        Snaps ``t`` down to the loader's cadence anchor, re-reads the covering
        file only when the **file** anchor changed (stepping within one file
        decodes it once), slices the current record on ``time_dim``, and returns
        the native arrays. For a CONST loader this returns the constant data
        (materializing once if needed). Raises :class:`ValueError` if ``t``
        precedes the loader epoch, :class:`IndexError` if the snapped record is
        absent from its file.
        """
        temporal = self.loader.temporal
        if temporal is None:
            return self._current if self._current is not None else self.materialize()

        anchor = _snap_down(temporal.start, t, temporal.frequency)
        if anchor < temporal.start:
            raise ValueError(
                f"t={t!r} precedes the loader start {temporal.start!r}"
            )
        file_anchor = _snap_down(temporal.start, anchor, temporal.file_period)
        if self._current_file is None or self._current_file_anchor != file_anchor:
            self._current_file = self._read_file(self.loader.resolve_url(file_anchor))
            self._current_file_anchor = file_anchor

        rec = (anchor - file_anchor) // temporal.frequency
        sliced = _slice_record(self._current_file, temporal.time_dim, rec)
        self._current = sliced
        self._current_anchor = anchor
        return sliced

    def refresh_times(self) -> List[_dt.datetime]:
        """The cadence anchors at which the data changes and the solver must
        :meth:`refresh` — the solver tstops.

        Empty for a CONST loader, or for an unbounded DISCRETE loader with no run
        window and no ``temporal.end``. Otherwise the aligned anchors in
        ``[lower, upper)`` where ``lower`` is the window start clamped to the
        loader epoch and ``upper`` is the window end (else ``temporal.end``).
        Each is a timezone-aware :class:`datetime.datetime`; a solver integrating
        in epoch seconds takes ``t.timestamp()``.
        """
        temporal = self.loader.temporal
        if temporal is None:
            return []
        upper = self.window[1] if self.window is not None else temporal.end
        if upper is None:
            return []  # unbounded — no enumerable schedule
        lower = self._lower_bound()
        # First aligned anchor >= lower (ceil of the elapsed step count).
        steps = -((temporal.start - lower) // temporal.frequency)
        anchor = temporal.start + steps * temporal.frequency
        out: List[_dt.datetime] = []
        while anchor < upper:
            out.append(anchor)
            anchor = anchor + temporal.frequency
        return out

    # -- prefetch ----------------------------------------------------------- #

    def prefetch(self, window: Optional[Window] = None) -> List[CacheEntry]:
        """Warm the cache for every file the provider will need, WITHOUT decoding.

        CONST: the single file. DISCRETE: each unique file covering the window
        (``window`` arg, else the provider's window). Lets a caller pull all
        blobs up front (e.g. before a solve, or while online) so later
        :meth:`materialize`/:meth:`refresh` hit a warm, offline-readable cache.
        Returns the :class:`~earthsciio.cache.CacheEntry` for each unique file.
        """
        temporal = self.loader.temporal
        if temporal is None:
            return [self._fetch(self.loader.resolve_url(_EPOCH))]

        win = window if window is not None else self.window
        upper = win[1] if win is not None else temporal.end
        if upper is None:
            raise ValueError(
                "prefetch needs a bounded window: pass window=(start, end) or set "
                "LoaderTemporal.end"
            )
        start = win[0] if (win is not None and win[0] > temporal.start) else temporal.start
        entries: List[CacheEntry] = []
        seen: set = set()
        file_anchor = _snap_down(temporal.start, start, temporal.file_period)
        while file_anchor < upper:
            url = self.loader.resolve_url(file_anchor)
            if url not in seen:
                seen.add(url)
                entries.append(self._fetch(url))
            file_anchor = file_anchor + temporal.file_period
        return entries

    # -- internals ---------------------------------------------------------- #

    def _lower_bound(self) -> _dt.datetime:
        """The effective lower bound: the window start clamped to the loader
        epoch (a DISCRETE provider always has a temporal)."""
        temporal = self.loader.temporal
        assert temporal is not None
        if self.window is not None and self.window[0] > temporal.start:
            return self.window[0]
        return temporal.start

    def _fetch(self, url: str) -> CacheEntry:
        return self.cache.fetch(
            url,
            source_loader=self.loader.name,
            auth_realm=self.loader.auth_realm,
            mirrors=tuple(self.loader.mirrors),
        )

    def _read_file(self, url: str) -> NativeDataset:
        entry = self._fetch(url)
        handle = self._reader.open(entry.path)
        variables = list(self.loader.variables) if self.loader.variables else None
        return self._reader.read_native(handle, variables, **self.loader.reader_kwargs)


def _slice_record(ds: NativeDataset, time_dim: str, rec: int) -> NativeDataset:
    """Slice cadence record ``rec`` out of every field carrying ``time_dim``.

    Non-temporal fields pass through whole; the sliced ``time_dim`` coordinate is
    dropped (its value for this record is implied by the anchor). Mirrors the
    Julia ``_slice_dim`` / Rust ``select_leading``.
    """
    variables = {name: _slice_field(f, time_dim, rec) for name, f in ds.variables.items()}
    coords = {
        name: _slice_field(f, time_dim, rec)
        for name, f in ds.coords.items()
        if name != time_dim
    }
    return NativeDataset(variables, coords)


def _slice_field(f: NativeField, time_dim: str, rec: int) -> NativeField:
    if time_dim not in f.dims:
        return f  # non-temporal field unchanged
    axis = f.dims.index(time_dim)
    length = f.data.shape[axis]
    if rec < 0 or rec >= length:
        raise IndexError(
            f"cadence record {rec} out of range for dimension {time_dim!r} "
            f"(length {length})"
        )
    sliced = np.take(f.data, rec, axis=axis)
    new_dims = tuple(d for d in f.dims if d != time_dim)
    return NativeField(sliced, new_dims, f.attrs)
