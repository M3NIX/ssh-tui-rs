#define _GNU_SOURCE

#include <dlfcn.h>
#include <netdb.h>
#include <stddef.h>
#include <string.h>

typedef int (*getaddrinfo_fn)(
    const char *,
    const char *,
    const struct addrinfo *,
    struct addrinfo **
);

int getaddrinfo(
    const char *node,
    const char *service,
    const struct addrinfo *hints,
    struct addrinfo **result
) {
    static getaddrinfo_fn real_getaddrinfo;
    if (real_getaddrinfo == NULL) {
        real_getaddrinfo = (getaddrinfo_fn)dlsym(RTLD_NEXT, "getaddrinfo");
    }

    const char *suffix = ".northstar.internal";
    size_t node_length = node == NULL ? 0 : strlen(node);
    size_t suffix_length = strlen(suffix);
    if (node_length >= suffix_length &&
        strcmp(node + node_length - suffix_length, suffix) == 0) {
        node = "127.0.0.1";
    }

    return real_getaddrinfo(node, service, hints, result);
}
