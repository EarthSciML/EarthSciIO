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
from .esm import EsmTemplate, esm_template_names

__all__ = ["EsmSourceError", "provider_from_esm", "source_from_esm"]

_SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*://")


class EsmSourceError(EarthSciIOError, ValueError):
    """An esm data source EarthSciIO cannot read. ``code`` is one of the
    ``esm_source_*`` codes of spec/esm-sources.md §3.3 (or ``esm_period_invalid``)
    and the message contains it and the source's name."""

    def __init__(self, code: str, name: str, detail: str) -> None:
        self.code = code
        self.source = name
        super().__init__(f"{code}: data source {name!r}: {detail}")


def _instant(name: str, field: str, value: Any) -> _dt.datetime:
    if not isinstance(value, str):
        raise EsmSourceError("esm_source_temporal", name, f"temporal.{field} must be a string")
    text = value.strip()
    if text.endswith(("Z", "z")):
        text = text[:-1] + "+00:00"
    try:
        t = _dt.datetime.fromisoformat(text)
    except ValueError as exc:
        raise EsmSourceError(
            "esm_source_temporal", name, f"temporal.{field} {value!r} is not an ISO 8601 instant"
        ) from exc
    if t.tzinfo is None:
        t = t.replace(tzinfo=_dt.timezone.utc)
    return t.astimezone(_dt.timezone.utc)


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
    if not start:
        return None  # no anchor: a time-invariant source
    freq = node.get("frequency") or None
    fp = node.get("file_period") or None
    if freq is None and fp is None:
        raise EsmSourceError(
            "esm_source_temporal",
            name,
            f"temporal anchors at {start!r} but declares neither frequency nor file_period",
        )
    f = _period(name, "frequency", freq or fp)
    p = _period(name, "file_period", fp or freq)
    st = _instant(name, "start", start)
    end = node.get("end")
    temporal = SourceTemporal(
        start=st,
        frequency=f,
        file_period=p,
        end=_instant(name, "end", end) if end else None,
        time_dim=str(node.get("time_variable") or "time"),
        records_per_sample=records_per_sample,
    )
    rpf = node.get("records_per_file")
    if rpf is not None and rpf != "auto":
        # The records of the first file: frequency anchors in [start, next file).
        nxt = temporal.next_file_anchor(st)
        n, a = 0, temporal.first_record_anchor_from(st)
        while a < nxt:
            n += 1
            a = temporal.next_record_anchor(a)
        if not isinstance(rpf, int) or isinstance(rpf, bool) or rpf != n:
            raise EsmSourceError(
                "esm_source_records_per_file",
                name,
                f"temporal.records_per_file is {rpf!r} but file_period / frequency "
                f"gives {n} records per file",
            )
    return temporal


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
    mirrors = [] if url_override is not None else list((src or {}).get("mirrors") or [])
    for t in [template, *mirrors]:
        _check_url(name, t)
    subs = dict(substitutions or {})
    needs = {n for t in [template, *mirrors] for n in esm_template_names(t)}
    if "var" in needs and "var" not in subs and len(variables) == 1:
        subs["var"] = str(variables[0])
    ropts = entry.get("reader_options") or {}
    if not isinstance(ropts, Mapping):
        raise EsmSourceError("esm_source_invalid", name, "reader_options must be an object")
    return DataSource(
        name=name,
        format=fmt,
        url=EsmTemplate(template, subs),
        variables=list(variables),
        temporal=_temporal(name, entry.get("temporal"), records_per_sample),
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
):
    """A :class:`~earthsciio.Provider` over an esm ``data_sources`` entry,
    bound to ``cache`` and the run ``window`` (UTC datetimes)."""
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
    )
