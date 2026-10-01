//! Pointer and status contracts the byte driver does not print.

use heatshrink_ffi::{
    heatshrink_decoder_alloc, heatshrink_decoder_finish, heatshrink_decoder_free,
    heatshrink_decoder_poll, heatshrink_decoder_sink, heatshrink_encoder_alloc,
    heatshrink_encoder_finish, heatshrink_encoder_free, heatshrink_encoder_poll,
    heatshrink_encoder_reset, heatshrink_encoder_sink, testing_trace_reset, testing_trace_stop,
};

// SAFETY: every helper below is called only with null (the C null-error path)
// or with a pointer returned by the matching alloc in the same test and not yet freed.
fn enc_free(p: *mut heatshrink_ffi::heatshrink_encoder) {
    unsafe { heatshrink_encoder_free(p) }
}
fn enc_reset(p: *mut heatshrink_ffi::heatshrink_encoder) {
    unsafe { heatshrink_encoder_reset(p) }
}
fn enc_sink(
    p: *mut heatshrink_ffi::heatshrink_encoder,
    buf: *mut u8,
    n: usize,
    out: *mut usize,
) -> i32 {
    unsafe { heatshrink_encoder_sink(p, buf, n, out) }
}
fn enc_poll(
    p: *mut heatshrink_ffi::heatshrink_encoder,
    buf: *mut u8,
    n: usize,
    out: *mut usize,
) -> i32 {
    unsafe { heatshrink_encoder_poll(p, buf, n, out) }
}
fn enc_finish(p: *mut heatshrink_ffi::heatshrink_encoder) -> i32 {
    unsafe { heatshrink_encoder_finish(p) }
}
fn dec_free(p: *mut heatshrink_ffi::heatshrink_decoder) {
    unsafe { heatshrink_decoder_free(p) }
}
fn dec_sink(
    p: *mut heatshrink_ffi::heatshrink_decoder,
    buf: *mut u8,
    n: usize,
    out: *mut usize,
) -> i32 {
    unsafe { heatshrink_decoder_sink(p, buf, n, out) }
}
fn dec_poll(
    p: *mut heatshrink_ffi::heatshrink_decoder,
    buf: *mut u8,
    n: usize,
    out: *mut usize,
) -> i32 {
    unsafe { heatshrink_decoder_poll(p, buf, n, out) }
}
fn dec_finish(p: *mut heatshrink_ffi::heatshrink_decoder) -> i32 {
    unsafe { heatshrink_decoder_finish(p) }
}

fn trace_lines(fail_on: u32, body: impl FnOnce()) -> String {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    testing_trace_reset(fail_on);
    body();
    let log = testing_trace_stop();
    let mut out = String::new();
    for (kind, size) in log {
        let name = match kind {
            1 => "alloc",
            2 => "alloc_fail",
            _ => "free",
        };
        out.push_str(&format!("{name} {size}\n"));
    }
    out
}

#[test]
fn encoder_alloc_sizes_match_c_window_8() {
    // sizeof(heatshrink_encoder)=32, buf=2<<8=512, index=2+512*2.
    let log = trace_lines(0, || {
        let hse = heatshrink_encoder_alloc(8, 7);
        assert!(!hse.is_null());
        enc_free(hse);
    });
    assert_eq!(log, "alloc 544\nalloc 1026\nfree 1026\nfree 544\n");
}

#[test]
fn encoder_second_alloc_failure_frees_the_first() {
    let log = trace_lines(2, || {
        let hse = heatshrink_encoder_alloc(8, 7);
        assert!(hse.is_null());
    });
    assert_eq!(log, "alloc 544\nalloc_fail 1026\nfree 544\n");
}

#[test]
fn encoder_first_alloc_failure_allocates_nothing_else() {
    let log = trace_lines(1, || {
        assert!(heatshrink_encoder_alloc(8, 7).is_null());
    });
    assert_eq!(log, "alloc_fail 544\n");
}

#[test]
fn invalid_encoder_args_do_not_allocate() {
    let log = trace_lines(0, || {
        assert!(heatshrink_encoder_alloc(3, 8).is_null());
        assert!(heatshrink_encoder_alloc(16, 8).is_null());
        assert!(heatshrink_encoder_alloc(8, 2).is_null());
        assert!(heatshrink_encoder_alloc(8, 9).is_null());
    });
    assert_eq!(log, "");
}

#[test]
fn decoder_alloc_size_and_free() {
    // sizeof(decoder)=18, window 256, input 256 -> 530.
    let log = trace_lines(0, || {
        let hsd = heatshrink_decoder_alloc(256, 8, 4);
        assert!(!hsd.is_null());
        dec_free(hsd);
    });
    assert_eq!(log, "alloc 530\nfree 530\n");
}

#[test]
fn decoder_alloc_failure_and_invalid_args() {
    let log = trace_lines(1, || {
        assert!(heatshrink_decoder_alloc(256, 8, 4).is_null());
    });
    assert_eq!(log, "alloc_fail 530\n");
    let log = trace_lines(0, || {
        assert!(heatshrink_decoder_alloc(0, 4, 3).is_null());
        assert!(heatshrink_decoder_alloc(256, 3, 4).is_null());
        assert!(heatshrink_decoder_alloc(1, 4, 4).is_null());
        assert!(heatshrink_decoder_alloc(1, 4, 5).is_null());
    });
    assert_eq!(log, "");
}

#[test]
fn null_pointers_return_error_codes() {
    let mut n = 0usize;
    let mut byte = 0u8;
    let hse = heatshrink_encoder_alloc(8, 7);
    assert_eq!(enc_sink(std::ptr::null_mut(), &mut byte, 1, &mut n), -1);
    assert_eq!(enc_sink(hse, std::ptr::null_mut(), 1, &mut n), -1);
    assert_eq!(enc_sink(hse, &mut byte, 1, std::ptr::null_mut()), -1);
    assert_eq!(enc_poll(std::ptr::null_mut(), &mut byte, 1, &mut n), -1);
    assert_eq!(enc_poll(hse, std::ptr::null_mut(), 1, &mut n), -1);
    assert_eq!(enc_poll(hse, &mut byte, 1, std::ptr::null_mut()), -1);
    assert_eq!(enc_poll(hse, &mut byte, 0, &mut n), -2);
    assert_eq!(enc_finish(std::ptr::null_mut()), -1);
    enc_free(hse);

    let hsd = heatshrink_decoder_alloc(32, 8, 4);
    assert_eq!(dec_sink(std::ptr::null_mut(), &mut byte, 1, &mut n), -1);
    assert_eq!(dec_sink(hsd, std::ptr::null_mut(), 1, &mut n), -1);
    assert_eq!(dec_sink(hsd, &mut byte, 1, std::ptr::null_mut()), -1);
    assert_eq!(dec_poll(std::ptr::null_mut(), &mut byte, 1, &mut n), -1);
    assert_eq!(dec_poll(hsd, std::ptr::null_mut(), 1, &mut n), -1);
    assert_eq!(dec_poll(hsd, &mut byte, 1, std::ptr::null_mut()), -1);
    assert_eq!(dec_finish(std::ptr::null_mut()), -1);
    dec_free(hsd);
}

#[test]
fn visible_input_fields_match_after_sink() {
    let hsd = heatshrink_decoder_alloc(256, 4, 3);
    assert!(!hsd.is_null());
    let input = [0u8, 1, 2, 3, 4, 5];
    let mut count = 99usize;
    let rc = dec_sink(hsd, input.as_ptr() as *mut u8, input.len(), &mut count);
    assert_eq!(rc, 0);
    assert_eq!(count, 6);
    // SAFETY: hsd is a live decoder. These are the fields the dynamic C test reads.
    unsafe {
        assert_eq!((*hsd).input_size, 6);
        assert_eq!((*hsd).input_index, 0);
    }
    dec_free(hsd);
}

#[test]
fn ffi_roundtrip_matches_literal_vector() {
    let hse = heatshrink_encoder_alloc(8, 7);
    let input = [0u8, 1, 2, 3, 4];
    let mut copied = 0usize;
    assert_eq!(
        enc_sink(hse, input.as_ptr() as *mut u8, input.len(), &mut copied),
        0
    );
    assert_eq!(copied, 5);
    let mut out = [0u8; 16];
    assert_eq!(enc_poll(hse, out.as_mut_ptr(), out.len(), &mut copied), 0);
    assert_eq!(copied, 0);
    assert_eq!(enc_finish(hse), 1);
    assert_eq!(enc_poll(hse, out.as_mut_ptr(), out.len(), &mut copied), 0);
    assert_eq!(&out[..copied], &[0x80, 0x40, 0x60, 0x50, 0x38, 0x20]);
    assert_eq!(enc_finish(hse), 0);
    enc_reset(hse);
    enc_free(hse);
}

#[test]
fn decoder_sink_full_on_one_byte_buffer() {
    let hsd = heatshrink_decoder_alloc(1, 4, 3);
    let input = [0u8, 1, 2, 3, 4, 5];
    let mut count = 0usize;
    assert_eq!(dec_sink(hsd, input.as_ptr() as *mut u8, 6, &mut count), 0);
    assert_eq!(count, 1);
    assert_eq!(
        dec_sink(hsd, input[1..].as_ptr() as *mut u8, 5, &mut count),
        1
    );
    assert_eq!(count, 0);
    dec_free(hsd);
}
