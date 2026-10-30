"""EarthSciIO — Python binding of the cross-language data-provider spec.

This package exposes:

* the **extensibility seam** — the three registries
  (:data:`transport_registry`, :data:`format_registry`, :data:`store_registry`),
  their interfaces (:class:`Transport`, :class:`Reader`, :class:`Store`), and the
  registry error types (:class:`Unsupported`, :class:`BackendNotRegistered`);
* the **cache core** (``esio-9nb.2``) — :class:`Cache` + :class:`CacheEntry`, the
  URL→content-addressed-blob fetcher behind the ESS opener/fetcher seam, with its
  runtime errors (:class:`CacheMiss`, :class:`IntegrityError`, …), the
  :class:`Manifest`, the :func:`cache_key`, the :class:`~earthsciio.validate.Temporal`
  freshness policy, and the pluggable auth resolvers;
* the **epoch prefetch** surface (``spec/prefetch.md``) — :class:`DimWindow`
  (a window that names its dimensions), :class:`PrefetchGroup` and the
  :class:`Sample`/:class:`EpochSample` its ``take`` returns, beside
  :meth:`Provider.sample_at`, :meth:`Provider.with_window` and
  :meth:`Provider.sample_windows_at`.

On import it registers the cloud **stubs** (S3 transport+store, Zarr reader) and
the **active** core backends (``http``/``file`` transports, ``local`` store) into
the three registries — additively, without changing the Provider API (the whole
point of the seam, ``spec/registries.md`` §4).
"""

from __future__ import annotations

# Register backends on import (idempotent). Stubs first (esio-9nb.8), then the
# active core backends (esio-9nb.2); order is irrelevant — names are orthogonal.
from . import backends, era5
from .auth import AuthRegistry, AuthResolver, StaticHeaderAuth
from .backends.cds import (
    CdsTransport,
    cds_api_key,
    cds_api_url,
    cds_auth,
    decode_cds_url,
    encode_cds_url,
)
from .cache import Cache, CacheEntry
from .cachekey import cache_key, range_keyed_url, sha256_bytes, sha256_file
from .config import (
    CACHE_FORMAT_VERSION,
    REVALIDATE_FILE_ENV,
    default_cache_root,
    resolve_cache_root,
    resolve_offline,
    resolve_revalidate_file,
)
from .errors import (
    AuthError,
    BackendNotRegistered,
    CacheMiss,
    CommitError,
    EarthSciIOError,
    FetchError,
    IntegrityError,
    OfflineError,
    RegionNotShardAligned,
    TransportError,
    UnknownReaderOption,
    Unsupported,
)
from .manifest import Manifest, parse_rfc3339, utc_now_rfc3339
from .native import NativeDataset, NativeField
from .prefetch import EpochSample, PrefetchGroup, Sample
from .provider import (
    DataLoader,  # 0.1.1 alias of DataSource; dropped in 0.2.0
    DataSource,
    LoaderTemporal,  # 0.1.1 alias of SourceTemporal; dropped in 0.2.0
    Provider,
    SourceTemporal,
    Window,
    reader_option_names,
)
from .readers import (
    CSVReader,
    FF10Reader,
    GeoTIFFReader,
    NetCDFReader,
    ParquetReader,
    ShapefileReader,
    register_format_readers,
)
from .registry import (
    Reader,
    Registry,
    RegistryEntry,
    Store,
    Transport,
    all_registries,
    dim_length,
    format_registry,
    store_registry,
    supports_selection,
    transport_registry,
)
from .selection import DimWindow
from .validate import (
    SourceRevalidator,
    Temporal,
    file_source_is_current,
    file_source_state,
)

backends.register_stub_backends()
backends.register_active_backends()

# Register the active netcdf/csv format readers (esio-9nb.3) — the decode half
# the Provider reads through. Idempotent; orthogonal to the zarr stub.
register_format_readers()

__all__ = [
    # config
    "CACHE_FORMAT_VERSION",
    "REVALIDATE_FILE_ENV",
    "AuthError",
    # auth
    "AuthRegistry",
    "AuthResolver",
    "BackendNotRegistered",
    "CSVReader",
    # cache core
    "Cache",
    "CacheEntry",
    # errors — cache core
    "CacheMiss",
    # CDS transport + ERA5 request mapping
    "CdsTransport",
    "CommitError",
    # 0.1.1 spellings, kept source-compatible; dropped in 0.2.0
    "DataLoader",
    "DataSource",
    # named-dimension windows + epoch prefetch (spec/prefetch.md)
    "DimWindow",
    # errors — registry seam
    "EarthSciIOError",
    "EpochSample",
    "FF10Reader",
    "FetchError",
    "GeoTIFFReader",
    "IntegrityError",
    "LoaderTemporal",
    "Manifest",
    "NativeDataset",
    # native arrays + format readers (esio-9nb.3)
    "NativeField",
    "NetCDFReader",
    "OfflineError",
    "ParquetReader",
    "PrefetchGroup",
    # provider API (esio-9nb.3)
    "Provider",
    "Reader",
    # errors — the streaming writer (spec/writer.md)
    "RegionNotShardAligned",
    # registry mechanism + interfaces
    "Registry",
    "RegistryEntry",
    "Sample",
    "ShapefileReader",
    "SourceRevalidator",
    "SourceTemporal",
    "StaticHeaderAuth",
    "Store",
    "Temporal",
    "Transport",
    "TransportError",
    "UnknownReaderOption",
    "Unsupported",
    "Window",
    "all_registries",
    # backend package
    "backends",
    "cache_key",
    "cds_api_key",
    "cds_api_url",
    "cds_auth",
    "decode_cds_url",
    "default_cache_root",
    "dim_length",
    "encode_cds_url",
    "era5",
    "file_source_is_current",
    "file_source_state",
    "format_registry",
    "parse_rfc3339",
    "range_keyed_url",
    "reader_option_names",
    "register_format_readers",
    "resolve_cache_root",
    "resolve_offline",
    "resolve_revalidate_file",
    "sha256_bytes",
    "sha256_file",
    "store_registry",
    "supports_selection",
    # the three registry singletons
    "transport_registry",
    "utc_now_rfc3339",
]
