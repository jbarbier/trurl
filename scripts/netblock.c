/*
 * netblock: an LD_PRELOAD shim that makes a process (and its children) behave
 * as if Wi-Fi were off, and records every attempt to reach the network.
 *
 * Any connect() or sendto() to an address that is not loopback fails with
 * ENETUNREACH and is logged to $NETBLOCK_LOG. Loopback (127.0.0.0/8, ::1) and
 * Unix sockets pass, so Trurl can still talk to its own llama-server.
 *
 * Build:  gcc -shared -fPIC -O2 -o netblock.so scripts/netblock.c -ldl
 * Use:    NETBLOCK_LOG=/tmp/net.log LD_PRELOAD=$PWD/netblock.so <command>
 * The offline test asserts the log stays empty.
 */
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <dlfcn.h>
#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static int is_allowed(const struct sockaddr *a) {
    if (!a) return 1;
    if (a->sa_family == AF_UNIX || a->sa_family == AF_UNSPEC) return 1;
    if (a->sa_family == AF_INET) {
        const struct sockaddr_in *in = (const struct sockaddr_in *)a;
        return (ntohl(in->sin_addr.s_addr) >> 24) == 127;
    }
    if (a->sa_family == AF_INET6) {
        const struct sockaddr_in6 *in6 = (const struct sockaddr_in6 *)a;
        if (IN6_IS_ADDR_LOOPBACK(&in6->sin6_addr)) return 1;
        if (IN6_IS_ADDR_V4MAPPED(&in6->sin6_addr)) return in6->sin6_addr.s6_addr[12] == 127;
        return 0;
    }
    return 0; /* netlink and friends: deny, they are not needed */
}

static void record(const char *fn, const struct sockaddr *a) {
    const char *path = getenv("NETBLOCK_LOG");
    if (!path) return;
    FILE *f = fopen(path, "a");
    if (!f) return;
    char host[INET6_ADDRSTRLEN] = "?";
    int port = 0;
    if (a && a->sa_family == AF_INET) {
        const struct sockaddr_in *in = (const struct sockaddr_in *)a;
        inet_ntop(AF_INET, &in->sin_addr, host, sizeof host);
        port = ntohs(in->sin_port);
    } else if (a && a->sa_family == AF_INET6) {
        const struct sockaddr_in6 *in6 = (const struct sockaddr_in6 *)a;
        inet_ntop(AF_INET6, &in6->sin6_addr, host, sizeof host);
        port = ntohs(in6->sin6_port);
    }
    fprintf(f, "pid=%d %s family=%d %s:%d\n", (int)getpid(), fn, a ? a->sa_family : -1, host, port);
    fclose(f);
}

int connect(int fd, const struct sockaddr *a, socklen_t len) {
    static int (*real)(int, const struct sockaddr *, socklen_t);
    if (!real) real = dlsym(RTLD_NEXT, "connect");
    if (!is_allowed(a)) {
        record("connect", a);
        errno = ENETUNREACH;
        return -1;
    }
    return real(fd, a, len);
}

ssize_t sendto(int fd, const void *buf, size_t n, int flags, const struct sockaddr *a, socklen_t len) {
    static ssize_t (*real)(int, const void *, size_t, int, const struct sockaddr *, socklen_t);
    if (!real) real = dlsym(RTLD_NEXT, "sendto");
    if (a && !is_allowed(a)) {
        record("sendto", a);
        errno = ENETUNREACH;
        return -1;
    }
    return real(fd, buf, n, flags, a, len);
}
