"""Building a provider from an esm ``data_sources`` entry (spec/esm-sources.md §3).

:func:`source_from_esm` reads the parts of an entry that locate and decode
files — ``source``, ``temporal``, ``reader_options`` and the reader name — into
a :class:`~earthsciio.DataSource`; :func:`provider_from_esm` binds it to a cache
and a run window. What a source's bytes MEAN (``select``, ``codes``,
``record_filter``, ``extent``, a parameter's ``unit_conversion``) is the
document binding's job (EarthSciAST), which calls this for the bytes.
"""

from __future__ import annotations

import datetime as _dt
import re
from collections.abc import Mapping, Sequence
from typing import Any

from .calendar import CalendarPeriod, CalendarPeriodError
from .errors import EarthSciIOError
from .esm import (
    EsmTemplate,
    EsmTemplateError,
    _check_esm_template,
    esm_template_names,
    expand_esm_template,
)
from .manifest import parse_rfc3339

__all__ = ["EsmSourceError", "provider_from_esm", "source_from_esm"]

_SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*://")


class EsmSourceError(EarthSciIOError, ValueError):
    """An esm data source EarthSciIO cannot read. ``code`` is one of the
    ``esm_source_*`` codes of spec/esm-sources.md §3.3, ``esm_period_invalid``,
    or an ``esm_template_*`` code for a template of the entry (the
    :class:`~earthsciio.EsmTemplateError` is its ``__cause__``). The message
    contains the code and the source's name."""

    def __init__(self, code: str, name: str, detail: str) -> None:
        self.code = code
        self.source = name
        super().__init__(f"{code}: data source {name!r}: {detail}")


def _absent(value: Any) -> bool:
    """A field the entry leaves out: missing, ``null`` or ``""``."""
    return value is None or (isinstance(value, str) and value == "")


def _instant(name: str, field: str, value: Any) -> _dt.datetime:
    if not isinstance(value, str):
        raise EsmSourceError("esm_source_temporal", name, f"temporal.{field} must be a string")
    try:
        return parse_rfc3339(value)
    except ValueError as exc:
        raise EsmSourceError(
            "esm_source_temporal", name, f"temporal.{field} {value!r} is not an ISO 8601 instant"
        ) from exc


def _template_error(name: str, exc: EsmTemplateError) -> EsmSourceError:
    """``exc`` restated to name the data source (spec §3.3)."""
    text = str(exc)
    prefix = f"{exc.code}: "
    return EsmSourceError(exc.code, name, text[len(prefix) :] if text.startswith(prefix) else text)


def _period(name: str, field: str, value: Any) -> CalendarPeriod:
    try:
        return CalendarPeriod.parse(value)
    except CalendarPeriodError as exc:
        raise EsmSourceError("esm_period_invalid", name, f"temporal.{field}: {exc}") from exc


def _check_url(name: str, template: str) -> None:
    if "${" in template:
        raise EsmSourceError(
            "esm_source_url",
            name,
            f"{template!r} names an environment variable, which esm-spec §8.2.1 refuses",
        )
    if not (_SCHEME.match(template) or template.startswith("{")):
        raise EsmSourceError(
            "esm_source_url",
            name,
            f"{template!r} is not a URL; load the document with EarthSciAST, which "
            "resolves a relative location against the file that declared it "
            "(esm-spec §8.2.1), or pass url_override",
        )


def _temporal(name: str, node: Any, records_per_sample: int | None):
    from .provider import SourceTemporal

    if node is None:
        return None
    if not isinstance(node, Mapping):
        raise EsmSourceError("esm_source_temporal", name, "temporal must be an object")
    start = node.get("start")
    freq = None if _absent(node.get("frequency")) else node.get("frequency")
    fp = None if _absent(node.get("file_period")) else node.get("file_period")
    if _absent(start):
        if freq is not None or fp is not None:
            raise EsmSourceError(
                "esm_source_temporal",
                name,
                "temporal declares a frequency or file_period but no start to anchor it",
            )
        return None  # no anchor: a time-invariant source
    if freq is None and fp is None:
        raise EsmSourceError(
            "esm_source_temporal",
            name,
            f"temporal anchors at {start!r} but declares neither frequency nor file_period",
        )
    f = _period(name, "frequency", freq if freq is not None else fp)
    p = _period(name, "file_period", fp if fp is not None else freq)
    st = _instant(name, "start", start)
    end = node.get("end")
    tv = node.get("time_variable")
    if not _absent(tv) and not isinstance(tv, str):
        raise EsmSourceError("esm_source_temporal", name, "temporal.time_variable must be a string")
    temporal = SourceTemporal(
        start=st,
        frequency=f,
        file_period=p,
        end=None if _absent(end) else _instant(name, "end", end),
        time_dim="time" if _absent(tv) else tv,
        records_per_sample=records_per_sample,
    )
    rpf = node.get("records_per_file")
    if rpf is not None and rpf != "auto":
        # The records of the first file: frequency anchors in [start, next
        # file), counted from their indices (spec §2.3), not by stepping.
        nxt = temporal.next_file_anchor(st)
        n = f.ceil(st, nxt) - f.ceil(st, st)
        if not isinstance(rpf, int) or isinstance(rpf, bool) or rpf != n:
            raise EsmSourceError(
                "esm_source_records_per_file",
                name,
                f"temporal.records_per_file is {rpf!r} but file_period / frequency "
                f"gives {n} records per file",
            )
    return temporal


def _mirrors(name: str, src: Any) -> list[str]:
    raw = src.get("mirrors") if isinstance(src, Mapping) else None
    if raw is None:
        return []
    if not isinstance(raw, (list, tuple)):
        raise EsmSourceError("esm_source_url", name, "source.mirrors must be an array")
    if not all(isinstance(m, str) for m in raw):
        raise EsmSourceError("esm_source_url", name, "every mirror must be a string")
    return list(raw)


def source_from_esm(
    name: str,
    entry: Mapping[str, Any],
    *,
    variables: Sequence[str] = (),
    substitutions: Mapping[str, str] | None = None,
    url_override: str | None = None,
    records_per_sample: int | None = None,
):
    """The :class:`~earthsciio.DataSource` an esm ``data_sources`` entry
    declares (spec/esm-sources.md §3)."""
    from .provider import DataSource

    if not isinstance(entry, Mapping):
        raise EsmSourceError("esm_source_invalid", name, "the entry must be an object")
    md = entry.get("metadata")
    fmt = md.get("esio_format") if isinstance(md, Mapping) else None
    if not isinstance(fmt, str) or not fmt:
        raise EsmSourceError(
            "esm_source_no_format",
            name,
            "declares no metadata.esio_format, so the reader is unknown",
        )
    src = entry.get("source")
    template = (
        url_override
        if url_override is not None
        else (src.get("url_template") if isinstance(src, Mapping) else None)
    )
    if not isinstance(template, str):
        raise EsmSourceError("esm_source_url", name, "has no source.url_template")
    mirrors = [] if url_override is not None else _mirrors(name, src)
    for t in [template, *mirrors]:
        _check_url(name, t)
    subs = dict(substitutions or {})
    needs = {n for t in [template, *mirrors] for n in esm_template_names(t)}
    if "var" in needs and "var" not in subs and len(variables) == 1:
        subs["var"] = str(variables[0])
    try:
        url = EsmTemplate(template, subs)
        for m in mirrors:
            _check_esm_template(m, subs)
    except EsmTemplateError as exc:
        raise _template_error(name, exc) from exc
    ropts = entry.get("reader_options") or {}
    if not isinstance(ropts, Mapping):
        raise EsmSourceError("esm_source_invalid", name, "reader_options must be an object")
    temporal = _temporal(name, entry.get("temporal"), records_per_sample)
    if temporal is None:
        # A time-invariant source has no anchor, so a {date:...} token has
        # nothing to expand against.
        try:
            for t in [template, *mirrors]:
                expand_esm_template(t, None, subs)
        except EsmTemplateError as exc:
            raise _template_error(name, exc) from exc
    return DataSource(
        name=name,
        format=fmt,
        url=url,
        variables=list(variables),
        temporal=temporal,
        mirrors=mirrors,
        reader_kwargs=dict(ropts),
    )


def provider_from_esm(
    name: str,
    entry: Mapping[str, Any],
    cache,
    *,
    variables: Sequence[str] = (),
    substitutions: Mapping[str, str] | None = None,
    url_override: str | None = None,
    records_per_sample: int | None = None,
    window=None,
    time_origin=None,
):
    """A :class:`~earthsciio.Provider` over an esm ``data_sources`` entry,
    bound to ``cache`` and the run ``window`` (UTC datetimes, or seconds from
    ``time_origin``).

    ``time_origin`` (a datetime or ISO 8601 string, a whole second) is the
    instant the provider counts time from, usually the model's time origin
    (EarthSciAST's ``time_origin(document)``); with it, every time in and out
    is seconds from it (spec/esm-sources.md §4). An entry does not carry the
    model's origin, so there is no default: without one, times are as before."""
    from .provider import Provider

    return Provider(
        source_from_esm(
            name,
            entry,
            variables=variables,
            substitutions=substitutions,
            url_override=url_override,
            records_per_sample=records_per_sample,
        ),
        cache,
        window,
        time_origin=time_origin,
    )
