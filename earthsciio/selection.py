"""The ``select`` vocabulary — ONE spelling, shared by every reader.

A ``select`` names its axes in one of two ways (``spec/registries.md`` §2.2,
"Two spellings, one vocabulary"):

* **positionally** — ``{"axes": [<axis>, ...]}`` (a bare list is accepted as
  shorthand), one entry per array dimension in the array's own dimension order,
  applied to the arrays whose rank equals the axis count;
* **by dimension name** — ``{"dims": {"<dim>": <axis>, ...}}``
  (``spec/prefetch.md`` §2.1), applied to every field and every coordinate that
  carries the dimension, whatever its rank; an unnamed dimension is ``"all"``.

Exactly one of the two keys: a ``select`` carrying both is an error, because
there is no rule for which would win and either answer is a silently different
window. Each ``<axis>`` is ``"all"``, ``{"indices": [...]}`` (an explicit,
possibly non-contiguous, ordered index list, returned in the order given) or
``{"slice": [start, stop, step?]}`` (half-open, ``step`` defaults to 1).
**Indices are 0-based**, in every reader and every track.

Two rules the three tracks must not drift on, so they live with the helpers:

* every resolved index is bounds-checked — ``0 <= i < dim_len`` — for a
  ``slice`` exactly as for an ``indices`` list. An over-long or negative
  ``[start, stop)`` is an error, never a silent clamp and never a negative
  wrap-around;
* an axis may resolve to NOTHING (``{"indices": []}``, or an empty half-open
  ``{"slice": [1, 1]}``). That is legal and yields a **zero-length axis**, kept
  in ``dims``: a length-0 array of the right rank, not an error and not a
  dropped dimension. (Selecting a single index likewise KEEPS the axis, at
  length 1 — this vocabulary never drops a dimension.)

These helpers live here rather than in one backend because two readers now parse
the same vocabulary and must not drift apart: the store-backed
:class:`~earthsciio.backends.zarr.ZarrReader`, whose selection decides which chunk
OBJECTS are fetched, and the whole-file
:class:`~earthsciio.readers.NetCDFReader`, which fetches the same blob under the
same cache key and materialises only the requested hyperslab. What a selection
SAVES differs; what it MEANS does not.

The module also holds :class:`DimWindow` (the convenience constructor for the
named form, ``spec/prefetch.md`` §2.2) and :class:`_WindowUnion`, the bounding
union :meth:`~earthsciio.provider.Provider.sample_windows_at` decodes once and
slices K windows back out of (§2.4) — both are pure vocabulary, no I/O.
"""

from __future__ import annotations

from typing import Any, Dict, Iterable, List, Mapping, Optional, Sequence, Tuple

import numpy as np

from .native import NativeDataset, NativeField

__all__ = [
    "DimWindow",
    "_ALL",
    "_parse_axis",
    "_select_axes",
    "_select_dims",
    "_resolve_axis_indices",
    "_absent_dims_error",
    "_as_dims_select",
    "_WindowUnion",
]

_ALL = ("all",)


def _parse_axis(spec: Any) -> Tuple:
    """Normalize one axis selector to a tagged tuple.

    Accepts ``"all"``/``None`` (whole axis), ``{"indices": [...]}`` (an explicit,
    possibly non-contiguous, ordered index list), ``{"slice": [start, stop,
    step?]}`` (a strided range, ``step`` defaults to 1), or a bare list of ints
    (shorthand for ``indices``).
    """
    if spec is None or spec == "all":
        return _ALL
    if isinstance(spec, dict):
        if "indices" in spec:
            return ("indices", [int(i) for i in spec["indices"]])
        if "slice" in spec:
            s = list(spec["slice"])
            start = int(s[0])
            stop = int(s[1])
            step = int(s[2]) if len(s) > 2 else 1
            return ("slice", start, stop, step)
        raise ValueError(f"unrecognized axis selector: {spec!r}")
    if isinstance(spec, (list, tuple)):
        return ("indices", [int(i) for i in spec])
    raise ValueError(f"unrecognized axis selector: {spec!r}")


def _check_one_form(select: Mapping) -> None:
    if "axes" in select and "dims" in select:
        raise ValueError(
            "select carries both `axes` and `dims`; a select names its axes "
            "positionally ({'axes': [...]}) OR by dimension name "
            "({'dims': {...}}), never both — there is no rule for which would "
            "win (spec/prefetch.md §2.1)"
        )


def _select_axes(select: Any) -> Optional[List[Any]]:
    """Extract the ordered per-axis selector list from a ``select`` argument.

    ``None`` ⇒ ``None`` (all). A ``{"axes": [...]}`` mapping or a bare list both
    yield the axis list. The named form ``{"dims": {...}}`` is not positional,
    so it yields ``None`` here (see :func:`_select_dims`); a mapping carrying
    BOTH keys is a :class:`ValueError`. Anything else ⇒ ``None`` (all).
    """
    if select is None:
        return None
    if isinstance(select, Mapping):
        _check_one_form(select)
        if "axes" in select:
            return list(select["axes"])
        return None
    if isinstance(select, (list, tuple)):
        return list(select)
    return None


def _select_dims(select: Any) -> Optional[Dict[str, Tuple]]:
    """The named form of ``select``, parsed: ``{dim: tagged selector}`` in the
    order given, or ``None`` when ``select`` is not the named form.

    ``{"dims": {}}`` is legal and selects nothing away (every dimension stays
    ``"all"``), so it parses to an EMPTY map, not ``None`` — a reader still runs
    the named path, and an empty window is still a window.
    """
    if not isinstance(select, Mapping) or "dims" not in select:
        return None
    _check_one_form(select)
    dims = select["dims"]
    if not isinstance(dims, Mapping):
        raise ValueError(
            "select['dims'] must map dimension names to selectors, got "
            f"{type(dims).__name__}"
        )
    return {str(d): _parse_axis(s) for d, s in dims.items()}


def _absent_dims_error(
    absent: Sequence[str], present: Iterable[str], hint: str = ""
) -> ValueError:
    """The error for a named dimension no field of the read carries.

    Named in one place so the netcdf and zarr readers say the same thing: a
    window that silently meant nothing is the failure this rule exists to stop
    (``spec/prefetch.md`` §2.1), and the overwhelmingly common cause is a
    near-miss spelling, so the message lists what IS there.
    """
    which = ", ".join(repr(d) for d in absent)
    return ValueError(
        f"select names dimension(s) {which}, which no field of this read "
        f"carries; the dimensions present are {sorted(set(present))}. A named "
        "window must name a dimension the decoded fields (after the `variables` "
        f"projection) or their coordinates carry.{hint}"
    )


def _resolve_axis_indices(axis: Tuple, dim_len: int) -> List[int]:
    """Resolve a tagged axis selector to its ordered list of global indices."""
    if axis[0] == "all":
        return list(range(dim_len))
    if axis[0] == "indices":
        idxs = axis[1]
        for g in idxs:
            if g < 0 or g >= dim_len:
                raise IndexError(f"index {g} out of range for dimension length {dim_len}")
        return list(idxs)
    if axis[0] == "slice":
        _, start, stop, step = axis
        if step < 1:
            raise ValueError(f"slice step must be >= 1, got {step}")
        out = list(range(start, stop, step))
        # A slice is bounds-checked exactly like an explicit `indices` list: a
        # `[start, stop)` reaching past the dimension (or a negative `start`) is
        # an ERROR, never a silent clamp and never a negative wrap-around. The
        # three tracks cannot agree on a clamp — numpy clamps, NCDatasets and the
        # `netcdf-reader` slice API do not — and an over-long window that quietly
        # returns fewer cells than asked for is a wrong number.
        for g in out:
            if g < 0 or g >= dim_len:
                raise IndexError(
                    f"slice [{start}, {stop}) by {step} reaches index {g}, out of "
                    f"range for dimension length {dim_len}"
                )
        return out
    raise ValueError(f"unrecognized axis selector: {axis!r}")


def _axis_indices_unbounded(axis: Tuple) -> Optional[List[int]]:
    """An axis selector's index list WITHOUT the dimension length, or ``None``
    for ``"all"``.

    A ``slice`` is ``range(start, stop, step)`` whatever the axis length, so a
    window union can be formed before anything is read. Bounds are NOT checked
    here: the read that consumes the union checks them against the file, and
    :meth:`_WindowUnion.split` re-checks a window the union widened to ``"all"``.
    """
    if axis[0] == "all":
        return None
    if axis[0] == "indices":
        return list(axis[1])
    if axis[0] == "slice":
        _, start, stop, step = axis
        if step < 1:
            raise ValueError(f"slice step must be >= 1, got {step}")
        return list(range(start, stop, step))
    raise ValueError(f"unrecognized axis selector: {axis!r}")


# --------------------------------------------------------------------------- #
# DimWindow — the named form's convenience constructor (spec/prefetch.md §2.2).
# --------------------------------------------------------------------------- #


def _wire_axis(dim: str, spec: Any) -> Any:
    """One ``DimWindow`` entry → its wire selector.

    A 2- or 3-tuple is a ``slice``, a list of ints is ``indices``, ``"all"`` and
    selector dicts pass through. The tuple/list split is the whole point of the
    constructor: on the wire both are lists, and a ``[120, 218]`` meaning "two
    cells" where ``(120, 218)`` meant "98 cells" is exactly the mix-up a typed
    constructor exists to remove.
    """
    if isinstance(spec, str):
        if spec != "all":
            raise ValueError(f"DimWindow[{dim!r}]: the only string selector is 'all', got {spec!r}")
        return "all"
    if isinstance(spec, tuple):
        if len(spec) not in (2, 3):
            raise ValueError(
                f"DimWindow[{dim!r}]: a tuple is a slice (start, stop[, step]), got "
                f"{len(spec)} elements"
            )
        return {"slice": [int(x) for x in spec]}
    if isinstance(spec, list):
        return {"indices": [int(i) for i in spec]}
    if isinstance(spec, Mapping):
        if "slice" in spec and len(spec["slice"]) not in (2, 3):
            raise ValueError(
                f"DimWindow[{dim!r}]: a slice is [start, stop] or [start, stop, "
                f"step], got {spec!r}"
            )
        tagged = _parse_axis(dict(spec))
        if tagged[0] == "indices":
            return {"indices": list(tagged[1])}
        if tagged[0] == "slice":
            return {"slice": [int(x) for x in spec["slice"]]}
        return "all"
    raise ValueError(
        f"DimWindow[{dim!r}]: expected 'all', a (start, stop[, step]) tuple, a "
        f"list of indices or a selector dict, got {type(spec).__name__}"
    )


class DimWindow:
    """A window that names its dimensions (``spec/prefetch.md`` §2.2).

    ``DimWindow({"lat": (120, 218), "lon": [1151, 0, 1], "lev": "all"})`` — a 2-
    or 3-tuple is a half-open ``slice`` ``(start, stop[, step])``, a list of ints
    is ``indices`` (kept in the order given, repeats included — a window across a
    periodic longitude seam is ``[1150, 1151, 0, 1]``), and ``"all"`` and wire
    selector dicts pass through. All indices are **0-based**, the wire
    vocabulary. :attr:`select` is the wire form ``{"dims": {...}}``.

    The name is ``DimWindow`` because :data:`earthsciio.Window` is already the
    run window ``(start, end)``. Wherever a ``DimWindow`` is accepted the wire
    dict is accepted too. Instances are immutable; construction validates the
    selectors' SHAPE, while bounds are checked against the file at read time.
    """

    __slots__ = ("_dims",)

    def __init__(self, dims: Optional[Mapping[str, Any]] = None) -> None:
        if dims is not None and not isinstance(dims, Mapping):
            raise TypeError(
                "DimWindow takes a mapping of dimension name -> selector, got "
                f"{type(dims).__name__}"
            )
        self._dims: Dict[str, Any] = {
            str(d): _wire_axis(str(d), s) for d, s in (dims or {}).items()
        }

    @classmethod
    def from_select(cls, select: Mapping[str, Any]) -> "DimWindow":
        """The window a wire ``{"dims": {...}}`` select spells."""
        named = _select_dims(select)
        if named is None:
            raise ValueError(
                "a window is the named form {'dims': {...}}; got "
                f"{sorted(map(str, select)) if isinstance(select, Mapping) else select!r}"
            )
        return cls(dict(select["dims"]))

    @property
    def dims(self) -> Dict[str, Any]:
        """``{dim: wire selector}`` — a copy, in the order given."""
        return {d: _copy_wire(s) for d, s in self._dims.items()}

    @property
    def select(self) -> Dict[str, Any]:
        """The wire form, ``{"dims": {...}}`` — a fresh dict each call."""
        return {"dims": self.dims}

    def __eq__(self, other: object) -> bool:
        if isinstance(other, DimWindow):
            return self._dims == other._dims
        return NotImplemented

    def __hash__(self) -> int:
        return hash(repr(self))

    def __repr__(self) -> str:
        return f"DimWindow({self._dims!r})"

    def __getstate__(self) -> Dict[str, Any]:
        return {"dims": self._dims}

    def __setstate__(self, state: Dict[str, Any]) -> None:
        self._dims = state["dims"]


def _copy_wire(s: Any) -> Any:
    if isinstance(s, dict):
        return {k: list(v) for k, v in s.items()}
    return s


def _as_dims_select(w: Any) -> Dict[str, Any]:
    """A window argument (:class:`DimWindow` or the wire dict) → the wire dict.

    A window NAMES its dimensions; a positional ``{"axes": [...]}`` cannot be a
    window, because its meaning depends on the rank of the field it lands on —
    the very thing a window spanning 2-D, 3-D and staggered fields must not
    depend on. Bake a positional select through ``reader_kwargs`` instead.
    """
    if isinstance(w, DimWindow):
        return w.select
    if isinstance(w, Mapping):
        if "dims" in w:
            return DimWindow.from_select(w).select
        if "axes" in w:
            raise ValueError(
                "a window names its dimensions ({'dims': {...}} or a DimWindow); "
                "a positional {'axes': [...]} select depends on each field's rank "
                "and cannot be one — bake it through reader_kwargs['select']"
            )
    raise TypeError(
        "expected a DimWindow or a wire select {'dims': {...}}, got "
        f"{type(w).__name__}"
    )


# --------------------------------------------------------------------------- #
# The bounding union of K windows (spec/prefetch.md §2.4).
# --------------------------------------------------------------------------- #


class _WindowUnion:
    """K named windows, their bounding union, and the slicing back.

    Per dimension any window names, the union is the SORTED DISTINCT union of
    the windows' indices — or ``"all"`` when some window leaves that dimension
    unnamed or names it ``"all"`` (that window needs the whole axis anyway).
    Every named dimension stays in the union, ``"all"`` included, so the read
    still refuses a name no field carries: what a window alone would raise, the
    union raises too.

    :meth:`split` slices window ``i`` back out of a dataset decoded under
    :attr:`select`, BY NAME, restoring the window's own order and repetitions.
    It is a pure gather from values the decode already produced — no value is
    computed twice or combined — so each result is bitwise what decoding that
    window alone would give.
    """

    def __init__(self, windows: Sequence[Any]) -> None:
        self.selects: List[Dict[str, Any]] = [_as_dims_select(w) for w in windows]
        self._parsed: List[Dict[str, Tuple]] = [
            _select_dims(s) or {} for s in self.selects
        ]
        self._wanted: List[Dict[str, Optional[List[int]]]] = [
            {d: _axis_indices_unbounded(a) for d, a in p.items()} for p in self._parsed
        ]
        names: List[str] = []
        for p in self._parsed:
            for d in p:
                if d not in names:
                    names.append(d)
        union: Dict[str, Optional[List[int]]] = {}
        for d in names:
            lists = [w.get(d) for w in self._wanted]  # unnamed and "all" are both None
            if any(lst is None for lst in lists):
                union[d] = None  # some window needs the whole axis
            else:
                union[d] = sorted({i for lst in lists for i in lst})
        self._union = union
        self.select: Dict[str, Any] = {
            "dims": {
                d: ("all" if idx is None else {"indices": list(idx)})
                for d, idx in union.items()
            }
        }

    def names(self) -> List[str]:
        """Every dimension some window names."""
        return list(self._union)

    def split(self, ds: NativeDataset) -> List[NativeDataset]:
        """Window ``i`` of ``ds`` (decoded under :attr:`select`), for every ``i``."""
        return [self._one(ds, wanted) for wanted in self._wanted]

    def _one(self, ds: NativeDataset, wanted: Dict[str, Optional[List[int]]]) -> NativeDataset:
        for dim, want in wanted.items():
            if want is not None and self._union[dim] is None and _is_time_dim(ds, dim):
                # The union read this dimension whole for some OTHER window, so
                # the reader never saw this window's subset of it: refuse it
                # here, as the read alone would have.
                raise ValueError(
                    f"select asks for a subset of the time dimension {dim!r}; record "
                    "selection is the Provider's (it owns the cadence: "
                    "records_per_sample), not the reader's — a time dimension may "
                    'only be "all"'
                )

        def cut(f: NativeField) -> NativeField:
            data = f.data
            for axis, dim in enumerate(f.dims):
                want = wanted.get(dim)
                if want is None:
                    continue  # unnamed or "all": the union already holds this window's axis
                have = self._union[dim]
                if have is None:
                    # The union widened this dimension to the whole axis for some
                    # OTHER window, so the read never bounds-checked these
                    # indices: check them here, against the decoded (= file)
                    # length, exactly as the read would have.
                    pos = _resolve_axis_indices(("indices", want), int(np.shape(data)[axis]))
                else:
                    at = {g: p for p, g in enumerate(have)}
                    pos = [at[g] for g in want]
                data = np.take(np.asarray(data), np.asarray(pos, dtype=np.intp), axis=axis)
            if data is f.data:
                return f
            return NativeField(data, f.dims, f.attrs)

        return NativeDataset(
            {n: cut(f) for n, f in ds.variables.items()},
            {n: cut(f) for n, f in ds.coords.items()},
        )


def _is_time_dim(ds: NativeDataset, dim: str) -> bool:
    """Is ``dim`` the decoded dataset's time axis? A same-named coordinate with
    CF ``"<step> since <ref>"`` units, or a dimension literally named ``time`` —
    the union of the readers' rules, since only a refusal rests on it."""
    coord = ds.coords.get(dim)
    if coord is not None:
        units = coord.attrs.get("units")
        if units and "since" in str(units).lower().split():
            return True
    return str(dim).lower() == "time"
