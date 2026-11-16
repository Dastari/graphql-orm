use std::sync::Arc;

use graphql_orm_storage::{
    LocalStorageBackend, StorageNamespace, StoragePutRequest, StorageService,
};

#[tokio::main]
async fn main() -> Result<(), graphql_orm_storage::StorageError> {
    let root = "./target/graphql-orm-storage-example";
    // Windows has no plain constructor: the host must own the whole tree.
    #[cfg(windows)]
    let backend = LocalStorageBackend::new_trusted_windows_root(root);
    #[cfg(not(windows))]
    let backend = LocalStorageBackend::new(root);
    let service = StorageService::new(Arc::new(backend));
    let object = service
        .put_object(StoragePutRequest {
            namespace: StorageNamespace::Originals,
            file_name: Some("example.txt".to_owned()),
            mime_type: Some("text/plain".to_owned()),
            bytes: b"storage example".to_vec(),
        })
        .await?;

    println!("{} {}", object.storage_key, object.sha256_hex);
    Ok(())
}
