/* Minimal AF_XDP rxdrop, the same shape as linux samples/bpf/xdpsock -r.
 * Bind native zero-copy (copy fallback), count RX frames, return them
 * to the fill ring. Used by scripts/bench-afxdp-xdpsock.sh.
 *
 * Usage: xdpsock_rxdrop <ifname> <queue> [seconds]
 */
#define _GNU_SOURCE
#include <errno.h>
#include <linux/if_link.h>
#include <linux/if_xdp.h>
#include <net/if.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

#ifndef AF_XDP
#define AF_XDP 44
#endif
#ifndef SOL_XDP
#define SOL_XDP 283
#endif

#define FRAME_SIZE 4096
#define NUM_FRAMES 4096
#define RING 256

static volatile int stop;

static void on_int(int s) { (void)s; stop = 1; }

static uint64_t nsec(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: %s ifname queue [seconds]\n", argv[0]);
        return 2;
    }
    const char *ifname = argv[1];
    unsigned queue = (unsigned)atoi(argv[2]);
    int seconds = argc > 3 ? atoi(argv[3]) : 3;
    unsigned ifindex = if_nametoindex(ifname);
    if (!ifindex) { perror("if_nametoindex"); return 1; }

    int fd = socket(AF_XDP, SOCK_RAW | SOCK_CLOEXEC, 0);
    if (fd < 0) { perror("socket AF_XDP"); return 1; }

    size_t umem_len = (size_t)NUM_FRAMES * FRAME_SIZE;
    void *umem = mmap(NULL, umem_len, PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (umem == MAP_FAILED) { perror("mmap umem"); return 1; }

    struct xdp_umem_reg reg = {
        .addr = (uint64_t)(uintptr_t)umem,
        .len = umem_len,
        .chunk_size = FRAME_SIZE,
    };
    if (setsockopt(fd, SOL_XDP, XDP_UMEM_REG, &reg, sizeof(reg)) < 0) {
        perror("XDP_UMEM_REG"); return 1;
    }
    unsigned ring = RING;
    if (setsockopt(fd, SOL_XDP, XDP_RX_RING, &ring, sizeof(ring)) < 0 ||
        setsockopt(fd, SOL_XDP, XDP_TX_RING, &ring, sizeof(ring)) < 0 ||
        setsockopt(fd, SOL_XDP, XDP_UMEM_FILL_RING, &ring, sizeof(ring)) < 0 ||
        setsockopt(fd, SOL_XDP, XDP_UMEM_COMPLETION_RING, &ring, sizeof(ring)) < 0) {
        perror("ring size"); return 1;
    }
    struct xdp_mmap_offsets off;
    socklen_t olen = sizeof(off);
    if (getsockopt(fd, SOL_XDP, XDP_MMAP_OFFSETS, &off, &olen) < 0) {
        perror("XDP_MMAP_OFFSETS"); return 1;
    }
    size_t rx_len = off.rx.desc + RING * sizeof(struct xdp_desc);
    size_t fr_len = off.fr.desc + RING * sizeof(uint64_t);
    void *rx = mmap(NULL, rx_len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    void *fr = mmap(NULL, fr_len, PROT_READ | PROT_WRITE, MAP_SHARED, fd,
                    0x100000000ULL);
    if (rx == MAP_FAILED || fr == MAP_FAILED) { perror("mmap rings"); return 1; }

    uint32_t *fr_prod = (uint32_t *)((char *)fr + off.fr.producer);
    uint64_t *fr_desc = (uint64_t *)((char *)fr + off.fr.desc);
    uint32_t *rx_prod = (uint32_t *)((char *)rx + off.rx.producer);
    uint32_t *rx_cons = (uint32_t *)((char *)rx + off.rx.consumer);
    struct xdp_desc *rx_desc = (struct xdp_desc *)((char *)rx + off.rx.desc);

    for (unsigned i = 0; i < RING; i++) fr_desc[i] = (uint64_t)i * FRAME_SIZE;
    __atomic_store_n(fr_prod, RING, __ATOMIC_RELEASE);

    struct sockaddr_xdp sx = {0};
    sx.sxdp_family = AF_XDP;
    sx.sxdp_ifindex = ifindex;
    sx.sxdp_queue_id = queue;
    sx.sxdp_flags = XDP_ZEROCOPY | XDP_USE_NEED_WAKEUP;
    if (bind(fd, (struct sockaddr *)&sx, sizeof(sx)) < 0) {
        sx.sxdp_flags = XDP_COPY | XDP_USE_NEED_WAKEUP;
        if (bind(fd, (struct sockaddr *)&sx, sizeof(sx)) < 0) {
            perror("bind"); return 1;
        }
        fprintf(stderr, "xdpsock_rxdrop: copy mode\n");
    } else {
        fprintf(stderr, "xdpsock_rxdrop: zero-copy mode\n");
    }

    signal(SIGINT, on_int);
    uint32_t rx_tail = 0, fill_head = RING;
    uint64_t pkts = 0, t0 = nsec();
    uint64_t deadline = t0 + (uint64_t)seconds * 1000000000ull;
    while (!stop && nsec() < deadline) {
        uint32_t head = __atomic_load_n(rx_prod, __ATOMIC_ACQUIRE);
        while (rx_tail != head) {
            struct xdp_desc d = rx_desc[rx_tail & (RING - 1)];
            rx_tail++;
            fr_desc[fill_head & (RING - 1)] = d.addr;
            fill_head++;
            pkts++;
        }
        __atomic_store_n(rx_cons, rx_tail, __ATOMIC_RELEASE);
        __atomic_store_n(fr_prod, fill_head, __ATOMIC_RELEASE);
        if (head == rx_tail) {
            struct pollfd p = { .fd = fd, .events = POLLIN };
            poll(&p, 1, 1);
        }
    }
    double s = (nsec() - t0) / 1e9;
    printf("xdpsock_rxdrop: %s queue %u, %.0f pps (%.2f Mpps) in %.2fs, %llu pkts\n",
           ifname, queue, pkts / s, pkts / s / 1e6, s, (unsigned long long)pkts);
    return 0;
}
