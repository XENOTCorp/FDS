// XDP redirect-to-XSK for the FDS AF_XDP bench harness.
// BTF-defined maps (libbpf v1.0+); no bpf_helpers.h; the macros are
// defined here and the helper is called via a function pointer.
#include <linux/bpf.h>
#include <linux/if_ether.h>

#define SEC(NAME) __attribute__((section(NAME), used))
#define __uint(name, val) int (*name)[val]
#define __type(name, val) typeof(val) *name
#define LIBBPF_PIN_BY_NAME 1

struct {
    __uint(type, BPF_MAP_TYPE_XSKMAP);
    __uint(max_entries, 1);
    __type(key, unsigned int);
    __type(value, unsigned int);
    __uint(pinning, LIBBPF_PIN_BY_NAME);
} xskmap SEC(".maps");

static int (*const bpf_redirect_map)(void *map, unsigned int key, unsigned int flags) =
    (void *)BPF_FUNC_redirect_map;

SEC("xdp")
int xdp_redirect_to_xsk(struct xdp_md *ctx) {
    return bpf_redirect_map(&xskmap, 0, 0);
}

char _license[] SEC("license") = "GPL";
