"""Zarr stub: a chunked-store reader (the future NetCDF→Zarr cloud path).

``status:"stub"`` in ``spec/registries.json`` — registered now so the format
seam is *exercised* (``esio-9nb.8``), implemented later by the ``esio-cloud``
epic (``spec/cloud-future.md``). It is interface-conformant
(:class:`earthsciio.registry.Reader`) so the Provider resolves and constructs it
by name unchanged; every real operation raises
:class:`~earthsciio.errors.Unsupported`.
"""

from __future__ import annotations

from typing import Any, Dict, List, Optional

from ..errors import Unsupported

__all__ = ["ZarrReader"]

_TRACKING = "esio-cloud"


class ZarrReader:
    """Stub ``zarr`` reader (chunked array store).

    Real implementation (``esio-cloud``) must open a (possibly remote) chunked
    store, read native arrays lazily/by-region aligned to the native grid, and
    CF-decode with **byte-identical** conventions to the ``netcdf`` reader
    (``spec/conformance.md`` §3) so cross-language array equality holds — see
    ``spec/cloud-future.md``.
    """

    #: Registry name + format key(s) + extension sniff hints.
    NAME = "zarr"
    FORMATS = ("zarr",)
    EXTENSIONS = ("zarr",)

    def formats(self) -> List[str]:
        return list(self.FORMATS)

    def extensions(self) -> List[str]:
        return list(self.EXTENSIONS)

    def open(self, blob_path: Any) -> Any:
        raise Unsupported(self.NAME, registry="format", operation="open", tracking=_TRACKING)

    def read_native(
        self,
        handle: Any,
        variables: List[str],
        select: Optional[Any] = None,
    ) -> Dict[str, Any]:
        raise Unsupported(self.NAME, registry="format", operation="read_native", tracking=_TRACKING)
