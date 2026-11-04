---
title: "EarthSciIO"
description: "Data-provider library: URL download, a shared content-addressed cache, and native-grid readers for Python, Julia and Rust."
---

EarthSciIO is the data-provider layer of the EarthSci stack. It resolves a data
source's URL at a time, downloads the file into a **shared content-addressed
cache**, and reads it into **native-grid arrays** the simulation consumes at its
own cadence. It provides data, never a solver: variable remapping stays in
EarthSciAST and regridding in EarthSciDiscretizations.

- [Overview](overview/) — how it works, with Python usage.
- [Getting started](generated/api/guides/getting-started/) — runnable examples in all three languages.
- [API reference](generated/api/) — every operation, side by side in Python, Julia and Rust.
- [Provider spec](generated/spec/) — the language-neutral contract the implementations follow.

The three implementations are written idiomatically for each language against
the shared spec, and a file fetched by one is reused byte-for-byte by the
others. A cross-language conformance corpus checks that they decode the same
native arrays.
