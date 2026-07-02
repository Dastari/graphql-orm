# Storage Streaming And Range Boundary

`graphql-orm` owns database rows, schema generation, migrations, and typed
database values. It only handles byte columns as ordinary SQL values
(`BLOB`, `BYTEA`, or the selected backend equivalent). It should not grow
object storage backends or HTTP download helpers.

Object bytes live in the sibling `graphql-orm-storage` repository. The local
checkout reviewed for this note already exposes streaming primitives and
byte-range reads:

- `StorageByteStream` and `BoxedStorageStream` for chunked byte bodies.
- `StorageService::put_object_stream` for streaming writes.
- `StorageService::get_object_stream` for streaming full-object reads.
- `BlobStore::get_blob` for streaming key-addressed full reads.
- `BlobStore::get_blob_range` for streaming key-addressed byte-range reads.
- `BlobStore::head_blob` for provider metadata needed to validate HTTP ranges.

The buffered APIs (`StorageService::put_object`, `StorageService::get_object`,
`StoragePutRequest`, and `StorageObjectBody`) remain compatibility wrappers.
Large media serving code should not use them, because they collect the whole
object into memory.

## Digitise Consumption Contract

Digitise should persist storage metadata in application-owned database rows,
using generated `graphql-orm` entities only for that metadata:

- backend identifier
- provider-neutral `storage_key`
- byte length
- checksum
- MIME type and filename metadata, when needed

For full HTTP responses, Digitise can call
`StorageService::get_object_stream(&stored_object)` or
`BlobStore::get_blob(&storage_key)` and stream the returned
`StorageByteStream` into the response body.

For HTTP `Range` responses, Digitise should:

1. Use `BlobStore::head_blob(&storage_key)` or the persisted byte length to
   validate the requested range.
2. Convert the HTTP byte range to an exclusive Rust range (`start..end + 1`).
3. Call `BlobStore::get_blob_range(&storage_key, start..end_exclusive)`.
4. Return `206 Partial Content` with `Content-Range`, `Accept-Ranges`, and the
   streamed chunk body.
5. Return `416 Range Not Satisfiable` without reading the object when the
   requested range is outside the known size.

Digitise should avoid `StorageService::get_object` and
`collect_storage_stream` in media routes unless it intentionally needs a small
in-memory buffer.

## Optional Storage-Crate Follow-Up

If downstream code should avoid depending on the low-level `BlobStore` trait
for ranged object reads, add this convenience API in `graphql-orm-storage`
instead of adding storage code here:

```rust
impl StorageService {
    pub async fn get_object_range(
        &self,
        object: &StoredObject,
        range: std::ops::Range<u64>,
    ) -> Result<StorageObjectStream, StorageError> {
        let body = self.backend.get_blob_range(&object.storage_key, range).await?;
        Ok(StorageObjectStream {
            object: object.clone(),
            body: body.body,
        })
    }
}
```

`StorageService::get_object_range` would be a thin object-metadata wrapper over
the existing `BlobStore::get_blob_range` primitive. It belongs in
`graphql-orm-storage`, not in `graphql-orm`.
