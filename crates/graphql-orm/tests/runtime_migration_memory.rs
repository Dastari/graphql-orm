#![cfg(any(feature = "sqlite", feature = "postgres"))]

use graphql_orm::graphql::orm::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static BYTES: Cell<isize> = const { Cell::new(0) };
}
struct CountAlloc;
fn count(delta: isize) {
    let _ = TRACK.try_with(|track| {
        if track.get() {
            let _ = BYTES.try_with(|bytes| bytes.set(bytes.get() + delta));
        }
    });
}
// Safety: allocation and pointer/layout handling delegate unchanged to System.
// Thread-local counters neither allocate nor retain the allocated pointers.
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            count(layout.size() as isize);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count(-(layout.size() as isize));
        unsafe {
            System.dealloc(pointer, layout);
        }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(pointer, layout, size) };
        if !new.is_null() {
            count(size as isize - layout.size() as isize);
        }
        new
    }
}
#[global_allocator]
static ALLOCATOR: CountAlloc = CountAlloc;
struct Tracking;
impl Drop for Tracking {
    fn drop(&mut self) {
        TRACK.with(|track| track.set(false));
    }
}

#[test]
fn repeated_owned_conversion_and_hashing_release_every_allocation() {
    let definition: RuntimeSchema = serde_json::from_value(serde_json::json!({
        "format_version":1,"collections":[{
            "id":"notes","api_type_name":"Note","api_plural_name":"Notes",
            "physical_table":"notes","primary_key":["note_id"],"append_only":false,"retention_purge":false,
            "fields":[{"id":"note_id","api_name":"id","physical_column":"id","value_kind":"uuid",
                "nullable":false,"unique":false,"filterable":false,"sortable":true,"generated":true,"default":null},
                {"id":"note_label","api_name":"label","physical_column":"label","value_kind":"string",
                "nullable":false,"unique":false,"filterable":true,"sortable":false,"generated":false,"default":null}],
            "relations":[],"indexes":[],"composite_unique":[],"default_order":[{"field":"note_id","direction":"asc"}]
        }]
    })).unwrap();
    let schema = definition.validate().unwrap();
    #[cfg(feature = "sqlite")]
    type Backend = SqliteBackend;
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    type Backend = PostgresBackend;
    let expected = schema
        .physical_schema::<Backend>(Default::default())
        .unwrap()
        .stable_hash();
    BYTES.with(|bytes| bytes.set(0));
    TRACK.with(|track| track.set(true));
    let tracking = Tracking;
    for _ in 0..1000 {
        let target = schema
            .physical_schema::<Backend>(Default::default())
            .unwrap();
        assert_eq!(target.stable_hash(), expected);
        assert_eq!(target.tables()[0].indexes().len(), 1);
    }
    drop(tracking);
    assert_eq!(
        BYTES.with(Cell::get),
        0,
        "owned conversion must not leak per-schema index/name storage"
    );
}
