# EarthSciIO offline-mode contract (v1)

**Deliverable (c) of `esio-9nb.1`.** Status: normative.

Offline mode is what makes conformance deterministic and CI **hermetic**: the
refinery and every conformance run touch **no network**. It is also the campfire
C2 acceptance ("3 loaders return real arrays from cache, offline").

---

## 1. Enabling

Offline is on when **either** holds:

- the Provider / opener is constructed with `offline = true`, **or**
- the environment variable `EARTHSCI_OFFLINE` is set to a truthy value
  (`1`, `true`, `yes`, case-insensitive).

The explicit argument wins over the environment when both are present.

---

## 2. Semantics — cache-only, never the network

When offline:

1. The [`transport` registry](registries.md#1-transport-registry)
   is **never consulted** — no transport is constructed, no socket is opened,
   no DNS lookup is made, for any scheme (including `file`-as-download).
2. A read resolves purely against the [`store`](registries.md#3-store-registry):
   compute `key = sha256(resolved_url)`; if `store.get_blob(key)` is present →
   decode and return it; the stored `sha256_content` is the only integrity check.
3. A **miss** (blob absent for the resolved key) raises **`CacheMiss`**
   immediately. It is never a silent empty result and never a fallback fetch.

```
offline read(resolved_url):
    key = sha256(resolved_url)
    if not store.exists(key): raise CacheMiss(resolved_url, key)
    return reader.read_native(store.get_blob(key), ...)
```

`CacheMiss` MUST carry the `resolved_url` and `key` so a failure names exactly
which blob the corpus/cache is missing.

---

## 3. Conditional-GET / TTL revalidation is suppressed

The validation ladder in
[cache-format.md §4](cache-format.md#4-validation-and-integrity) is
**short-circuited** offline: steps that need the network (conditional GET, TTL
refresh of an incomplete period) do not run. Presence + stored content hash is
sufficient. A stale-but-present blob is **used**, not refreshed — offline trades
freshness for hermeticity by design.

---

## 4. Hermetic CI + conformance

- The conformance corpus (`conformance/corpus/cache`) **is** a pre-populated
  `$EARTHSCIDATADIR`. Point a provider at it with `offline = true` and every
  golden case resolves from disk.
- CI runs the whole suite offline → no network on the refinery, fully
  deterministic, reproducible on any machine.
- Live network fetches are a separate, **opt-in** path (`EARTHSCI_LIVE=1`), one
  smoke test per auth realm, **never** in CI. Their cached results become new
  golden fixtures.

---

## 5. Cross-language requirement

All three tracks MUST implement identical offline semantics: same enabling
flags, same cache-only resolution, the same `CacheMiss` error on a missing key.
A corpus that resolves offline in Python MUST resolve offline in Julia and Rust
from the same `cache/` directory — that is the whole point of the
[shared cache key](cache-format.md#1-cache-key).
