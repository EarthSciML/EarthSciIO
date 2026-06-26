"""Cache-root resolution, offline-mode detection, and the cache-format version.

These are the few process-wide knobs the spec pins:

* ``$EARTHSCIDATADIR`` resolution (``spec/cache-format.md`` §5) — env always
  wins; the default lives on ``/scratch.local``, **never** ``/u`` (the home
  inode quota cannot absorb many small NetCDF slices — Risk R6, a hard rule).
* Offline-mode enabling (``spec/offline-mode.md`` §1) — ``offline=True`` **or**
  ``EARTHSCI_OFFLINE`` truthy; the explicit argument wins over the environment.
* The cache-format version directory (``v1``); bumping it invalidates the whole
  cache by changing one path segment.
"""

from __future__ import annotations

import getpass
import os
import pathlib
from typing import Optional

#: Cache-format version. Bumping this string invalidates every cached blob
#: (the on-disk layout lives under ``<root>/<CACHE_FORMAT_VERSION>/``).
CACHE_FORMAT_VERSION = "v1"

_TRUTHY = {"1", "true", "yes", "on"}


def default_cache_root() -> pathlib.Path:
    """The fallback cache root when ``$EARTHSCIDATADIR`` is unset.

    ``/scratch.local/$USER/earthsci-cache`` — on scratch, never the home inode
    quota. ``$USER`` falls back to ``getpass.getuser()`` when the env var is
    absent (e.g. some batch contexts).
    """
    user = os.environ.get("USER") or _safe_getuser()
    return pathlib.Path("/scratch.local") / user / "earthsci-cache"


def _safe_getuser() -> str:
    try:
        return getpass.getuser()
    except Exception:  # pragma: no cover - getuser can raise if no passwd entry
        return "earthsci"


def resolve_cache_root(explicit: Optional[os.PathLike] = None) -> pathlib.Path:
    """Resolve the cache root directory.

    Precedence: an explicit argument, then ``$EARTHSCIDATADIR``, then the
    ``/scratch.local`` default. The environment variable **always wins** over the
    default — only an explicit in-process argument overrides it (used by tests
    and by callers that point at, e.g., the conformance corpus).
    """
    if explicit is not None:
        return pathlib.Path(explicit)
    env = os.environ.get("EARTHSCIDATADIR")
    if env:
        return pathlib.Path(env)
    return default_cache_root()


def env_offline() -> bool:
    """Whether ``$EARTHSCI_OFFLINE`` requests offline mode (truthy, ci-safe)."""
    return os.environ.get("EARTHSCI_OFFLINE", "").strip().lower() in _TRUTHY


def env_live() -> bool:
    """Whether ``$EARTHSCI_LIVE`` opts into live network fetches.

    Live fetches are the explicit, opt-in path (one smoke test per auth realm),
    **never** in CI. The cache/transport core does not branch on this today; it
    is exposed so callers and tests can gate live-only paths.
    """
    return os.environ.get("EARTHSCI_LIVE", "").strip().lower() in _TRUTHY


def resolve_offline(explicit: Optional[bool] = None) -> bool:
    """Resolve effective offline mode.

    ``explicit`` of ``True``/``False`` wins; ``None`` means "consult the
    environment" (``EARTHSCI_OFFLINE``). This sentinel is how the spec's "the
    explicit argument wins over the environment when both are present" is honored
    without conflating an unset argument with an explicit ``False``.
    """
    if explicit is not None:
        return bool(explicit)
    return env_offline()


def expand_datadir(template: str, cache_root: os.PathLike) -> str:
    """Expand ``${EARTHSCIDATADIR}`` / ``$EARTHSCIDATADIR`` inside a template.

    Used for ``file://`` mirror templates (the ``nei2016`` pattern) so a
    pre-populated local mirror is found. The cache root in effect for *this*
    provider is substituted (which may be an explicit override, not the raw env
    var), keeping mirror resolution consistent with where blobs actually live.
    """
    root = str(cache_root)
    return (
        template.replace("${EARTHSCIDATADIR}", root)
        .replace("$EARTHSCIDATADIR", root)
    )
