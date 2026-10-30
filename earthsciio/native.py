"""Native-grid array containers — the cross-language NATIVE-ARRAY contract.

A format reader decodes a cached blob into a :class:`NativeDataset`: data
``variables`` and grid ``coords``, each a :class:`NativeField` (an array + its
ordered on-disk dimension names + decode-relevant ``attrs``). This is the Python
realization of ``spec/schemas/native-field.schema.json`` and the peer Julia
``NativeField``/``NativeDataset`` (``julia/src/readers.jl``) and Rust
``NativeField``/``NativeDataset`` (``rust/src/format``) — so the *same* blob
decodes to *equal* arrays in all three tracks (Risk R4; conformance
``esio-9nb.9``).

The arrays are RAW native-grid values keyed by the on-disk ``file_variable``
name, exactly as the reader produced them: **no** variable-name remap and **no**
``unit_conversion`` (those stay in ESS — Risk R3; ``spec/conformance.md`` §3).
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any

__all__ = ["NativeDataset", "NativeField"]


class NativeField:
    """One native-grid array as a reader decodes it.

    Parameters
    ----------
    data:
        The values. Numeric fields are a :class:`numpy.ndarray` whose axes
        correspond, **in order**, to ``dims`` (on-disk dimension names, file
        order — e.g. ``("time", "latitude", "longitude")``); text columns are a
        plain ``list`` of :class:`str`.
    dims:
        Ordered dimension names. Row-major (C order) flattening of ``data``
        follows this order, matching the corpus encoding.
    attrs:
        Decode-relevant metadata the reader must NOT act on but ESS needs —
        notably a CF time axis's ``units``/``calendar`` (calendar decoding to
        wall-clock instants is ESS's job, never the reader's).

    Per ``spec/conformance.md`` §3: numeric fields are ``float64`` with ``NaN``
    for masked/``_FillValue`` cells; an unpacked pure-integer field keeps its
    integer dtype (e.g. a raw time axis); text columns are ``str``.
    """

    __slots__ = ("attrs", "data", "dims")

    def __init__(
        self,
        data: Any,
        dims: Sequence[str],
        attrs: Mapping[str, Any] | None = None,
    ) -> None:
        self.data = data
        self.dims: tuple[str, ...] = tuple(str(d) for d in dims)
        self.attrs: dict[str, Any] = dict(attrs) if attrs else {}

    @property
    def shape(self) -> tuple[int, ...]:
        """Length of each dimension, in ``dims`` order."""
        shp = getattr(self.data, "shape", None)
        return tuple(shp) if shp is not None else (len(self.data),)

    @property
    def dtype(self) -> Any:
        """The numpy dtype for a numeric field, or ``None`` for a string field."""
        return getattr(self.data, "dtype", None)

    def __repr__(self) -> str:  # pragma: no cover - debug aid
        kind = self.dtype if self.dtype is not None else "string"
        return f"NativeField({kind} shape={list(self.shape)} dims={list(self.dims)})"


class NativeDataset:
    """The native arrays from one blob: data ``variables`` + grid ``coords``.

    Both map a ``file_variable`` name → :class:`NativeField`. ``__getitem__``
    looks in ``variables`` then ``coords``, so ``nds["t2m"]`` and ``nds["time"]``
    both resolve (mirrors the Julia/Rust ``NativeDataset``). Coordinates are the
    dimension-coordinate fields (e.g. ``latitude``/``longitude``/``time``); the
    data variables are everything else.
    """

    __slots__ = ("coords", "variables")

    def __init__(
        self,
        variables: Mapping[str, NativeField] | None = None,
        coords: Mapping[str, NativeField] | None = None,
    ) -> None:
        self.variables: dict[str, NativeField] = dict(variables) if variables else {}
        self.coords: dict[str, NativeField] = dict(coords) if coords else {}

    def __getitem__(self, name: str) -> NativeField:
        if name in self.variables:
            return self.variables[name]
        if name in self.coords:
            return self.coords[name]
        raise KeyError(name)

    def __contains__(self, name: object) -> bool:
        return name in self.variables or name in self.coords

    def variable_names(self) -> list[str]:
        """Names of the data variables (not coordinates), sorted."""
        return sorted(self.variables)

    def coord_names(self) -> list[str]:
        """Names of the coordinate fields, sorted."""
        return sorted(self.coords)

    def __repr__(self) -> str:  # pragma: no cover - debug aid
        return f"NativeDataset(variables={self.variable_names()}, coords={self.coord_names()})"
