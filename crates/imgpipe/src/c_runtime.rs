//! The few C library functions the C codecs (libwebp, libdeflate) need on
//! `wasm32-unknown-unknown`, which has no libc: an allocator over Rust's,
//! and `qsort`. The symbols are compiled only for wasm with the opt-in
//! `wasm-libc-shim` feature, because they are global and would clash with
//! any other definition in the final binary (the sort itself is also built
//! for native tests). Threads are never used there: build libwebp with
//! `-UWEBP_USE_THREAD -U_THREAD_SAFE` in `CFLAGS_wasm32_unknown_unknown`.

use std::ffi::c_void;

#[cfg(all(target_arch = "wasm32", feature = "wasm-libc-shim"))]
mod alloc_shim {
    use std::alloc::{Layout, alloc, alloc_zeroed, dealloc, realloc as rs_realloc};
    use std::ffi::c_void;

    /// Every block starts with this header, which records its size.
    const HEADER: usize = 16;

    fn layout(size: usize) -> Option<Layout> {
        Layout::from_size_align(size.checked_add(HEADER)?, HEADER).ok()
    }

    unsafe fn finish(base: *mut u8, size: usize) -> *mut c_void {
        if base.is_null() {
            return std::ptr::null_mut();
        }
        unsafe {
            (base as *mut usize).write(size);
            base.add(HEADER).cast()
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
        let Some(l) = layout(size) else {
            return std::ptr::null_mut();
        };
        unsafe { finish(alloc(l), size) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn calloc(n: usize, size: usize) -> *mut c_void {
        let Some(total) = n.checked_mul(size) else {
            return std::ptr::null_mut();
        };
        let Some(l) = layout(total) else {
            return std::ptr::null_mut();
        };
        unsafe { finish(alloc_zeroed(l), total) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
        if ptr.is_null() {
            return unsafe { malloc(size) };
        }
        unsafe {
            let base = (ptr as *mut u8).sub(HEADER);
            let old = (base as *const usize).read();
            let (Some(old_l), Some(_)) = (layout(old), layout(size)) else {
                return std::ptr::null_mut();
            };
            finish(rs_realloc(base, old_l, size + HEADER), size)
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn free(ptr: *mut c_void) {
        if ptr.is_null() {
            return;
        }
        unsafe {
            let base = (ptr as *mut u8).sub(HEADER);
            let size = (base as *const usize).read();
            if let Some(l) = layout(size) {
                dealloc(base, l);
            }
        }
    }
}

/// `qsort`: sorts an index with the C comparator, then permutes the bytes.
///
/// A C comparator need not be a total order (it may be inconsistent, or
/// compare by a key that changes), and `qsort` must still return. Rust's
/// `sort_by` and `sort_unstable_by` may panic on a non-total order, and a
/// panic in an `extern "C"` function aborts the process, so this uses
/// [`heapsort`], which only ever compares and swaps in-bounds indices: a bad
/// comparator gives an unspecified order, never a panic.
#[cfg(all(target_arch = "wasm32", feature = "wasm-libc-shim"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qsort(
    base: *mut c_void,
    n: usize,
    size: usize,
    cmp: unsafe extern "C" fn(*const c_void, *const c_void) -> i32,
) {
    unsafe { qsort_impl(base, n, size, cmp) }
}

/// [`qsort`]'s body, callable from tests on any target.
unsafe fn qsort_impl(
    base: *mut c_void,
    n: usize,
    size: usize,
    cmp: unsafe extern "C" fn(*const c_void, *const c_void) -> i32,
) {
    if n < 2 || size == 0 {
        return;
    }
    let Some(total) = n.checked_mul(size) else {
        return;
    };
    let bytes = base as *mut u8;
    let mut order: Vec<usize> = (0..n).collect();
    heapsort(&mut order, |a, b| unsafe {
        cmp(bytes.add(a * size).cast(), bytes.add(b * size).cast()) < 0
    });
    let copy = unsafe { std::slice::from_raw_parts(bytes, total) }.to_vec();
    for (dst, &src) in order.iter().enumerate() {
        unsafe {
            std::ptr::copy_nonoverlapping(
                copy.as_ptr().add(src * size),
                bytes.add(dst * size),
                size,
            );
        }
    }
}

/// In-place heapsort of `v` by `less`. Every index it touches is in bounds
/// whatever `less` returns, so it cannot panic (the result is only sorted
/// when `less` is a strict weak order).
fn heapsort(v: &mut [usize], mut less: impl FnMut(usize, usize) -> bool) {
    let mut sift_down = |v: &mut [usize], mut node: usize, end: usize| {
        loop {
            let mut child = 2 * node + 1;
            if child >= end {
                break;
            }
            if child + 1 < end && less(v[child], v[child + 1]) {
                child += 1;
            }
            if !less(v[node], v[child]) {
                break;
            }
            v.swap(node, child);
            node = child;
        }
    };
    let n = v.len();
    for i in (0..n / 2).rev() {
        sift_down(v, i, n);
    }
    for end in (1..n).rev() {
        v.swap(0, end);
        sift_down(v, 0, end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    unsafe extern "C" fn by_value(a: *const c_void, b: *const c_void) -> i32 {
        let (a, b) = unsafe { (*(a as *const u32), *(b as *const u32)) };
        a.cmp(&b) as i32
    }

    static STATE: AtomicU32 = AtomicU32::new(0x9e37_79b9);

    /// Answers at random: no order at all.
    unsafe extern "C" fn inconsistent(_: *const c_void, _: *const c_void) -> i32 {
        let mut x = STATE.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        STATE.store(x, Ordering::Relaxed);
        (x % 3) as i32 - 1
    }

    fn values(n: u32) -> Vec<u32> {
        (0..n)
            .map(|i| i.wrapping_mul(2_654_435_761) % 1000)
            .collect()
    }

    #[test]
    fn sorts_like_qsort() {
        for n in [0, 1, 2, 3, 10, 257] {
            let mut v = values(n);
            unsafe { qsort_impl(v.as_mut_ptr().cast(), v.len(), 4, by_value) };
            let mut expected = values(n);
            expected.sort();
            assert_eq!(v, expected);
        }
    }

    #[test]
    fn an_inconsistent_comparator_never_panics() {
        for n in [2, 20, 33, 1000] {
            let mut v = values(n);
            unsafe { qsort_impl(v.as_mut_ptr().cast(), v.len(), 4, inconsistent) };
            // Still a permutation of the input.
            let (mut got, mut expected) = (v, values(n));
            got.sort();
            expected.sort();
            assert_eq!(got, expected);
        }
    }
}
