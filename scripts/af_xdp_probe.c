// Raw L2 echo test for the FDS AF_XDP veth harness.
// Sends one crafted IPv4/UDP frame into veth0 via veth1 and prints
// whatever comes back. Root (CAP_NET_RAW) required.
// Usage: sudo ./af_xdp_probe veth1 46:36:0d:fa:5a:ba aa:cf:b8:57:08:f3
#include <arpa/inet.h>
#include <errno.h>
#include <linux/if_ether.h>
#include <linux/if_packet.h>
#include <net/if.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static unsigned short csum16(const void *data, int len) {
    const unsigned short *p = data;
    unsigned int sum = 0;
    while (len > 1) { sum += *p++; len -= 2; }
    if (len) sum += *(const unsigned char *)p;
    while (sum >> 16) sum = (sum & 0xffff) + (sum >> 16);
    return (unsigned short)~sum;
}

static int hexmac(const char *s, unsigned char *out) {
    return sscanf(s, "%hhx:%hhx:%hhx:%hhx:%hhx:%hhx",
                  &out[0], &out[1], &out[2], &out[3], &out[4], &out[5]) == 6;
}

int main(int argc, char **argv) {
    if (argc < 4) { fprintf(stderr, "usage: %s iface dstmac srcmac\n", argv[0]); return 2; }
    const char *iface = argv[1];
    unsigned char dmac[6], smac[6];
    if (!hexmac(argv[2], dmac) || !hexmac(argv[3], smac)) { fprintf(stderr, "bad mac\n"); return 2; }

    int ifidx = if_nametoindex(iface);
    if (!ifidx) { perror("if_nametoindex"); return 1; }

    int fd = socket(AF_PACKET, SOCK_RAW, htons(ETH_P_ALL));
    if (fd < 0) { perror("socket"); return 1; }
    // Bind to the interface so recv() only sees frames on it (an
    // unbound AF_PACKET socket would also catch wifi traffic).
    struct sockaddr_ll bindll = {0};
    bindll.sll_family = AF_PACKET;
    bindll.sll_protocol = htons(ETH_P_ALL);
    bindll.sll_ifindex = ifidx;
    if (bind(fd, (struct sockaddr *)&bindll, sizeof(bindll)) < 0) { perror("bind"); return 1; }
    struct sockaddr_ll sll = {0};
    sll.sll_family = AF_PACKET;
    sll.sll_ifindex = ifidx;
    sll.sll_halen = ETH_ALEN;
    memcpy(sll.sll_addr, dmac, ETH_ALEN);

    // Ethernet + IPv4 + UDP + payload.
    unsigned char frame[64];
    memset(frame, 0, sizeof(frame));
    memcpy(frame + 0, dmac, 6);
    memcpy(frame + 6, smac, 6);
    frame[12] = 0x08; frame[13] = 0x00;

    struct { unsigned char vhl, tos; unsigned short len, id, frag; unsigned char ttl, proto; unsigned short csum; unsigned int src, dst; } *ip = (void *)(frame + 14);
    ip->vhl = 0x45;
    ip->len = htons(20 + 8 + 6);
    ip->id = htons(1);
    ip->ttl = 64;
    ip->proto = 17; // UDP
    ip->src = inet_addr("10.9.9.2");
    ip->dst = inet_addr("10.9.9.1");
    ip->csum = 0;
    ip->csum = csum16(frame + 14, 20);

    struct { unsigned short sport, dport, len; unsigned short csum; } *udp = (void *)(frame + 34);
    udp->sport = htons(31337);
    udp->dport = htons(7777);
    udp->len = htons(8 + 6);
    udp->csum = 0;
    memcpy(frame + 42, "xdp-e2e", 6);

    int n = sendto(fd, frame, 48, 0, (struct sockaddr *)&sll, sizeof(sll));
    if (n < 0) { perror("sendto"); return 1; }
    printf("sent %d bytes\n", n);

    // Wait for the echoed frame (MAC-swapped back to us).
    fd_set rfds; struct timeval tv = {2, 0};
    FD_ZERO(&rfds); FD_SET(fd, &rfds);
    int r = select(fd + 1, &rfds, NULL, NULL, &tv);
    if (r <= 0) { printf("no reply\n"); return 1; }
    unsigned char buf[2048];
    n = recv(fd, buf, sizeof(buf), 0);
    if (n < 0) { perror("recv"); return 1; }
    printf("reply %d bytes: dst %02x:%02x:%02x:%02x:%02x:%02x src %02x:%02x:%02x:%02x:%02x:%02x\n",
           n, buf[0], buf[1], buf[2], buf[3], buf[4], buf[5],
           buf[6], buf[7], buf[8], buf[9], buf[10], buf[11]);
    // Reply dst should be our MAC (aa:cf...:f3) and src the veth0 MAC.
    int ok = memcmp(buf, smac, 6) == 0 && memcmp(buf + 6, dmac, 6) == 0;
    printf(ok ? "ECHO OK (MACs swapped)\n" : "ECHO MISMATCH\n");
    return ok ? 0 : 1;
}
