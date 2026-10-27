//! Fetch a file into the content-addressed cache, then read it back offline.

use earthsciio::{cache_key, Cache, FetchRequest};
use earthsciio_api_examples::fixture;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = fixture("stations.csv").canonicalize()?;
    let url = format!("file://{}", source.display());
    let cache_root = tempfile::tempdir()?; // hide

    // region: fetch ops=cache,cache_entry,cache_key
    let cache = Cache::builder()
        .data_dir(cache_root.path())
        .offline(false)
        .build()?;
    let blob = cache.fetch(&FetchRequest::new(&url).loader("stations"))?;
    assert_eq!(std::fs::read(&blob.path)?, std::fs::read(&source)?);
    // The blob is addressed by the SHA-256 of the resolved URL.
    assert_eq!(blob.key, cache_key(&url));

    // Asking again returns the same blob without a transfer.
    let again = cache.fetch(&FetchRequest::new(&url))?;
    assert_eq!(again.path, blob.path);
    // endregion

    // region: offline ops=is_offline,error
    let offline = Cache::builder()
        .data_dir(cache_root.path())
        .offline(true)
        .build()?;
    assert!(offline.is_offline());
    assert_eq!(offline.fetch(&FetchRequest::new(&url))?.path, blob.path); // served from disk only

    // An offline miss is an error, never a network request.
    let miss = offline
        .fetch(&FetchRequest::new("file:///not/cached.csv"))
        .unwrap_err();
    assert!(miss.is_cache_miss());
    // endregion

    // region: data_dir ops=data_dir
    assert!(!earthsciio::data_dir().as_os_str().is_empty());
    // endregion
    Ok(())
}
