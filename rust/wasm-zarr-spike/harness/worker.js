// The OPFS half of the acceptance test, run where it actually has to run: a
// dedicated Web Worker. `createSyncAccessHandle()` exists ONLY in a worker, and
// the consuming runner is already off the main thread, so this is the real
// configuration rather than a convenient one.
//
// Loaded by index.html; posts a single result object back.

import init, {
  opfs_roundtrip,
  opfs_put,
  opfs_read_array,
  opfs_rm,
} from "./pkg/earthsciio_wasm_zarr_spike.js";

const results = [];
const record = (name, ok, detail) => results.push({ name, ok, detail });

async function run() {
  await init();

  // 1. Sanity: the Worker-only synchronous OPFS API is present.
  const root = await navigator.storage.getDirectory();
  const probe = await root.getFileHandle("__probe__", { create: true });
  const sync = await probe.createSyncAccessHandle();
  sync.close();
  await root.removeEntry("__probe__");
  record("createSyncAccessHandle available in worker", true, null);

  // 2. Write a wasm-profile sharded store to OPFS and read it back.
  const summary = JSON.parse(await opfs_roundtrip("spike-roundtrip"));
  record(
    "OPFS round-trip decodes exactly",
    summary.ok === true && summary.max_abs_error === 0,
    summary,
  );
  record(
    "store on disk is sharded, zstd, blosc-free",
    summary.sharded === true && summary.zstd === true && summary.blosc_free === true,
    { sharded: summary.sharded, zstd: summary.zstd, blosc_free: summary.blosc_free },
  );
  await opfs_rm("spike-roundtrip");

  // 3. THE CROSS-READ: put a store the NATIVE writer produced into OPFS, then
  //    decode it with the wasm build.
  const store = await (await fetch("../fixtures/native_wasm_profile.store.json")).json();
  const expected = await (await fetch("../fixtures/native_wasm_profile.expected.json")).json();
  await opfs_rm("spike-native");
  for (const [key, bytes] of Object.entries(store)) {
    await opfs_put("spike-native", key, new Uint8Array(bytes));
  }
  const got = await opfs_read_array("spike-native", "conc");
  let maxErr = 0;
  for (let i = 0; i < expected.values.length; i += 1) {
    maxErr = Math.max(maxErr, Math.abs(got[i] - expected.values[i]));
  }
  record(
    "native-written store cross-reads from OPFS in wasm",
    got.length === expected.values.length && maxErr === 0,
    { elements: got.length, max_abs_error: maxErr, objects: Object.keys(store).length },
  );
  await opfs_rm("spike-native");
}

run().then(
  () => postMessage({ done: true, results }),
  (e) => postMessage({ done: true, error: String(e && e.stack ? e.stack : e), results }),
);
