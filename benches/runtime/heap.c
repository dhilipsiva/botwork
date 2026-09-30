// Counts the peak heap of a dynamically linked GNU Linux process: the private
// memory of the CLI in measurement protocol 2 (scripts/performance.py). Preload
// it with LD_PRELOAD; it forwards every allocation to glibc and, when the
// process exits normally, writes the peak in bytes to the file that
// BOTWORK_HEAP_REPORT names. Sizes are malloc_usable_size, so each allocation
// counts with its allocator rounding.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <malloc.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>

extern void *__libc_malloc(size_t size);
extern void *__libc_calloc(size_t count, size_t size);
extern void *__libc_realloc(void *pointer, size_t size);
extern void *__libc_memalign(size_t alignment, size_t size);
extern void __libc_free(void *pointer);

// Signed, so a free of memory allocated before the preload took effect cannot
// wrap the count around.
static long long live, peak;

static void add(long long bytes) {
    long long now = __atomic_add_fetch(&live, bytes, __ATOMIC_RELAXED);
    long long seen = __atomic_load_n(&peak, __ATOMIC_RELAXED);
    while (now > seen && !__atomic_compare_exchange_n(&peak, &seen, now, 1, __ATOMIC_RELAXED,
                                                      __ATOMIC_RELAXED)) {
    }
}

static void *counted(void *pointer) {
    if (pointer) {
        add((long long)malloc_usable_size(pointer));
    }
    return pointer;
}

void *malloc(size_t size) { return counted(__libc_malloc(size)); }

void *calloc(size_t count, size_t size) { return counted(__libc_calloc(count, size)); }

void free(void *pointer) {
    if (pointer) {
        __atomic_sub_fetch(&live, (long long)malloc_usable_size(pointer), __ATOMIC_RELAXED);
    }
    __libc_free(pointer);
}

void *realloc(void *pointer, size_t size) {
    long long before = pointer ? (long long)malloc_usable_size(pointer) : 0;
    void *moved = __libc_realloc(pointer, size);
    if (moved) {
        add((long long)malloc_usable_size(moved) - before);
    } else if (pointer && size == 0) {
        add(-before); // glibc frees the block and returns null.
    }
    return moved;
}

void *reallocarray(void *pointer, size_t count, size_t size) {
    size_t bytes;
    if (__builtin_mul_overflow(count, size, &bytes)) {
        errno = ENOMEM;
        return NULL;
    }
    return realloc(pointer, bytes);
}

void *memalign(size_t alignment, size_t size) {
    return counted(__libc_memalign(alignment, size));
}

void *aligned_alloc(size_t alignment, size_t size) { return memalign(alignment, size); }

int posix_memalign(void **out, size_t alignment, size_t size) {
    if (alignment < sizeof(void *) || (alignment & (alignment - 1)) != 0) {
        return EINVAL;
    }
    void *pointer = memalign(alignment, size);
    if (!pointer) {
        return ENOMEM;
    }
    *out = pointer;
    return 0;
}

void *valloc(size_t size) { return memalign((size_t)sysconf(_SC_PAGESIZE), size); }

void *pvalloc(size_t size) {
    size_t page = (size_t)sysconf(_SC_PAGESIZE);
    if (size > SIZE_MAX - page) {
        errno = ENOMEM;
        return NULL;
    }
    return memalign(page, (size + page - 1) & ~(page - 1));
}

// Formats without allocating, since the allocator is being counted.
__attribute__((destructor)) static void report(void) {
    const char *path = getenv("BOTWORK_HEAP_REPORT");
    if (!path) {
        return;
    }
    char digits[24];
    size_t at = sizeof digits;
    digits[--at] = '\n';
    unsigned long long value = (unsigned long long)__atomic_load_n(&peak, __ATOMIC_RELAXED);
    do {
        digits[--at] = (char)('0' + value % 10);
        value /= 10;
    } while (value);
    int file = open(path, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0644);
    if (file >= 0) {
        ssize_t written = write(file, digits + at, sizeof digits - at);
        (void)written;
        close(file);
    }
}
