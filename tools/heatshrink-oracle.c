/* Differential oracle for heatshrink.
 * Uses only the public headers (heatshrink_encoder.h / heatshrink_decoder.h).
 * The same source links against the C library or heatshrink-ffi.
 *
 * See tools/DRIVER_FORMAT.md.
 */
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "heatshrink_encoder.h"
#include "heatshrink_decoder.h"

#define POLL_MORE 1
#define OUT_BOUND_MUL 32u
#define OUT_BOUND_ADD 65536u

typedef struct {
    const char *alloc;
    unsigned window;
    unsigned lookahead;
    unsigned input_buffer;
    size_t sink_chunk;
    size_t poll_chunk;
    const uint8_t *payload;
    size_t in_len;
} fixture;

typedef int (*sink_fn)(void *ctx, uint8_t *buf, size_t n, size_t *copied);
typedef int (*poll_fn)(void *ctx, uint8_t *buf, size_t n, size_t *copied);
typedef int (*finish_fn)(void *ctx);

static int g_want_encode;
static int g_want_decode;
static int g_want_roundtrip;

static void die_out(const char *msg) {
    printf("error %s\n", msg);
    exit(2);
}

static int parse_u32(const char *s, unsigned long maxv, unsigned long *out) {
    char *end = NULL;
    unsigned long v;
    if (s == NULL || *s == '\0' || *s == '-' || *s == '+') {
        return -1;
    }
    errno = 0;
    v = strtoul(s, &end, 10);
    if (errno != 0 || end == s || *end != '\0' || v > maxv) {
        return -1;
    }
    *out = v;
    return 0;
}

static int header_line(const uint8_t *buf, size_t len, size_t *pos, char *dst, size_t dst_cap) {
    size_t n = 0;
    if (*pos >= len) {
        return -1;
    }
    while (*pos < len && buf[*pos] != '\n') {
        if (buf[*pos] == '\r' || n + 1 >= dst_cap) {
            return -1;
        }
        dst[n++] = (char)buf[*pos];
        (*pos)++;
    }
    if (*pos >= len || buf[*pos] != '\n') {
        return -1;
    }
    (*pos)++;
    dst[n] = '\0';
    return 0;
}

static void load_fixture(const char *path, fixture *fx, uint8_t **owned) {
    FILE *f;
    long sz;
    uint8_t *buf;
    size_t pos = 0;
    char line[256];
    int saw_alloc = 0, saw_window = 0, saw_lookahead = 0, saw_ibs = 0;
    int saw_sink = 0, saw_poll = 0;
    unsigned long v;

    memset(fx, 0, sizeof(*fx));
    f = fopen(path, "rb");
    if (f == NULL) {
        die_out("open");
    }
    if (fseek(f, 0, SEEK_END) != 0) {
        fclose(f);
        die_out("seek");
    }
    sz = ftell(f);
    if (sz < 0) {
        fclose(f);
        die_out("tell");
    }
    if (fseek(f, 0, SEEK_SET) != 0) {
        fclose(f);
        die_out("seek");
    }
    buf = (uint8_t *)malloc((size_t)sz + 1);
    if (buf == NULL) {
        fclose(f);
        die_out("oom");
    }
    if (sz > 0 && fread(buf, 1, (size_t)sz, f) != (size_t)sz) {
        free(buf);
        fclose(f);
        die_out("read");
    }
    fclose(f);
    buf[sz] = 0;
    *owned = buf;

    if (header_line(buf, (size_t)sz, &pos, line, sizeof line) != 0 || strcmp(line, "HSF1") != 0) {
        die_out("bad_fixture");
    }
    for (;;) {
        char *eq;
        if (header_line(buf, (size_t)sz, &pos, line, sizeof line) != 0) {
            die_out("bad_fixture");
        }
        if (line[0] == '\0') {
            break;
        }
        eq = strchr(line, '=');
        if (eq == NULL) {
            die_out("bad_fixture");
        }
        *eq = '\0';
        if (strcmp(line, "alloc") == 0) {
            if (saw_alloc) {
                die_out("bad_fixture");
            }
            if (strcmp(eq + 1, "dyn") != 0 && strcmp(eq + 1, "static") != 0) {
                die_out("bad_fixture");
            }
            fx->alloc = strcmp(eq + 1, "dyn") == 0 ? "dyn" : "static";
            saw_alloc = 1;
        } else if (strcmp(line, "window") == 0) {
            if (saw_window || parse_u32(eq + 1, 255, &v) != 0) {
                die_out("bad_fixture");
            }
            fx->window = (unsigned)v;
            saw_window = 1;
        } else if (strcmp(line, "lookahead") == 0) {
            if (saw_lookahead || parse_u32(eq + 1, 255, &v) != 0) {
                die_out("bad_fixture");
            }
            fx->lookahead = (unsigned)v;
            saw_lookahead = 1;
        } else if (strcmp(line, "input_buffer") == 0) {
            if (saw_ibs || parse_u32(eq + 1, 65535, &v) != 0) {
                die_out("bad_fixture");
            }
            fx->input_buffer = (unsigned)v;
            saw_ibs = 1;
        } else if (strcmp(line, "sink_chunk") == 0) {
            if (saw_sink || parse_u32(eq + 1, 0xFFFFFFFFul, &v) != 0) {
                die_out("bad_fixture");
            }
            fx->sink_chunk = (size_t)v;
            saw_sink = 1;
        } else if (strcmp(line, "poll_chunk") == 0) {
            if (saw_poll || parse_u32(eq + 1, 0xFFFFFFFFul, &v) != 0) {
                die_out("bad_fixture");
            }
            fx->poll_chunk = (size_t)v;
            saw_poll = 1;
        } else {
            die_out("bad_fixture");
        }
    }
    if (!saw_alloc || !saw_window || !saw_lookahead || !saw_ibs || !saw_sink || !saw_poll) {
        die_out("bad_fixture");
    }
    if (strcmp(fx->alloc, "static") == 0) {
        if (fx->window != 8 || fx->lookahead != 4 || fx->input_buffer != 32) {
            die_out("static_config");
        }
    }
    fx->payload = buf + pos;
    fx->in_len = (size_t)sz - pos;
}

static void print_hex(const char *label, const uint8_t *p, size_t n) {
    static const char hexd[] = "0123456789abcdef";
    char tmp[4096];
    size_t k = 0;
    size_t i;
    fputs(label, stdout);
    if (n == 0) {
        fputs(" -\n", stdout);
        return;
    }
    fputc(' ', stdout);
    for (i = 0; i < n; i++) {
        tmp[k++] = hexd[p[i] >> 4];
        tmp[k++] = hexd[p[i] & 0x0f];
        if (k == sizeof tmp) {
            fwrite(tmp, 1, k, stdout);
            k = 0;
        }
    }
    if (k) {
        fwrite(tmp, 1, k, stdout);
    }
    fputc('\n', stdout);
}

static int over_bound(size_t produced, size_t in_len) {
    if (in_len > (SIZE_MAX - OUT_BOUND_ADD) / OUT_BOUND_MUL) {
        return 0;
    }
    return produced > in_len * OUT_BOUND_MUL + OUT_BOUND_ADD;
}

static int ensure_cap(uint8_t **buf, size_t *cap, size_t need) {
    size_t ncap;
    uint8_t *grown;
    if (need <= *cap) {
        return 0;
    }
    ncap = *cap == 0 ? 8 : *cap;
    while (ncap < need) {
        if (ncap > SIZE_MAX / 2) {
            return -1;
        }
        ncap *= 2;
    }
    grown = (uint8_t *)realloc(*buf, ncap);
    if (grown == NULL) {
        return -1;
    }
    *buf = grown;
    *cap = ncap;
    return 0;
}

/* Streaming loop from the C test runners: sink, finish once the input is
 * exhausted, poll until not MORE, finish again once the input is exhausted.
 * sink_chunk/poll_chunk of 0 means "as much as the library will take" / the
 * whole remaining output buffer. */
static const char *run_stream(void *ctx, sink_fn sink, poll_fn poll, finish_fn finish,
                              const uint8_t *in, size_t in_len,
                              size_t sink_chunk, size_t poll_chunk,
                              uint8_t **out, size_t *out_len) {
    size_t cap = in_len + in_len / 2 + 4;
    size_t sunk = 0;
    size_t polled = 0;
    uint8_t *buf;
    if (cap < 4) {
        cap = 4;
    }
    buf = (uint8_t *)malloc(cap);
    if (buf == NULL) {
        die_out("oom");
    }
    memset(buf, 0, cap);
    while (sunk < in_len) {
        size_t want = in_len - sunk;
        size_t copied = 0;
        size_t polled_before = polled;
        size_t sunk_before = sunk;
        int sres;
        if (sink_chunk != 0 && want > sink_chunk) {
            want = sink_chunk;
        }
        sres = sink(ctx, (uint8_t *)(in + sunk), want, &copied);
        printf("op sink %d %zu\n", sres, copied);
        if (sres < 0) {
            *out = buf;
            *out_len = polled;
            return "error";
        }
        sunk += copied;
        if (sunk == in_len) {
            int fres = finish(ctx);
            printf("op finish %d\n", fres);
        }
        for (;;) {
            size_t space;
            size_t got = 0;
            int pres;
            if (polled == cap) {
                if (ensure_cap(&buf, &cap, cap + 1) != 0) {
                    free(buf);
                    die_out("oom");
                }
            }
            space = cap - polled;
            if (poll_chunk != 0 && space > poll_chunk) {
                space = poll_chunk;
            }
            pres = poll(ctx, buf + polled, space, &got);
            printf("op poll %d %zu\n", pres, got);
            polled += got;
            if (pres < 0) {
                *out = buf;
                *out_len = polled;
                return "error";
            }
            if (over_bound(polled, in_len)) {
                *out = buf;
                *out_len = polled;
                return "bound";
            }
            if (pres != POLL_MORE) {
                break;
            }
            if (got == 0) {
                *out = buf;
                *out_len = polled;
                return "stall";
            }
        }
        if (sunk == in_len) {
            int fres = finish(ctx);
            printf("op finish %d\n", fres);
            /* A 1-byte poll can return EMPTY while flush still owes a byte
             * (encoder FLUSH_BITS falls through to EMPTY). Keep cranking
             * while finish reports MORE, which is also how the tiny-buffer
             * C tests drain the stream. FINISH_MORE is 1 for both sides. */
            while (fres == 1) {
                size_t before = polled;
                for (;;) {
                    size_t space;
                    size_t got = 0;
                    int pres;
                    if (polled == cap) {
                        if (ensure_cap(&buf, &cap, cap + 1) != 0) {
                            free(buf);
                            die_out("oom");
                        }
                    }
                    space = cap - polled;
                    if (poll_chunk != 0 && space > poll_chunk) {
                        space = poll_chunk;
                    }
                    pres = poll(ctx, buf + polled, space, &got);
                    printf("op poll %d %zu\n", pres, got);
                    polled += got;
                    if (pres < 0) {
                        *out = buf;
                        *out_len = polled;
                        return "error";
                    }
                    if (over_bound(polled, in_len)) {
                        *out = buf;
                        *out_len = polled;
                        return "bound";
                    }
                    if (pres != POLL_MORE) {
                        break;
                    }
                    if (got == 0) {
                        *out = buf;
                        *out_len = polled;
                        return "stall";
                    }
                }
                fres = finish(ctx);
                printf("op finish %d\n", fres);
                if (polled == before) {
                    *out = buf;
                    *out_len = polled;
                    return "stall";
                }
            }
        }
        if (sunk == sunk_before && polled == polled_before) {
            *out = buf;
            *out_len = polled;
            return "stall";
        }
    }
    *out = buf;
    *out_len = polled;
    return "ok";
}

static int enc_sink(void *ctx, uint8_t *buf, size_t n, size_t *copied) {
    return (int)heatshrink_encoder_sink((heatshrink_encoder *)ctx, buf, n, copied);
}
static int enc_poll(void *ctx, uint8_t *buf, size_t n, size_t *copied) {
    return (int)heatshrink_encoder_poll((heatshrink_encoder *)ctx, buf, n, copied);
}
static int enc_finish(void *ctx) {
    return (int)heatshrink_encoder_finish((heatshrink_encoder *)ctx);
}
static int dec_sink(void *ctx, uint8_t *buf, size_t n, size_t *copied) {
    return (int)heatshrink_decoder_sink((heatshrink_decoder *)ctx, buf, n, copied);
}
static int dec_poll(void *ctx, uint8_t *buf, size_t n, size_t *copied) {
    return (int)heatshrink_decoder_poll((heatshrink_decoder *)ctx, buf, n, copied);
}
static int dec_finish(void *ctx) {
    return (int)heatshrink_decoder_finish((heatshrink_decoder *)ctx);
}

static void print_cfg(const fixture *fx) {
    printf("cfg alloc=%s window=%u lookahead=%u input_buffer=%u sink_chunk=%zu poll_chunk=%zu in_len=%zu\n",
           fx->alloc, fx->window, fx->lookahead, fx->input_buffer,
           fx->sink_chunk, fx->poll_chunk, fx->in_len);
}

static void emit_body(int alloc_ok, const char *status, const uint8_t *bytes, size_t n) {
    if (!alloc_ok) {
        printf("status alloc_null\n");
        printf("nbytes 0\n");
        print_hex("hex", NULL, 0);
        return;
    }
    printf("status %s\n", status);
    printf("nbytes %zu\n", n);
    print_hex("hex", bytes, n);
}

static void section_encode(const fixture *fx) {
    heatshrink_encoder *hse;
    uint8_t *out = NULL;
    size_t out_len = 0;
    const char *status = "alloc_null";
    int ok;
    printf("SECTION encode\n");
    print_cfg(fx);
    hse = heatshrink_encoder_alloc((uint8_t)fx->window, (uint8_t)fx->lookahead);
    ok = hse != NULL;
    printf("alloc %s\n", ok ? "ok" : "null");
    if (ok) {
        status = run_stream(hse, enc_sink, enc_poll, enc_finish,
                            fx->payload, fx->in_len, fx->sink_chunk, fx->poll_chunk,
                            &out, &out_len);
        heatshrink_encoder_free(hse);
    }
    emit_body(ok, status, out, out_len);
    free(out);
    printf("END encode\n");
}

static void section_decode(const fixture *fx) {
    heatshrink_decoder *hsd;
    uint8_t *out = NULL;
    size_t out_len = 0;
    const char *status = "alloc_null";
    int ok;
    printf("SECTION decode\n");
    print_cfg(fx);
    hsd = heatshrink_decoder_alloc((uint16_t)fx->input_buffer,
                                   (uint8_t)fx->window, (uint8_t)fx->lookahead);
    ok = hsd != NULL;
    printf("alloc %s\n", ok ? "ok" : "null");
    if (ok) {
        status = run_stream(hsd, dec_sink, dec_poll, dec_finish,
                            fx->payload, fx->in_len, fx->sink_chunk, fx->poll_chunk,
                            &out, &out_len);
        heatshrink_decoder_free(hsd);
    }
    emit_body(ok, status, out, out_len);
    free(out);
    printf("END decode\n");
}

static int bytes_eq(const uint8_t *a, size_t na, const uint8_t *b, size_t nb) {
    if (na != nb) {
        return 0;
    }
    if (na == 0) {
        return 1;
    }
    return memcmp(a, b, na) == 0;
}

static void section_roundtrip(const fixture *fx) {
    heatshrink_encoder *hse;
    heatshrink_decoder *hsd = NULL;
    uint8_t *comp = NULL;
    uint8_t *plain = NULL;
    size_t comp_len = 0;
    size_t plain_len = 0;
    const char *enc_status = "alloc_null";
    const char *dec_status = "alloc_null";
    int enc_ok;
    int dec_ok = 0;
    int match = 0;

    printf("SECTION roundtrip\n");
    print_cfg(fx);
    hse = heatshrink_encoder_alloc((uint8_t)fx->window, (uint8_t)fx->lookahead);
    enc_ok = hse != NULL;
    printf("enc_alloc %s\n", enc_ok ? "ok" : "null");
    if (enc_ok) {
        enc_status = run_stream(hse, enc_sink, enc_poll, enc_finish,
                                fx->payload, fx->in_len, fx->sink_chunk, fx->poll_chunk,
                                &comp, &comp_len);
        heatshrink_encoder_free(hse);
    }
    emit_body(enc_ok, enc_status, comp, comp_len);

    /* Decode only after a finished encode. A failed encode records dec_alloc null. */
    if (enc_ok && strcmp(enc_status, "ok") == 0) {
        hsd = heatshrink_decoder_alloc((uint16_t)fx->input_buffer,
                                       (uint8_t)fx->window, (uint8_t)fx->lookahead);
        dec_ok = hsd != NULL;
    }
    printf("dec_alloc %s\n", dec_ok ? "ok" : "null");
    if (dec_ok) {
        dec_status = run_stream(hsd, dec_sink, dec_poll, dec_finish,
                                comp, comp_len, fx->sink_chunk, fx->poll_chunk,
                                &plain, &plain_len);
        heatshrink_decoder_free(hsd);
    }
    emit_body(dec_ok, dec_status, plain, plain_len);
    if (dec_ok && strcmp(dec_status, "ok") == 0) {
        match = bytes_eq(fx->payload, fx->in_len, plain, plain_len);
    }
    printf("match %d\n", match);
    printf("END roundtrip\n");
    free(comp);
    free(plain);
}

static void usage(void) {
    fprintf(stderr, "usage: heatshrink-oracle <fixture> [--sections encode,decode,roundtrip]\n");
}

static int set_sections(const char *list) {
    const char *p;
    if (list == NULL || list[0] == '\0') {
        return -1;
    }
    g_want_encode = 0;
    g_want_decode = 0;
    g_want_roundtrip = 0;
    p = list;
    while (*p != '\0') {
        const char *comma = strchr(p, ',');
        size_t n = comma != NULL ? (size_t)(comma - p) : strlen(p);
        if (n == 6 && memcmp(p, "encode", 6) == 0) {
            g_want_encode = 1;
        } else if (n == 6 && memcmp(p, "decode", 6) == 0) {
            g_want_decode = 1;
        } else if (n == 9 && memcmp(p, "roundtrip", 9) == 0) {
            g_want_roundtrip = 1;
        } else {
            return -1;
        }
        if (comma == NULL) {
            break;
        }
        p = comma + 1;
        if (*p == '\0') {
            return -1;
        }
    }
    if (!g_want_encode && !g_want_decode && !g_want_roundtrip) {
        return -1;
    }
    return 0;
}

int main(int argc, char **argv) {
    const char *path = NULL;
    fixture fx;
    uint8_t *owned = NULL;
    int i;

    g_want_encode = 1;
    g_want_decode = 1;
    g_want_roundtrip = 1;

    for (i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--sections") == 0) {
            if (i + 1 >= argc || set_sections(argv[++i]) != 0) {
                usage();
                die_out("usage");
            }
        } else if (strncmp(argv[i], "--sections=", 11) == 0) {
            if (set_sections(argv[i] + 11) != 0) {
                usage();
                die_out("usage");
            }
        } else if (strcmp(argv[i], "--help") == 0) {
            usage();
            return 2;
        } else if (argv[i][0] == '-') {
            usage();
            die_out("usage");
        } else if (path != NULL) {
            usage();
            die_out("usage");
        } else {
            path = argv[i];
        }
    }
    if (path == NULL) {
        usage();
        die_out("usage");
    }
    load_fixture(path, &fx, &owned);
    if (g_want_encode) {
        section_encode(&fx);
    }
    if (g_want_decode) {
        section_decode(&fx);
    }
    if (g_want_roundtrip) {
        section_roundtrip(&fx);
    }
    free(owned);
    return 0;
}
