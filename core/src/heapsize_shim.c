#include <stddef.h>

#if defined(__APPLE__)
#include <malloc/malloc.h>
#elif defined(__linux__) || defined(__ANDROID__)
#include <malloc.h>
#endif

/*
 * The numext -> heapsize dependency expects a prefixed jemalloc symbol on
 * Apple, Linux and Android. Fiber Wallet uses the platform allocator, so map
 * the symbol to the platform's usable-size query instead of shipping jemalloc.
 */
#if defined(__APPLE__)
size_t je_malloc_usable_size(const void *ptr) {
  return malloc_size(ptr);
}
#elif defined(__linux__) || defined(__ANDROID__)
size_t je_malloc_usable_size(const void *ptr) {
  return malloc_usable_size((void *)ptr);
}
#endif
