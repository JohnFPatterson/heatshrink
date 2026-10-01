/* Allocator-hook trace for the dynamic public API.
 * Link with -Wl,--wrap=malloc -Wl,--wrap=free against either the C sources
 * or libheatshrink_ffi. Logging uses write() so the trace itself does not
 * allocate. Free sizes are the sizes passed to the matching malloc.
 */
#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "heatshrink_encoder.h"
#include "heatshrink_decoder.h"

void *__real_malloc(size_t);
void __real_free(void *);

static int g_fail_on;
static int g_alloc_n;

#define MAP_MAX 32
static struct {
    void *p;
    size_t sz;
    int used;
} g_map[MAP_MAX];

static void emit(const char *kind, size_t sz) {
    char buf[64];
    int n = snprintf(buf, sizeof buf, "%s %zu\n", kind, sz);
    if (n > 0) {
        (void)write(STDOUT_FILENO, buf, (size_t)n);
    }
}

void *__wrap_malloc(size_t sz) {
    g_alloc_n++;
    if (g_fail_on && g_alloc_n == g_fail_on) {
        emit("alloc_fail", sz);
        return NULL;
    }
    void *p = __real_malloc(sz);
    if (p == NULL) {
        emit("alloc_fail", sz);
        return NULL;
    }
    emit("alloc", sz);
    for (int i = 0; i < MAP_MAX; i++) {
        if (!g_map[i].used) {
            g_map[i].used = 1;
            g_map[i].p = p;
            g_map[i].sz = sz;
            break;
        }
    }
    return p;
}

void __wrap_free(void *p) {
    if (p == NULL) {
        __real_free(p);
        return;
    }
    size_t sz = 0;
    for (int i = 0; i < MAP_MAX; i++) {
        if (g_map[i].used && g_map[i].p == p) {
            sz = g_map[i].sz;
            g_map[i].used = 0;
            break;
        }
    }
    emit("free", sz);
    __real_free(p);
}

static void begin(int fail_on) {
    g_fail_on = fail_on;
    g_alloc_n = 0;
}

static void case_name(const char *name) {
    char buf[80];
    int n = snprintf(buf, sizeof buf, "CASE %s\n", name);
    if (n > 0) {
        (void)write(STDOUT_FILENO, buf, (size_t)n);
    }
}

int main(void) {
    case_name("encoder_round");
    begin(0);
    heatshrink_encoder *hse = heatshrink_encoder_alloc(8, 7);
    uint8_t in[4] = {1, 2, 3, 4};
    size_t n = 0;
    (void)heatshrink_encoder_sink(hse, in, sizeof in, &n);
    uint8_t out[32];
    (void)heatshrink_encoder_poll(hse, out, sizeof out, &n);
    (void)heatshrink_encoder_finish(hse);
    (void)heatshrink_encoder_poll(hse, out, sizeof out, &n);
    heatshrink_encoder_reset(hse);
    heatshrink_encoder_free(hse);

    case_name("encoder_fail_first");
    begin(1);
    hse = heatshrink_encoder_alloc(8, 7);
    if (hse != NULL) {
        heatshrink_encoder_free(hse);
    }

    case_name("encoder_fail_second");
    begin(2);
    hse = heatshrink_encoder_alloc(8, 7);
    if (hse != NULL) {
        heatshrink_encoder_free(hse);
    }

    case_name("encoder_invalid");
    begin(0);
    hse = heatshrink_encoder_alloc(3, 8);
    hse = heatshrink_encoder_alloc(16, 8);
    hse = heatshrink_encoder_alloc(8, 2);
    hse = heatshrink_encoder_alloc(8, 9);
    (void)hse;

    case_name("decoder_round");
    begin(0);
    heatshrink_decoder *hsd = heatshrink_decoder_alloc(256, 8, 4);
    uint8_t comp[4] = {0xb3, 0x5b, 0xed, 0xe0};
    (void)heatshrink_decoder_sink(hsd, comp, sizeof comp, &n);
    (void)heatshrink_decoder_poll(hsd, out, sizeof out, &n);
    (void)heatshrink_decoder_finish(hsd);
    heatshrink_decoder_reset(hsd);
    heatshrink_decoder_free(hsd);

    case_name("decoder_fail_first");
    begin(1);
    hsd = heatshrink_decoder_alloc(256, 8, 4);
    if (hsd != NULL) {
        heatshrink_decoder_free(hsd);
    }

    case_name("decoder_invalid");
    begin(0);
    hsd = heatshrink_decoder_alloc(0, 4, 3);
    hsd = heatshrink_decoder_alloc(256, 3, 4);
    hsd = heatshrink_decoder_alloc(1, 4, 4);
    hsd = heatshrink_decoder_alloc(1, 4, 5);
    (void)hsd;
    return 0;
}
