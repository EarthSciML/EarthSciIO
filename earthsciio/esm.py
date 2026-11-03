"""esm data-source URL templates (spec/esm-sources.md §1).

An esm ``source.url_template`` (and every ``source.mirrors`` entry) is literal
text with ``{date:<format>}`` and ``{name}`` tokens (esm-spec §8.2). This module
expands one, exactly as the Julia and Rust tracks do: the date directives are
implemented here rather than handed to :meth:`datetime.strftime`, whose
locale and padding rules are the platform's.
"""

from __future__ import annotations

import datetime as _dt
import re
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field

from .errors import EarthSciIOError

__all__ = [
    "EsmTemplate",
    "EsmTemplateError",
    "esm_template_names",
    "expand_esm_template",
]

_TOKEN = re.compile(r"\{([A-Za-z_][A-Za-z0-9_]*)(?::([^{}]*))?\}")


class EsmTemplateError(EarthSciIOError, ValueError):
    """An esm URL template could not be expanded. ``code`` is one of the
    ``esm_template_*`` codes of spec/esm-sources.md §1.3, and the message
    contains it."""

    def __init__(self, code: str, template: str, detail: str) -> None:
        self.code = code
        self.template = template
        super().__init__(f"{code}: {detail} in esm URL template {template!r}")


def _to_utc(anchor: _dt.datetime) -> _dt.datetime:
    if anchor.tzinfo is not None:
        anchor = anchor.astimezone(_dt.timezone.utc).replace(tzinfo=None)
    return anchor


def _format_date(template: str, fmt: str, anchor: _dt.datetime) -> str:
    out: list[str] = []
    i = 0
    while i < len(fmt):
        c = fmt[i]
        if c != "%":
            out.append(c)
            i += 1
            continue
        if i + 1 >= len(fmt):
            raise EsmTemplateError(
                "esm_template_directive", template, f"a '%' ends the date format {fmt!r}"
            )
        d = fmt[i + 1]
        if d == "Y":
            if not 0 <= anchor.year <= 9999:
                raise EsmTemplateError(
                    "esm_template_year_range",
                    template,
                    f"%Y of year {anchor.year} is outside 0 to 9999",
                )
            out.append(f"{anchor.year:04d}")
        elif d == "y":
            out.append(f"{anchor.year % 100:02d}")
        elif d == "m":
            out.append(f"{anchor.month:02d}")
        elif d == "d":
            out.append(f"{anchor.day:02d}")
        elif d == "j":
            out.append(f"{anchor.timetuple().tm_yday:03d}")
        elif d == "H":
            out.append(f"{anchor.hour:02d}")
        elif d == "M":
            out.append(f"{anchor.minute:02d}")
        elif d == "S":
            out.append(f"{anchor.second:02d}")
        elif d == "%":
            out.append("%")
        else:
            raise EsmTemplateError(
                "esm_template_directive",
                template,
                f"unsupported date directive %{d} (supported: %Y %y %m %d %j %H %M %S %%)",
            )
        i += 2
    return "".join(out)


def expand_esm_template(
    template: str,
    anchor: _dt.datetime | None = None,
    substitutions: Mapping[str, str] | None = None,
) -> str:
    """Expand an esm URL template at ``anchor`` (a UTC instant; a naive datetime
    is read as UTC) with ``substitutions``. See spec/esm-sources.md §1."""
    subs = dict(substitutions or {})
    if "date" in subs:
        raise EsmTemplateError(
            "esm_template_reserved",
            template,
            "the substitution name 'date' is reserved for {date:<format>}",
        )
    when = None if anchor is None else _to_utc(anchor)

    def sub(m: re.Match[str]) -> str:
        name, fmt = m.group(1), m.group(2)
        if name == "date":
            if not fmt:
                raise EsmTemplateError(
                    "esm_template_directive",
                    template,
                    "{date} needs a format, e.g. {date:%Y%m%d}",
                )
            if when is None:
                raise EsmTemplateError(
                    "esm_template_no_anchor",
                    template,
                    "{date:...} needs a time and none was given",
                )
            return _format_date(template, fmt, when)
        if fmt is not None:
            raise EsmTemplateError(
                "esm_template_directive",
                template,
                f"the substitution {{{name}}} takes no format (got {{{name}:{fmt}}})",
            )
        if name not in subs:
            raise EsmTemplateError(
                "esm_template_unbound",
                template,
                f"the substitution {{{name}}} is not bound",
            )
        return str(subs[name])

    return _TOKEN.sub(sub, template)


def esm_template_names(template: str) -> list[str]:
    """The substitution names ``template`` needs, in order of first appearance,
    without ``date``."""
    out: list[str] = []
    for m in _TOKEN.finditer(template):
        name = m.group(1)
        if name != "date" and name not in out:
            out.append(name)
    return out


@dataclass(frozen=True)
class EsmTemplate:
    """A loader URL that is an esm template (spec/esm-sources.md §1.5).

    Use it as :class:`~earthsciio.DataSource` ``url``; give ``mirrors`` as esm
    templates too and they are expanded at the same anchor with the same
    ``substitutions``. Calling it expands the template at an anchor.
    """

    template: str
    substitutions: Mapping[str, str] = field(default_factory=dict)

    def __post_init__(self) -> None:
        # Refuse a reserved name or an unbound one now, not at the first fetch.
        expand_esm_template(
            _TOKEN.sub(lambda m: m.group(0) if m.group(1) != "date" else "", self.template),
            None,
            self.substitutions,
        )

    def __call__(self, anchor: _dt.datetime) -> str:
        return expand_esm_template(self.template, anchor, self.substitutions)

    def expand_mirrors(self, mirrors: Sequence[str], anchor: _dt.datetime) -> list[str]:
        """Each mirror template expanded at ``anchor``."""
        return [expand_esm_template(m, anchor, self.substitutions) for m in mirrors]
