use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokenfold_core::fit::{DataRepresentation, DataRequest, FitError, MAX_SOURCE_BYTES};

struct TrackingAllocator;
static TRACK: AtomicBool = AtomicBool::new(false);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            ALLOCATED.fetch_add(size, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

#[test]
fn hostile_folded_dictionary_is_rejected_before_expansion() {
    let source =
        json!({"__tf_dict__":["x".repeat(4096)],"__tf_data__":vec![json!({"__tf_ref__":0});1024]});
    assert_eq!(serde_json::to_vec(&source).unwrap().len(), 21_540);
    // Initialize secret regexes outside the measured constructor call.
    DataRequest::new(json!(null), DataRepresentation::Json).unwrap();
    tokenfold_core::fit::validate_text("clean").unwrap();
    ALLOCATED.store(0, Ordering::Relaxed);
    TRACK.store(true, Ordering::Relaxed);
    let result = DataRequest::new(source, DataRepresentation::Folded);
    TRACK.store(false, Ordering::Relaxed);
    let allocated = ALLOCATED.load(Ordering::Relaxed);
    assert!(matches!(result, Err(FitError::InvalidInput(_))));
    assert!(
        allocated < MAX_SOURCE_BYTES,
        "constructor allocated {allocated} bytes for a 21,540-byte source; collision checks must not expand the 4,197,377-byte decoded payload"
    );
    for value in [
        json!({"__tf_dict__":0,"__tf_data__":[]}),
        json!({"__tf_dict__":[],"__tf_data__":[],"ordinary":true}),
        json!({"__tf_cols__":[0],"__tf_rows__":[[1]]}),
    ] {
        DataRequest::new(value, DataRepresentation::Folded).unwrap();
    }
    assert!(
        DataRequest::new(
            json!({"nested":[{"__tf_cols__":["a"],"__tf_rows__":[[1]]}]}),
            DataRepresentation::Folded
        )
        .is_err()
    );
    let mut rows = vec![json!([0]); 1024];
    rows.push(json!([]));
    let malformed = json!({"__tf_cols__":["x".repeat(4096)],"__tf_rows__":rows});
    assert!(
        matches!(
            DataRequest::new(malformed, DataRepresentation::Folded),
            Err(FitError::InvalidInput(_))
        ),
        "a malformed final row must not allow framing whose earlier rows would expand during legacy validation"
    );
}
