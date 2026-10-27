---
title: "Getting started"
description: "Fetch a file into the shared cache, read it back offline, and decode it into native-grid arrays."
weight: 10
---

EarthSciIO resolves a URL, downloads it once into a **content-addressed cache**
shared by the Python, Julia and Rust implementations, and decodes the cached file
into **native-grid arrays**. This guide follows one small file through both
steps. Each snippet is cut from a program that runs, and checks its own result,
in CI.

The examples read `stations.csv`, a four-row table of air-quality readings from
two stations, through a `file://` URL so they need no network.

## Fetch into the cache

The cache key is the SHA-256 of the resolved URL, so the same URL always lands
on the same blob, whichever language fetched it. The first fetch downloads; the
second is a hit.

<!-- example: fetch -->

## Work offline

Offline mode serves only what is already cached. A request that is not cached
fails with a cache miss rather than opening a connection, which makes test runs
hermetic.

<!-- example: offline -->

## Choose the cache location

The cache root defaults to `$EARTHSCIDATADIR`; the constructors take an explicit
root.

<!-- example: data_dir -->

## Read native-grid arrays

A provider ties a data source to the cache and to a format reader, and returns
the file's variables as raw native-grid arrays. Variable renaming, unit
conversion and regridding are left to the rest of the stack.

<!-- example: provider -->

The full operation list is in the [API reference](/generated/api/), and the
contract all three implementations follow is in the
[provider spec](/generated/spec/).
