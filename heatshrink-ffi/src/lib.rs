//! Thin C ABI for the dynamic `heatshrink_encoder.h` / `heatshrink_decoder.h` API.
//!
//! Allocation sizes and order match `HEATSHRINK_MALLOC` / `HEATSHRINK_FREE` in
//! the dynamic build (`HEATSHRINK_USE_INDEX` 1). The static build has no
//! alloc/free symbols; those constants are constructed in `heatshrink-core`.

#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use heatshrink_core::internals::{DecoderState, EncoderState};
use heatshrink_core::{Error, SinkFull};
use libc::c_int;

static TRACE_ON: AtomicBool = AtomicBool::new(false);

struct TraceState {
    fail_on: u32,
    allocs: u32,
    log: Vec<(u8, usize)>,
}

static TRACE: Mutex<TraceState> = Mutex::new(TraceState {
    fail_on: 0,
    allocs: 0,
    log: Vec::new(),
});

/// Record malloc/free size and order. `fail_on` is 1-based; 0 disables failures.
/// Not a C export. Used to diff allocator behavior against the C library.
pub fn testing_trace_reset(fail_on: u32) {
    TRACE_ON.store(true, Ordering::SeqCst);
    let mut guard = TRACE.lock().unwrap_or_else(|e| e.into_inner());
    guard.fail_on = fail_on;
    guard.allocs = 0;
    guard.log.clear();
}

pub fn testing_trace_stop() -> Vec<(u8, usize)> {
    TRACE_ON.store(false, Ordering::SeqCst);
    let mut guard = TRACE.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut guard.log)
}

fn hs_malloc(size: usize) -> *mut libc::c_void {
    if TRACE_ON.load(Ordering::Relaxed) {
        let mut guard = TRACE.lock().unwrap_or_else(|e| e.into_inner());
        guard.allocs = guard.allocs.wrapping_add(1);
        if guard.fail_on != 0 && guard.allocs == guard.fail_on {
            guard.log.push((2, size));
            return std::ptr::null_mut();
        }
        guard.log.push((1, size));
    }
    // SAFETY: libc::malloc is the same allocator the C library uses. A null
    // return is propagated to the caller, matching `HEATSHRINK_MALLOC`.
    unsafe { libc::malloc(size) }
}

fn hs_free(ptr: *mut libc::c_void, size: usize) {
    if TRACE_ON.load(Ordering::Relaxed) {
        let mut guard = TRACE.lock().unwrap_or_else(|e| e.into_inner());
        guard.log.push((0, size));
    }
    // SAFETY: `ptr` came from hs_malloc, or this is the matching
    // HEATSHRINK_FREE of that block. `size` is only logged.
    unsafe { libc::free(ptr) }
}

#[repr(C)]
struct HsIndex {
    size: u16,
}

#[repr(C)]
pub struct heatshrink_encoder {
    pub input_size: u16,
    pub match_scan_index: u16,
    pub match_length: u16,
    pub match_pos: u16,
    pub outgoing_bits: u16,
    pub outgoing_bits_count: u8,
    pub flags: u8,
    pub state: u8,
    pub current_byte: u8,
    pub bit_index: u8,
    pub window_sz2: u8,
    pub lookahead_sz2: u8,
    _pad: [u8; 7],
    search_index: *mut HsIndex,
}

#[repr(C)]
pub struct heatshrink_decoder {
    pub input_size: u16,
    pub input_index: u16,
    pub output_count: u16,
    pub output_index: u16,
    pub head_index: u16,
    pub state: u8,
    pub current_byte: u8,
    pub bit_index: u8,
    pub window_sz2: u8,
    pub lookahead_sz2: u8,
    pub input_buffer_size: u16,
}

const _: () = assert!(std::mem::size_of::<heatshrink_encoder>() == 32);
const _: () = assert!(std::mem::size_of::<heatshrink_decoder>() == 18);
const _: () = assert!(std::mem::size_of::<HsIndex>() == 2);
const _: () = assert!(std::mem::offset_of!(heatshrink_encoder, search_index) == 24);
const _: () = assert!(std::mem::offset_of!(heatshrink_decoder, input_buffer_size) == 16);

fn enc_alloc_size(window_sz2: u8) -> usize {
    std::mem::size_of::<heatshrink_encoder>() + (2usize << window_sz2)
}

fn index_alloc_size(window_sz2: u8) -> usize {
    let buf_sz = 2usize << window_sz2;
    std::mem::size_of::<HsIndex>() + buf_sz * std::mem::size_of::<u16>()
}

fn dec_alloc_size(window_sz2: u8, input_buffer_size: u16) -> usize {
    std::mem::size_of::<heatshrink_decoder>() + (1usize << window_sz2) + input_buffer_size as usize
}

fn load_enc(hse: &heatshrink_encoder) -> EncoderState {
    EncoderState {
        input_size: hse.input_size,
        match_scan_index: hse.match_scan_index,
        match_length: hse.match_length,
        match_pos: hse.match_pos,
        outgoing_bits: hse.outgoing_bits,
        outgoing_bits_count: hse.outgoing_bits_count,
        flags: hse.flags,
        state: hse.state,
        current_byte: hse.current_byte,
        bit_index: hse.bit_index,
        window_sz2: hse.window_sz2,
        lookahead_sz2: hse.lookahead_sz2,
    }
}

fn store_enc(hse: &mut heatshrink_encoder, state: &EncoderState) {
    hse.input_size = state.input_size;
    hse.match_scan_index = state.match_scan_index;
    hse.match_length = state.match_length;
    hse.match_pos = state.match_pos;
    hse.outgoing_bits = state.outgoing_bits;
    hse.outgoing_bits_count = state.outgoing_bits_count;
    hse.flags = state.flags;
    hse.state = state.state;
    hse.current_byte = state.current_byte;
    hse.bit_index = state.bit_index;
    hse.window_sz2 = state.window_sz2;
    hse.lookahead_sz2 = state.lookahead_sz2;
}

fn load_dec(hsd: &heatshrink_decoder) -> DecoderState {
    DecoderState {
        input_size: hsd.input_size,
        input_index: hsd.input_index,
        output_count: hsd.output_count,
        output_index: hsd.output_index,
        head_index: hsd.head_index,
        state: hsd.state,
        current_byte: hsd.current_byte,
        bit_index: hsd.bit_index,
        window_sz2: hsd.window_sz2,
        lookahead_sz2: hsd.lookahead_sz2,
        input_buffer_size: hsd.input_buffer_size,
    }
}

fn store_dec(hsd: &mut heatshrink_decoder, state: &DecoderState) {
    hsd.input_size = state.input_size;
    hsd.input_index = state.input_index;
    hsd.output_count = state.output_count;
    hsd.output_index = state.output_index;
    hsd.head_index = state.head_index;
    hsd.state = state.state;
    hsd.current_byte = state.current_byte;
    hsd.bit_index = state.bit_index;
    hsd.window_sz2 = state.window_sz2;
    hsd.lookahead_sz2 = state.lookahead_sz2;
    hsd.input_buffer_size = state.input_buffer_size;
}

fn with_encoder<R>(
    hse: *mut heatshrink_encoder,
    f: impl FnOnce(&mut EncoderState, &mut [u8], &mut [i16]) -> R,
) -> R {
    // SAFETY: `hse` is a live encoder. The shared header borrow ends before
    // the tail buffer and the separate index allocation are borrowed.
    let mut state = unsafe { load_enc(&*hse) };
    let result = {
        // SAFETY: the byte buffer is the tail of the encoder allocation
        // (`2 << window` bytes). The index is the second malloc: a u16 size
        // followed by that many int16 values. Neither alias the header borrow
        // above, which has ended.
        let (buffer, index) = unsafe {
            let window = (*hse).window_sz2;
            let buf_sz = 2usize << window;
            let bytes = (hse as *mut u8).add(std::mem::size_of::<heatshrink_encoder>());
            let buffer = std::slice::from_raw_parts_mut(bytes, buf_sz);
            let idx = (*hse).search_index;
            let index_ptr = (idx as *mut u8).add(std::mem::size_of::<HsIndex>()) as *mut i16;
            let index = std::slice::from_raw_parts_mut(index_ptr, buf_sz);
            (buffer, index)
        };
        f(&mut state, buffer, index)
    };
    // SAFETY: buffer and index borrows ended with the block above. The header
    // is written back and does not cover the tail.
    unsafe { store_enc(&mut *hse, &state) };
    result
}

fn with_decoder<R>(
    hsd: *mut heatshrink_decoder,
    f: impl FnOnce(&mut DecoderState, &mut [u8]) -> R,
) -> R {
    // SAFETY: `hsd` is a live decoder. The header borrow ends before the tail
    // buffer is borrowed.
    let mut state = unsafe { load_dec(&*hsd) };
    let result = {
        // SAFETY: the tail is `input_buffer_size + (1 << window)` bytes
        // immediately after the 18-byte header.
        let buffers = unsafe {
            let window = (*hsd).window_sz2;
            let ibs = (*hsd).input_buffer_size as usize;
            let n = (1usize << window) + ibs;
            let bytes = (hsd as *mut u8).add(std::mem::size_of::<heatshrink_decoder>());
            std::slice::from_raw_parts_mut(bytes, n)
        };
        f(&mut state, buffers)
    };
    // SAFETY: the tail borrow ended. Only header fields are written.
    unsafe { store_dec(&mut *hsd, &state) };
    result
}

#[no_mangle]
pub extern "C" fn heatshrink_encoder_alloc(
    window_sz2: u8,
    lookahead_sz2: u8,
) -> *mut heatshrink_encoder {
    if !(heatshrink_core::MIN_WINDOW_BITS..=heatshrink_core::MAX_WINDOW_BITS).contains(&window_sz2)
        || lookahead_sz2 < heatshrink_core::MIN_LOOKAHEAD_BITS
        || lookahead_sz2 >= window_sz2
    {
        return std::ptr::null_mut();
    }
    let enc_sz = enc_alloc_size(window_sz2);
    let hse = hs_malloc(enc_sz) as *mut heatshrink_encoder;
    if hse.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: `hse` is a fresh enc_sz-byte allocation. The header is written
    // before any read, then the tail is the sliding buffer reset by the core.
    unsafe {
        std::ptr::write(
            hse,
            heatshrink_encoder {
                input_size: 0,
                match_scan_index: 0,
                match_length: 0,
                match_pos: 0,
                outgoing_bits: 0,
                outgoing_bits_count: 0,
                flags: 0,
                state: 0,
                current_byte: 0,
                bit_index: 0,
                window_sz2,
                lookahead_sz2,
                _pad: [0; 7],
                search_index: std::ptr::null_mut(),
            },
        );
        let buf_sz = 2usize << window_sz2;
        let bytes = (hse as *mut u8).add(std::mem::size_of::<heatshrink_encoder>());
        let buffer = std::slice::from_raw_parts_mut(bytes, buf_sz);
        let mut state = EncoderState::fresh(window_sz2, lookahead_sz2);
        state.reset(buffer);
        store_enc(&mut *hse, &state);
    }
    let idx_sz = index_alloc_size(window_sz2);
    let idx = hs_malloc(idx_sz) as *mut HsIndex;
    if idx.is_null() {
        hs_free(hse as *mut libc::c_void, enc_sz);
        return std::ptr::null_mut();
    }
    // SAFETY: `idx` is the index allocation. `size` is the byte length of the
    // int16 array, truncated to u16 the way `hs_index.size` is assigned in C.
    unsafe {
        let buf_sz = 2usize << window_sz2;
        (*idx).size = (buf_sz * std::mem::size_of::<u16>()) as u16;
        (*hse).search_index = idx;
    }
    hse
}

/// # Safety
/// Pointer arguments are null (the C error return) or a live object from the
/// matching `*_alloc` that has not been freed. Each `unsafe` block in the body
/// states the specific contract.
#[no_mangle]
pub unsafe extern "C" fn heatshrink_encoder_free(hse: *mut heatshrink_encoder) {
    if hse.is_null() {
        // C dereferences the pointer before free.
        std::process::abort();
    }
    // SAFETY: `hse` was allocated by heatshrink_encoder_alloc. Index is freed
    // first, then the encoder block, matching heatshrink_encoder_free.
    unsafe {
        let window = (*hse).window_sz2;
        let idx = (*hse).search_index;
        let index_free = std::mem::size_of::<HsIndex>() + (*idx).size as usize;
        let enc_free = enc_alloc_size(window);
        hs_free(idx as *mut libc::c_void, index_free);
        hs_free(hse as *mut libc::c_void, enc_free);
    }
}

/// # Safety
/// Pointer arguments are null (the C error return) or a live object from the
/// matching `*_alloc` that has not been freed. Each `unsafe` block in the body
/// states the specific contract.
#[no_mangle]
pub unsafe extern "C" fn heatshrink_encoder_reset(hse: *mut heatshrink_encoder) {
    if hse.is_null() {
        std::process::abort();
    }
    with_encoder(hse, |state, buffer, _index| state.reset(buffer));
}

/// # Safety
/// Pointer arguments are null (the C error return) or a live object from the
/// matching `*_alloc` that has not been freed. Each `unsafe` block in the body
/// states the specific contract.
#[no_mangle]
pub unsafe extern "C" fn heatshrink_encoder_sink(
    hse: *mut heatshrink_encoder,
    in_buf: *mut u8,
    size: usize,
    input_size: *mut usize,
) -> c_int {
    if hse.is_null() || in_buf.is_null() || input_size.is_null() {
        return -1;
    }
    // SAFETY: `in_buf` points at `size` caller-owned bytes for this call.
    let input = unsafe { std::slice::from_raw_parts(in_buf, size) };
    let result = with_encoder(hse, |state, buffer, _index| state.sink(buffer, input));
    match result {
        Ok(n) => {
            // SAFETY: `input_size` is non-null. C writes it only on success.
            unsafe { *input_size = n };
            0
        }
        Err(Error::Misuse) => -2,
        Err(_) => -2,
    }
}

/// # Safety
/// Pointer arguments are null (the C error return) or a live object from the
/// matching `*_alloc` that has not been freed. Each `unsafe` block in the body
/// states the specific contract.
#[no_mangle]
pub unsafe extern "C" fn heatshrink_encoder_poll(
    hse: *mut heatshrink_encoder,
    out_buf: *mut u8,
    out_buf_size: usize,
    output_size: *mut usize,
) -> c_int {
    if hse.is_null() || out_buf.is_null() || output_size.is_null() {
        return -1;
    }
    if out_buf_size == 0 {
        return -2;
    }
    // SAFETY: `out_buf` points at `out_buf_size` writable caller bytes.
    let out = unsafe { std::slice::from_raw_parts_mut(out_buf, out_buf_size) };
    let result = with_encoder(hse, |state, buffer, index| state.poll(buffer, index, out));
    match result {
        Ok(p) => {
            // SAFETY: `output_size` is non-null. C writes the produced count.
            unsafe { *output_size = p.n };
            if p.more {
                1
            } else {
                0
            }
        }
        Err(_) => -2,
    }
}

/// # Safety
/// Pointer arguments are null (the C error return) or a live object from the
/// matching `*_alloc` that has not been freed. Each `unsafe` block in the body
/// states the specific contract.
#[no_mangle]
pub unsafe extern "C" fn heatshrink_encoder_finish(hse: *mut heatshrink_encoder) -> c_int {
    if hse.is_null() {
        return -1;
    }
    let done = with_encoder(hse, |state, _buffer, _index| state.finish());
    if done {
        0
    } else {
        1
    }
}

#[no_mangle]
pub extern "C" fn heatshrink_decoder_alloc(
    input_buffer_size: u16,
    expansion_buffer_sz2: u8,
    lookahead_sz2: u8,
) -> *mut heatshrink_decoder {
    if input_buffer_size == 0
        || !((heatshrink_core::MIN_WINDOW_BITS..=heatshrink_core::MAX_WINDOW_BITS)
            .contains(&expansion_buffer_sz2)
            && lookahead_sz2 >= heatshrink_core::MIN_LOOKAHEAD_BITS
            && lookahead_sz2 < expansion_buffer_sz2)
    {
        return std::ptr::null_mut();
    }
    let sz = dec_alloc_size(expansion_buffer_sz2, input_buffer_size);
    let hsd = hs_malloc(sz) as *mut heatshrink_decoder;
    if hsd.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: `hsd` is a fresh `sz`-byte allocation. The header is initialized
    // before the tail buffer is reset.
    unsafe {
        std::ptr::write(
            hsd,
            heatshrink_decoder {
                input_size: 0,
                input_index: 0,
                output_count: 0,
                output_index: 0,
                head_index: 0,
                state: 0,
                current_byte: 0,
                bit_index: 0,
                window_sz2: expansion_buffer_sz2,
                lookahead_sz2,
                input_buffer_size,
            },
        );
        let n = (1usize << expansion_buffer_sz2) + input_buffer_size as usize;
        let bytes = (hsd as *mut u8).add(std::mem::size_of::<heatshrink_decoder>());
        let buffers = std::slice::from_raw_parts_mut(bytes, n);
        let mut state = DecoderState::fresh(input_buffer_size, expansion_buffer_sz2, lookahead_sz2);
        state.reset(buffers);
        store_dec(&mut *hsd, &state);
    }
    hsd
}

/// # Safety
/// Pointer arguments are null (the C error return) or a live object from the
/// matching `*_alloc` that has not been freed. Each `unsafe` block in the body
/// states the specific contract.
#[no_mangle]
pub unsafe extern "C" fn heatshrink_decoder_free(hsd: *mut heatshrink_decoder) {
    if hsd.is_null() {
        std::process::abort();
    }
    // SAFETY: `hsd` was allocated by heatshrink_decoder_alloc. One free, with
    // the same size expression as heatshrink_decoder_free.
    unsafe {
        let sz = dec_alloc_size((*hsd).window_sz2, (*hsd).input_buffer_size);
        hs_free(hsd as *mut libc::c_void, sz);
    }
}

/// # Safety
/// Pointer arguments are null (the C error return) or a live object from the
/// matching `*_alloc` that has not been freed. Each `unsafe` block in the body
/// states the specific contract.
#[no_mangle]
pub unsafe extern "C" fn heatshrink_decoder_reset(hsd: *mut heatshrink_decoder) {
    if hsd.is_null() {
        std::process::abort();
    }
    with_decoder(hsd, |state, buffers| state.reset(buffers));
}

/// # Safety
/// Pointer arguments are null (the C error return) or a live object from the
/// matching `*_alloc` that has not been freed. Each `unsafe` block in the body
/// states the specific contract.
#[no_mangle]
pub unsafe extern "C" fn heatshrink_decoder_sink(
    hsd: *mut heatshrink_decoder,
    in_buf: *mut u8,
    size: usize,
    input_size: *mut usize,
) -> c_int {
    if hsd.is_null() || in_buf.is_null() || input_size.is_null() {
        return -1;
    }
    // SAFETY: `in_buf` points at `size` caller-owned bytes for this call.
    let input = unsafe { std::slice::from_raw_parts(in_buf, size) };
    let result = with_decoder(hsd, |state, buffers| state.sink(buffers, input));
    match result {
        Ok(n) => {
            // SAFETY: `input_size` is non-null. C writes the copied count.
            unsafe { *input_size = n };
            0
        }
        Err(SinkFull) => {
            // SAFETY: `input_size` is non-null. C writes 0 on HSDR_SINK_FULL.
            unsafe { *input_size = 0 };
            1
        }
    }
}

/// # Safety
/// Pointer arguments are null (the C error return) or a live object from the
/// matching `*_alloc` that has not been freed. Each `unsafe` block in the body
/// states the specific contract.
#[no_mangle]
pub unsafe extern "C" fn heatshrink_decoder_poll(
    hsd: *mut heatshrink_decoder,
    out_buf: *mut u8,
    out_buf_size: usize,
    output_size: *mut usize,
) -> c_int {
    if hsd.is_null() || out_buf.is_null() || output_size.is_null() {
        return -1;
    }
    // SAFETY: `out_buf` points at `out_buf_size` writable caller bytes.
    let out = unsafe { std::slice::from_raw_parts_mut(out_buf, out_buf_size) };
    let result = with_decoder(hsd, |state, buffers| state.poll(buffers, out));
    match result {
        Ok(p) => {
            // SAFETY: `output_size` is non-null. C writes the produced count.
            unsafe { *output_size = p.n };
            if p.more {
                1
            } else {
                0
            }
        }
        Err(_) => {
            // SAFETY: `output_size` is non-null.
            unsafe { *output_size = 0 };
            -2
        }
    }
}

/// # Safety
/// Pointer arguments are null (the C error return) or a live object from the
/// matching `*_alloc` that has not been freed. Each `unsafe` block in the body
/// states the specific contract.
#[no_mangle]
pub unsafe extern "C" fn heatshrink_decoder_finish(hsd: *mut heatshrink_decoder) -> c_int {
    if hsd.is_null() {
        return -1;
    }
    // SAFETY: `hsd` is non-null and points at a live decoder header. finish
    // does not mutate, so the tail buffer is not borrowed.
    let state = unsafe { load_dec(&*hsd) };
    if state.finish() {
        0
    } else {
        1
    }
}
