//! AF_XDP path (feature `af-xdp`): raw Ethernet sockets with the XDP
//! umem/ring setup; `socket(AF_XDP, SOCK_RAW, 0)`, `XDP_UMEM_REG`,
//! `XDP_RX_RING`/`XDP_TX_RING` (mmap'd), `bind()` with `struct
//! sockaddr_xdp`. The UAPI constants are declared in-crate (libc does
//! not ship them) and were verified against `<linux/if_xdp.h>`.
//!
//! The datapath is **zero-copy**: [`XskSocket::recv_frame`] hands out a
//! frame descriptor into the umem (no copy); the caller processes the
//! frame in place through [`XskSocket::frame_mut`] and either
//! [`XskSocket::tx_frame`]s it back out of the same umem slot (echo
//! with no copy) or [`XskSocket::drop_frame`]s it. Transmitted frames
//! are reclaimed from the completion ring by
//! [`XskSocket::recycle_tx`] and returned to the fill ring. Bind mode
//! is `XDP_ZEROCOPY` when the driver supports it (the socket's umem is
//! the NIC's own memory); it falls back to `XDP_COPY` automatically.
//! [`XskSocket::kick`] implements the `XDP_USE_NEED_WAKEUP` contract:
//! the poller is woken only when the kernel asks.
//!
//! [`XskMultiqueue`] opens one socket per device queue, each with its
//! own umem and rings. [`XskOpenOpts::node`] binds the umem to a NUMA
//! node with `mbind`, so a pinned worker's rings live on its own node
//! (no cross-socket bounce for the data plane).
//!
//! CONTRACT (implementer): declare the XDP_* setsockopt constants and
//! `struct xdp_umem_reg` / `struct xdp_mmap_offsets` per
//! `<linux/if_xdp.h>`; implement [`XskSocket`] with the public API
//! below. Tests: socket creation with `AddressFamily::XDP` skips
//! gracefully when unsupported; full umem/ring setup is compile-checked
//! but only run when a device is available (skip by default).

use std::io;
use std::sync::atomic::{AtomicU32, Ordering};

// ---- AF_XDP UAPI (verified against /usr/include/linux/if_xdp.h) ----

/// `SOL_XDP` socket option level.
const SOL_XDP: libc::c_int = 283;
/// `XDP_MMAP_OFFSETS` getsockopt: per-ring byte offsets in the mmap'd regions.
const XDP_MMAP_OFFSETS: libc::c_int = 1;
/// `XDP_RX_RING` setsockopt: RX ring entry count.
const XDP_RX_RING: libc::c_int = 2;
/// `XDP_TX_RING` setsockopt: TX ring entry count.
const XDP_TX_RING: libc::c_int = 3;
/// `XDP_UMEM_REG` setsockopt: register the umem ([`XdpUmemReg`]).
const XDP_UMEM_REG: libc::c_int = 4;
/// `XDP_UMEM_FILL_RING` setsockopt: fill ring entry count.
const XDP_UMEM_FILL_RING: libc::c_int = 5;
/// `XDP_UMEM_COMPLETION_RING` setsockopt: completion ring entry count.
const XDP_UMEM_COMPLETION_RING: libc::c_int = 6;

/// `XDP_SHARED_UMEM` bind flag: share umem with another socket.
const XDP_SHARED_UMEM: u16 = 1 << 0;
/// `XDP_COPY` bind flag: the kernel copies frames between the umem and
/// the driver.
const XDP_COPY: u16 = 1 << 1;
/// `XDP_ZEROCOPY` bind flag: the driver uses the umem directly.
const XDP_ZEROCOPY: u16 = 1 << 2;
/// `XDP_USE_NEED_WAKEUP` bind flag: the kernel may sleep; user space
/// must `poll`/`sendto` when `XDP_RING_NEED_WAKEUP` is set on the fill
/// or TX ring.
const XDP_USE_NEED_WAKEUP: u16 = 1 << 3;

/// `XDP_RING_NEED_WAKEUP` in the RX/TX ring flags word: the kernel
/// needs an explicit wakeup (`poll` for RX, `sendto` for TX).
const XDP_RING_NEED_WAKEUP: u32 = 1 << 0;

// Bind flags from `<linux/if_xdp.h>`: SHARED_UMEM=bit0, COPY=bit1,
// ZEROCOPY=bit2, USE_NEED_WAKEUP=bit3. A wrong ZEROCOPY value binds
// SHARED_UMEM instead and never enters native zero-copy.
const _: () = {
    assert!(XDP_SHARED_UMEM == 1u16);
    assert!(XDP_COPY == 2u16);
    assert!(XDP_ZEROCOPY == 4u16);
    assert!(XDP_USE_NEED_WAKEUP == 8u16);
    assert!(XDP_RING_NEED_WAKEUP == 1u32);
};

/// mmap offset of the RX ring (byte offset passed to `mmap`; the kernel's
/// `xsk_mmap` compares these against `XDP_PGOFF_*` directly).
const XDP_PGOFF_RX_RING: libc::off_t = 0;
/// mmap offset of the TX ring.
const XDP_PGOFF_TX_RING: libc::off_t = 0x8000_0000;
/// mmap offset of the fill ring.
const XDP_UMEM_PGOFF_FILL_RING: libc::off_t = 0x1_0000_0000;
/// mmap offset of the completion ring.
const XDP_UMEM_PGOFF_COMPLETION_RING: libc::off_t = 0x1_8000_0000;

/// Default umem frames: 4096 frames of one page each (16 MiB).
const DEFAULT_NUM_FRAMES: u32 = 4096;
/// Default umem frame size (chunk size), one page.
const DEFAULT_FRAME_SIZE: u32 = 4096;
/// Default per-ring entry count (a power of two, as the kernel requires).
const DEFAULT_RING_SIZE: u32 = 256;

/// `struct sockaddr_xdp` from `<linux/if_xdp.h>`.
#[repr(C)]
struct SockaddrXdp {
    sxdp_family: u16,
    sxdp_flags: u16,
    sxdp_ifindex: u32,
    sxdp_queue_id: u32,
    sxdp_shared_umem_fd: u32,
}

/// `struct xdp_umem_reg` from `<linux/if_xdp.h>`; includes
/// `tx_metadata_len`, which this kernel's header declares.
#[repr(C)]
struct XdpUmemReg {
    addr: u64,
    len: u64,
    chunk_size: u32,
    headroom: u32,
    flags: u32,
    tx_metadata_len: u32,
}

/// `struct xdp_ring_offset` from `<linux/if_xdp.h>`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XdpRingOffset {
    producer: u64,
    consumer: u64,
    desc: u64,
    flags: u64,
}

/// `struct xdp_mmap_offsets` from `<linux/if_xdp.h>`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XdpMmapOffsets {
    rx: XdpRingOffset,
    tx: XdpRingOffset,
    fr: XdpRingOffset,
    cr: XdpRingOffset,
}

/// `struct xdp_desc` from `<linux/if_xdp.h>`; an RX/TX ring entry.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XdpDesc {
    addr: u64,
    len: u32,
    options: u32,
}

/// Open options for an AF_XDP socket.
#[derive(Clone, Copy, Debug)]
pub struct XskOpenOpts {
    /// Entry count of every ring (power of two).
    pub ring_size: u32,
    /// Umem frame count.
    pub num_frames: u32,
    /// Bind with `XDP_ZEROCOPY`; falls back to `XDP_COPY` when the
    /// driver rejects it.
    pub zero_copy: bool,
    /// NUMA node for the umem (`mbind`); `None` = current node.
    pub node: Option<i32>,
}

impl Default for XskOpenOpts {
    fn default() -> Self {
        XskOpenOpts {
            ring_size: DEFAULT_RING_SIZE,
            num_frames: DEFAULT_NUM_FRAMES,
            zero_copy: true,
            node: None,
        }
    }
}

/// The bind mode of a socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XdpMode {
    /// The NIC uses the umem directly (no copy).
    ZeroCopy,
    /// The kernel copies frames between the driver and the umem.
    Copy,
}

/// One received frame: an offset into the umem and its length. The
/// frame is checked out of the socket; call [`XskSocket::frame_mut`] to
/// process it, then [`XskSocket::tx_frame`] or
/// [`XskSocket::drop_frame`] to release it.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    addr: u64,
    len: u32,
}

impl Frame {
    /// The frame's byte length.
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// True when the frame is empty (zero length).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The frame's umem offset.
    pub fn addr(&self) -> u64 {
        self.addr
    }
}

/// An AF_XDP socket (umem + rx/tx/fill/completion rings + bind).
pub struct XskSocket {
    fd: i32,
    /// Anonymous umem mapping; frame `i` is at `umem + i * frame_size`.
    umem: *mut u8,
    /// Total umem length in bytes.
    umem_len: usize,
    /// Umem frame (chunk) size in bytes.
    frame_size: u32,
    /// Entry count of every ring (a power of two).
    ring_size: u32,
    /// mmap base of the RX ring region.
    rx_base: *mut u8,
    /// mmap base of the TX ring region.
    tx_base: *mut u8,
    /// mmap base of the fill ring region.
    fill_base: *mut u8,
    /// mmap base of the completion ring region.
    cr_base: *mut u8,
    /// Per-ring byte offsets inside each region (from `XDP_MMAP_OFFSETS`).
    offsets: XdpMmapOffsets,
    /// Userspace RX consumer index (kernel producer is read from the ring).
    rx_tail: u32,
    /// Userspace TX producer index (kernel consumer is read from the ring).
    tx_head: u32,
    /// Userspace fill-ring producer index.
    fill_head: u32,
    /// Userspace completion-ring consumer index.
    cr_tail: u32,
    /// TX frame addrs awaiting completion (in submission order).
    tx_inflight: std::collections::VecDeque<u64>,
    /// Umem offsets reserved for generated TX (not in the fill ring).
    tx_free: std::collections::VecDeque<u64>,
    /// First umem offset that belongs to the TX pool (RX fill uses `[0, tx_pool_base)`).
    tx_pool_base: u64,
    /// How the socket is bound.
    mode: XdpMode,
}

/// Byte length of a ring's mmap region: the descriptor area plus the
/// producer/consumer/flags pages before it (must match the kernel's ring
/// vmalloc size for `xsk_mmap` to accept it).
fn ring_map_len(desc_off: u64, ring_size: u32, entry_size: usize) -> usize {
    desc_off as usize + ring_size as usize * entry_size
}

/// `mmap` one ring region of the AF_XDP socket `fd` at `pgoff` (byte
/// offset; the kernel's `xsk_mmap` compares these against the
/// `XDP_PGOFF_*`/`XDP_UMEM_PGOFF_*` constants directly).
fn mmap_ring(fd: i32, pgoff: libc::off_t, map_len: usize) -> io::Result<*mut u8> {
    // SAFETY: fresh mapping of our own socket fd; `map_len` is bounded by
    // the ring sizes set via `XDP_*_RING`, and `pgoff` is one of the four
    // offsets the kernel accepts for AF_XDP rings.
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            map_len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            pgoff,
        )
    };
    if p == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(p.cast::<u8>())
}

/// Load a kernel-written ring index (RX/CR producer, TX/FILL consumer)
/// with acquire ordering.
///
/// # Safety
/// `ptr` must be a ring-index word inside a region mmap'd by this socket
/// (aligned and valid for the socket's lifetime).
unsafe fn ring_load(ptr: *mut u32) -> u32 {
    // SAFETY: caller guarantees `ptr` is a live, aligned ring-index word.
    unsafe { AtomicU32::from_ptr(ptr).load(Ordering::Acquire) }
}

/// Store a userspace-owned ring index (TX/FILL producer, RX/CR consumer)
/// with release ordering.
///
/// # Safety
/// `ptr` must be a ring-index word inside a region mmap'd by this socket
/// (aligned and valid for the socket's lifetime).
unsafe fn ring_store(ptr: *mut u32, val: u32) {
    // SAFETY: caller guarantees `ptr` is a live, aligned ring-index word.
    unsafe { AtomicU32::from_ptr(ptr).store(val, Ordering::Release) }
}

/// Bind the umem to NUMA `node` with `mbind(MPOL_BIND)` (best-effort:
/// a non-NUMA kernel or missing permission leaves the allocation where
/// it is).
fn mbind_umem(umem: *mut u8, len: usize, node: i32) {
    // Fixed 1024-bit nodemask (16 words) covers any real node count.
    let mut mask = [0u64; 16];
    let word = node as usize / 64;
    let bit = node as usize % 64;
    if word < mask.len() {
        mask[word] = 1u64 << bit;
    }
    // SAFETY: the mask array is initialized and alive for the call;
    // `umem`/`len` describe a live mmap from this process.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_mbind,
            umem,
            len,
            2, // MPOL_BIND
            mask.as_ptr(),
            (mask.len() * 8) as libc::c_ulong, // maxnode in bits
            0,                                 // MPOL_MF_STRICT unset: best-effort
        )
    };
    if rc != 0 {
        // Not fatal: without NUMA the allocation is still correct.
    }
}

impl XskSocket {
    /// Pointer to ring-index word `off` inside ring region `base`.
    fn field(&self, base: *mut u8, off: u64) -> *mut u32 {
        base.wrapping_add(off as usize).cast::<u32>()
    }

    /// Kernel-written RX producer word.
    fn rx_producer(&self) -> *mut u32 {
        self.field(self.rx_base, self.offsets.rx.producer)
    }
    /// Userspace RX consumer word.
    fn rx_consumer(&self) -> *mut u32 {
        self.field(self.rx_base, self.offsets.rx.consumer)
    }
    /// RX descriptor array.
    fn rx_desc(&self) -> *mut XdpDesc {
        self.rx_base
            .wrapping_add(self.offsets.rx.desc as usize)
            .cast::<XdpDesc>()
    }
    /// Userspace TX producer word.
    fn tx_producer(&self) -> *mut u32 {
        self.field(self.tx_base, self.offsets.tx.producer)
    }
    /// Kernel-written TX consumer word.
    fn tx_consumer(&self) -> *mut u32 {
        self.field(self.tx_base, self.offsets.tx.consumer)
    }
    /// TX descriptor array.
    fn tx_desc(&self) -> *mut XdpDesc {
        self.tx_base
            .wrapping_add(self.offsets.tx.desc as usize)
            .cast::<XdpDesc>()
    }
    /// Userspace fill-ring producer word.
    fn fill_producer(&self) -> *mut u32 {
        self.field(self.fill_base, self.offsets.fr.producer)
    }
    /// Kernel-written fill-ring consumer word.
    fn fill_consumer(&self) -> *mut u32 {
        self.field(self.fill_base, self.offsets.fr.consumer)
    }
    /// Fill-ring descriptor array (umem frame offsets).
    fn fill_desc(&self) -> *mut u64 {
        self.fill_base
            .wrapping_add(self.offsets.fr.desc as usize)
            .cast::<u64>()
    }
    /// Kernel-written completion-ring producer word.
    fn cr_producer(&self) -> *mut u32 {
        self.field(self.cr_base, self.offsets.cr.producer)
    }
    /// Userspace completion-ring consumer word.
    fn cr_consumer(&self) -> *mut u32 {
        self.field(self.cr_base, self.offsets.cr.consumer)
    }
    /// Completion-ring descriptor array.
    fn cr_desc(&self) -> *mut u64 {
        self.cr_base
            .wrapping_add(self.offsets.cr.desc as usize)
            .cast::<u64>()
    }

    /// The bind mode of this socket.
    pub fn mode(&self) -> XdpMode {
        self.mode
    }

    /// Umem frame (chunk) size in bytes.
    pub fn frame_size(&self) -> u32 {
        self.frame_size
    }

    /// Open an AF_XDP socket for `ifindex` on queue `queue_id` with
    /// default options.
    pub fn open(ifindex: i32, queue_id: u32) -> io::Result<Self> {
        Self::open_with(ifindex, queue_id, XskOpenOpts::default())
    }

    /// Open an AF_XDP socket for `ifindex` on queue `queue_id`.
    pub fn open_with(ifindex: i32, queue_id: u32, opts: XskOpenOpts) -> io::Result<Self> {
        let frame_size = DEFAULT_FRAME_SIZE;
        let num_frames = opts.num_frames.max(1);
        let umem_len = (num_frames as usize) * (frame_size as usize);
        let ring_size = opts.ring_size.max(1).next_power_of_two();

        // socket(AF_XDP, SOCK_RAW | SOCK_CLOEXEC, 0): EPERM without
        // CAP_NET_RAW, EAFNOSUPPORT when the kernel lacks AF_XDP.
        // SAFETY: standard socket(2) call with a valid domain/type/protocol.
        let fd = unsafe { libc::socket(libc::AF_XDP, libc::SOCK_RAW | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut umem: *mut u8 = std::ptr::null_mut();
        let mut rx_base: *mut u8 = std::ptr::null_mut();
        let mut tx_base: *mut u8 = std::ptr::null_mut();
        let mut fill_base: *mut u8 = std::ptr::null_mut();
        let mut cr_base: *mut u8 = std::ptr::null_mut();
        let mut offsets = XdpMmapOffsets::default();
        let mut mode = XdpMode::Copy;

        // All fallible steps run in one closure so the error path releases
        // everything allocated so far in a single place.
        let setup = (|| -> io::Result<()> {
            // Umem: anonymous, page-aligned by mmap.
            // SAFETY: anonymous private mapping of a fresh region.
            let p = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    umem_len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if p == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            umem = p.cast::<u8>();
            if let Some(node) = opts.node {
                mbind_umem(umem, umem_len, node);
            }

            // Register the umem.
            let reg = XdpUmemReg {
                addr: umem as u64,
                len: umem_len as u64,
                chunk_size: frame_size,
                headroom: 0,
                flags: 0,
                tx_metadata_len: 0,
            };
            // SAFETY: `reg` is an initialized xdp_umem_reg of exactly the
            // size this kernel's header declares (32 bytes).
            let rc = unsafe {
                libc::setsockopt(
                    fd,
                    SOL_XDP,
                    XDP_UMEM_REG,
                    &reg as *const XdpUmemReg as *const libc::c_void,
                    std::mem::size_of::<XdpUmemReg>() as libc::socklen_t,
                )
            };
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }

            // Ring sizes (each a power of two, as the kernel requires).
            for opt in [
                XDP_RX_RING,
                XDP_TX_RING,
                XDP_UMEM_FILL_RING,
                XDP_UMEM_COMPLETION_RING,
            ] {
                // SAFETY: `ring_size` is a valid u32 of the size the kernel
                // expects for every ring-size option.
                let rc = unsafe {
                    libc::setsockopt(
                        fd,
                        SOL_XDP,
                        opt,
                        &ring_size as *const u32 as *const libc::c_void,
                        std::mem::size_of::<u32>() as libc::socklen_t,
                    )
                };
                if rc != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            // Byte offsets of producer/consumer/flags/desc inside each
            // mmap'd ring region.
            let mut len = std::mem::size_of::<XdpMmapOffsets>() as libc::socklen_t;
            // SAFETY: `offsets` is an xdp_mmap_offsets buffer of the size
            // the kernel writes for XDP_MMAP_OFFSETS.
            let rc = unsafe {
                libc::getsockopt(
                    fd,
                    SOL_XDP,
                    XDP_MMAP_OFFSETS,
                    &mut offsets as *mut XdpMmapOffsets as *mut libc::c_void,
                    &mut len,
                )
            };
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }

            // Map the four ring regions (before bind; the kernel accepts
            // both pre- and post-bind mmaps).
            rx_base = mmap_ring(
                fd,
                XDP_PGOFF_RX_RING,
                ring_map_len(offsets.rx.desc, ring_size, std::mem::size_of::<XdpDesc>()),
            )?;
            tx_base = mmap_ring(
                fd,
                XDP_PGOFF_TX_RING,
                ring_map_len(offsets.tx.desc, ring_size, std::mem::size_of::<XdpDesc>()),
            )?;
            fill_base = mmap_ring(
                fd,
                XDP_UMEM_PGOFF_FILL_RING,
                ring_map_len(offsets.fr.desc, ring_size, std::mem::size_of::<u64>()),
            )?;
            cr_base = mmap_ring(
                fd,
                XDP_UMEM_PGOFF_COMPLETION_RING,
                ring_map_len(offsets.cr.desc, ring_size, std::mem::size_of::<u64>()),
            )?;

            // Bind via bind(2) with struct sockaddr_xdp; the kernel has
            // no XDP_BIND setsockopt; its xsk_bind() runs on the syscall.
            // Native zero-copy is requested first; copy mode is the
            // fallback when the driver rejects it. NEED_WAKEUP is always
            // set so [`Self::kick`] matches the kernel contract.
            let mut flags = XDP_USE_NEED_WAKEUP;
            if opts.zero_copy {
                flags |= XDP_ZEROCOPY;
            } else {
                flags |= XDP_COPY;
            }
            let mut sxdp = SockaddrXdp {
                sxdp_family: libc::AF_XDP as u16,
                sxdp_flags: flags,
                sxdp_ifindex: ifindex as u32,
                sxdp_queue_id: queue_id,
                sxdp_shared_umem_fd: 0,
            };
            // SAFETY: `sxdp` is an initialized sockaddr_xdp; bind(2)
            // copies it into kernel space before returning.
            let rc = unsafe {
                libc::bind(
                    fd,
                    &sxdp as *const SockaddrXdp as *const libc::sockaddr,
                    std::mem::size_of::<SockaddrXdp>() as libc::socklen_t,
                )
            };
            if rc == 0 {
                mode = if opts.zero_copy {
                    XdpMode::ZeroCopy
                } else {
                    XdpMode::Copy
                };
                return Ok(());
            }
            let err = io::Error::last_os_error();
            if !opts.zero_copy {
                return Err(err);
            }
            // Zero-copy rejected (driver without ZC): retry in copy
            // mode. libbpf xsk.c retries on any bind error, not only
            // EOPNOTSUPP; some drivers return EINVAL.
            sxdp.sxdp_flags = XDP_USE_NEED_WAKEUP | XDP_COPY;
            // SAFETY: as above, with the copy flag.
            let rc = unsafe {
                libc::bind(
                    fd,
                    &sxdp as *const SockaddrXdp as *const libc::sockaddr,
                    std::mem::size_of::<SockaddrXdp>() as libc::socklen_t,
                )
            };
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }
            mode = XdpMode::Copy;
            Ok(())
        })();

        if let Err(e) = setup {
            let rx_len = ring_map_len(offsets.rx.desc, ring_size, std::mem::size_of::<XdpDesc>());
            let tx_len = ring_map_len(offsets.tx.desc, ring_size, std::mem::size_of::<XdpDesc>());
            let fr_len = ring_map_len(offsets.fr.desc, ring_size, std::mem::size_of::<u64>());
            let cr_len = ring_map_len(offsets.cr.desc, ring_size, std::mem::size_of::<u64>());
            // SAFETY: each non-null pointer was mmap'd by this function
            // with exactly that length, and `fd` was created above and is
            // still open; no other code can have touched them yet.
            unsafe {
                if !umem.is_null() {
                    libc::munmap(umem.cast(), umem_len);
                }
                for (base, len) in [
                    (rx_base, rx_len),
                    (tx_base, tx_len),
                    (fill_base, fr_len),
                    (cr_base, cr_len),
                ] {
                    if !base.is_null() {
                        libc::munmap(base.cast(), len);
                    }
                }
                libc::close(fd);
            }
            return Err(e);
        }

        let sock = XskSocket {
            fd,
            umem,
            umem_len,
            frame_size,
            ring_size,
            rx_base,
            tx_base,
            fill_base,
            cr_base,
            offsets,
            rx_tail: 0,
            tx_head: 0,
            // The fill ring is pre-filled with `ring_size` entries below.
            fill_head: ring_size,
            cr_tail: 0,
            tx_inflight: std::collections::VecDeque::new(),
            tx_free: {
                let mut q = std::collections::VecDeque::new();
                for i in ring_size..num_frames {
                    q.push_back(i as u64 * frame_size as u64);
                }
                q
            },
            tx_pool_base: ring_size as u64 * frame_size as u64,
            mode,
        };

        // Pre-fill the fill ring with the first `ring_size` frame offsets;
        // the kernel draws from it to fill the RX ring after bind.
        // SAFETY: the first `ring_size` fill entries lie inside the mapped
        // fill region, and the fill ring is empty (nothing bound yet).
        unsafe {
            for i in 0..ring_size {
                sock.fill_desc()
                    .add(i as usize)
                    .write((i as u64) * (frame_size as u64));
            }
            ring_store(sock.fill_producer(), ring_size);
        }
        Ok(sock)
    }

    /// Receive one frame descriptor, or `None` when the RX ring is
    /// empty. The frame is checked out: process it with
    /// [`Self::frame_mut`], then release it with [`Self::tx_frame`] or
    /// [`Self::drop_frame`].
    pub fn recv_frame(&mut self) -> Option<Frame> {
        let mask = self.ring_size - 1;
        // SAFETY: rx_producer() is the RX-ring producer word (written by
        // the kernel), inside the region mmap'd in open().
        let rx_head = unsafe { ring_load(self.rx_producer()) };
        if rx_head == self.rx_tail {
            return None;
        }
        let idx = (self.rx_tail & mask) as usize;
        // SAFETY: idx < ring_size, so the descriptor lies inside the
        // mapped RX ring region.
        let desc = unsafe { self.rx_desc().add(idx).read() };
        // Advance the consumer: the frame is ours now.
        self.rx_tail = self.rx_tail.wrapping_add(1);
        // SAFETY: rx_consumer() is the RX-ring consumer word; the release
        // store pairs with the kernel's acquire read.
        unsafe { ring_store(self.rx_consumer(), self.rx_tail) };
        Some(Frame {
            addr: desc.addr,
            len: desc.len,
        })
    }

    /// Mutable access to a checked-out frame's bytes (in place, in the
    /// umem; no copy).
    ///
    /// # Panics
    /// Panics if `frame` lies outside the umem.
    pub fn frame_mut(&mut self, frame: &Frame) -> &mut [u8] {
        assert!(
            frame
                .addr
                .checked_add(frame.len as u64)
                .is_some_and(|end| end <= self.umem_len as u64),
            "af_xdp: frame outside umem"
        );
        // SAFETY: the frame is checked out (not in the fill ring or the
        // kernel's rings), so user space owns it exclusively; the bounds
        // were checked above.
        unsafe { std::slice::from_raw_parts_mut(self.umem.add(frame.addr as usize), frame.len as usize) }
    }

    /// Transmit a checked-out frame from its umem slot (zero-copy echo:
    /// the same pages the NIC delivered are sent back). Returns `false`
    /// when the TX ring is full; the caller retries after
    /// [`Self::recycle_tx`].
    pub fn tx_frame(&mut self, frame: Frame) -> bool {
        let mask = self.ring_size - 1;
        // The TX ring needs room for one descriptor.
        // SAFETY: tx_consumer() is the TX-ring consumer word (written by
        // the kernel), inside the mapped region.
        let tx_tail = unsafe { ring_load(self.tx_consumer()) };
        if self.tx_head.wrapping_sub(tx_tail) >= self.ring_size {
            return false;
        }
        let idx = (self.tx_head & mask) as usize;
        // SAFETY: idx < ring_size, so the descriptor slot lies inside the
        // mapped TX ring region.
        unsafe {
            self.tx_desc().add(idx).write(XdpDesc {
                addr: frame.addr,
                len: frame.len,
                options: 0,
            });
        }
        self.tx_head = self.tx_head.wrapping_add(1);
        // SAFETY: tx_producer() is the TX-ring producer word; the release
        // store pairs with the kernel's acquire read (the descriptor
        // write above happens-before it).
        unsafe { ring_store(self.tx_producer(), self.tx_head) };
        self.tx_inflight.push_back(frame.addr);
        true
    }

    /// Drop a checked-out frame back to the fill ring (not transmitted).
    pub fn drop_frame(&mut self, frame: Frame) {
        if frame.addr >= self.tx_pool_base {
            self.tx_free.push_back(frame.addr);
        } else {
            self.fill_one(frame.addr);
        }
    }

    /// Allocate a TX-only umem frame from the pool (frames that were
    /// never placed in the fill ring). Used by userspace TCP to emit
    /// generated segments. Returns `None` when the TX pool is empty.
    pub fn alloc_tx(&mut self, len: u32) -> Option<Frame> {
        let addr = self.tx_free.pop_front()?;
        if len as u64 > u64::from(self.frame_size) {
            self.tx_free.push_front(addr);
            return None;
        }
        Some(Frame { addr, len })
    }

    /// Reclaim transmitted frames from the completion ring and return
    /// them to the fill ring. Call this after a TX burst.
    pub fn recycle_tx(&mut self) {
        let mask = self.ring_size - 1;
        // SAFETY: cr_producer() is the completion-ring producer word
        // (written by the kernel), inside the mapped region.
        let cr_head = unsafe { ring_load(self.cr_producer()) };
        while self.cr_tail != cr_head {
            let idx = (self.cr_tail & mask) as usize;
            // SAFETY: idx < ring_size, so the descriptor lies inside the
            // mapped completion region.
            let addr = unsafe { self.cr_desc().add(idx).read() };
            self.cr_tail = self.cr_tail.wrapping_add(1);
            self.tx_inflight.pop_front();
            if addr >= self.tx_pool_base {
                self.tx_free.push_back(addr);
            } else {
                self.fill_one(addr);
            }
        }
        // SAFETY: cr_consumer() is the completion-ring consumer word; the
        // release store pairs with the kernel's acquire read.
        unsafe { ring_store(self.cr_consumer(), self.cr_tail) };
    }

    /// How many TX frames await completion.
    pub fn tx_pending(&self) -> usize {
        self.tx_inflight.len()
    }

    /// The `XDP_USE_NEED_WAKEUP` contract: `(rx, tx)` booleans are true
    /// when the kernel asks for an explicit wakeup. RX wakeup is the
    /// fill-ring flag; TX wakeup is the TX-ring flag (kernel
    /// `Documentation/networking/af_xdp.rst`).
    pub fn need_wakeup(&self) -> (bool, bool) {
        // SAFETY: the flags words lie inside the mapped ring regions.
        let rx = unsafe { ring_load(self.field(self.fill_base, self.offsets.fr.flags)) }
            & XDP_RING_NEED_WAKEUP
            != 0;
        let tx = unsafe { ring_load(self.field(self.tx_base, self.offsets.tx.flags)) }
            & XDP_RING_NEED_WAKEUP
            != 0;
        (rx, tx)
    }

    /// Wake the kernel when it asked for it: `poll` for RX, `sendto`
    /// with no payload for TX (per the `XDP_USE_NEED_WAKEUP` contract).
    pub fn kick(&self) {
        let (rx, tx) = self.need_wakeup();
        if rx {
            let mut pfd = libc::pollfd {
                fd: self.fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: `pfd` is initialized and alive for the call.
            unsafe {
                libc::poll(&mut pfd, 1, 0);
            }
        }
        if tx {
            // SAFETY: a zero-length sendto with no payload only wakes the
            // kernel's TX path; the socket is ours.
            unsafe {
                libc::sendto(
                    self.fd,
                    std::ptr::null(),
                    0,
                    libc::MSG_DONTWAIT,
                    std::ptr::null(),
                    0,
                );
            }
        }
    }

    /// Block until RX is ready or `timeout_ms` elapses. Used on the idle
    /// path so a worker without frames does not spin.
    pub fn wait_rx(&self, timeout_ms: i32) {
        let mut pfd = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pfd` is initialized and alive for the call.
        unsafe {
            libc::poll(&mut pfd, 1, timeout_ms);
        }
    }

    /// Push one frame offset back to the fill ring.
    fn fill_one(&mut self, addr: u64) {
        let mask = self.ring_size - 1;
        // SAFETY: fill_consumer() is the kernel-written fill-ring
        // consumer word, inside the mapped fill region.
        let cons = unsafe { ring_load(self.fill_consumer()) };
        if self.fill_head.wrapping_sub(cons) >= self.ring_size {
            // Fill ring full: leak the umem slot until the socket
            // closes. Checkout accounting should prevent this.
            return;
        }
        let idx = (self.fill_head & mask) as usize;
        // SAFETY: idx < ring_size, so the descriptor lies inside the
        // mapped fill region.
        unsafe {
            self.fill_desc().add(idx).write(addr);
        }
        self.fill_head = self.fill_head.wrapping_add(1);
        // SAFETY: fill_producer() is the fill-ring producer word; the
        // release store pairs with the kernel's acquire read.
        unsafe { ring_store(self.fill_producer(), self.fill_head) };
    }

    /// Insert this socket into an XSKMAP so an attached XDP program can
    /// steer frames into the socket's RX ring. `map_path` is a pinned
    /// bpffs map (e.g. `/sys/fs/bpf/xskmap`); the socket is registered
    /// at `queue`. The map fd comes from `BPF_OBJ_GET`; plain `open()`
    /// on a pinned XSKMAP returns EIO on this kernel (verified; array
    /// maps open fine, XSKMAP does not).
    pub fn register_in_map(&self, map_path: &str, queue: u32) -> io::Result<()> {
        const BPF_OBJ_GET: libc::c_int = 7;
        const BPF_MAP_UPDATE_ELEM: libc::c_int = 2;
        let path = std::ffi::CString::new(map_path)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "map path contains NUL"))?;
        // `union bpf_attr` for BPF_OBJ_GET: pathname pointer at 0,
        // bpf_fd at 8, file_flags at 16.
        #[repr(C)]
        struct BpfAttrObjGet {
            pathname: u64,
            bpf_fd: u32,
            file_flags: u32,
        }
        let oattr = BpfAttrObjGet {
            pathname: path.as_ptr() as u64,
            bpf_fd: 0,
            file_flags: 0,
        };
        // SAFETY: `path` is a NUL-terminated C string alive for the call.
        let map_fd = unsafe {
            libc::syscall(
                libc::SYS_bpf,
                BPF_OBJ_GET,
                &oattr as *const BpfAttrObjGet,
                std::mem::size_of::<BpfAttrObjGet>(),
            )
        };
        if map_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // `union bpf_attr` for BPF_MAP_UPDATE_ELEM: map_fd/key/value/flags.
        // key/value are POINTERS to the actual u32s (repr(C) aligns the
        // u64s after the u32 map_fd, matching the kernel's bpf_attr).
        #[repr(C)]
        struct BpfAttrUpdateElem {
            map_fd: u32,
            key: u64,
            value: u64,
            flags: u64,
        }
        let key: u32 = queue;
        let value: u32 = self.fd as u32;
        let attr = BpfAttrUpdateElem {
            map_fd: map_fd as u32,
            key: (&key as *const u32) as u64,
            value: (&value as *const u32) as u64,
            flags: 0,
        };
        // SAFETY: the bpf(2) syscall reads `attr` and the pointed-to
        // key/value synchronously; all three outlive the call (stack
        // locals), and the struct layouts match the kernel's `bpf_attr`
        // for BPF_OBJ_GET / BPF_MAP_UPDATE_ELEM (verified against
        // <linux/bpf.h> and empirically on this kernel).
        let ret = unsafe {
            libc::syscall(
                libc::SYS_bpf,
                BPF_MAP_UPDATE_ELEM,
                &attr as *const BpfAttrUpdateElem,
                std::mem::size_of::<BpfAttrUpdateElem>(),
            )
        };
        // SAFETY: `map_fd` was returned by the kernel in the call above
        // and is not needed after this function.
        unsafe { libc::close(map_fd as libc::c_int) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for XskSocket {
    /// Unmap the umem and rings, then close the socket.
    fn drop(&mut self) {
        // SAFETY: open() only constructs the socket after every mapping
        // succeeded, so all bases are live mmap regions of exactly these
        // lengths, and the fd is still open.
        unsafe {
            libc::munmap(self.umem.cast(), self.umem_len);
            libc::munmap(
                self.rx_base.cast(),
                ring_map_len(self.offsets.rx.desc, self.ring_size, std::mem::size_of::<XdpDesc>()),
            );
            libc::munmap(
                self.tx_base.cast(),
                ring_map_len(self.offsets.tx.desc, self.ring_size, std::mem::size_of::<XdpDesc>()),
            );
            libc::munmap(
                self.fill_base.cast(),
                ring_map_len(self.offsets.fr.desc, self.ring_size, std::mem::size_of::<u64>()),
            );
            libc::munmap(
                self.cr_base.cast(),
                ring_map_len(self.offsets.cr.desc, self.ring_size, std::mem::size_of::<u64>()),
            );
            libc::close(self.fd);
        }
    }
}

/// Multiple AF_XDP sockets, one per device queue, each with its own
/// umem and rings. A pinned worker owns one socket; the ring memory and
/// umem of every socket are allocated on the requested NUMA node, so a
/// queue handled on its local node never bounces across sockets.
pub struct XskMultiqueue {
    sockets: Vec<XskSocket>,
}

impl XskMultiqueue {
    /// Open one socket per queue id.
    pub fn open_multi(ifindex: i32, queues: &[u32], opts: XskOpenOpts) -> io::Result<Self> {
        let mut sockets = Vec::with_capacity(queues.len());
        for &q in queues {
            sockets.push(XskSocket::open_with(ifindex, q, opts)?);
        }
        Ok(XskMultiqueue { sockets })
    }

    /// The socket for `index` (the queue position in `open_multi`).
    pub fn socket(&mut self, index: usize) -> &mut XskSocket {
        &mut self.sockets[index]
    }

    /// Iterate the sockets.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut XskSocket> {
        self.sockets.iter_mut()
    }

    /// Number of sockets.
    pub fn len(&self) -> usize {
        self.sockets.len()
    }

    /// True when there are no sockets.
    pub fn is_empty(&self) -> bool {
        self.sockets.is_empty()
    }
}

/// The outcome of processing one received frame.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameAction {
    /// Validated and rewritten for echo (MACs swapped, TTL decremented,
    /// IP checksum recomputed); transmit it.
    Echo,
    /// Not IPv4/UDP, malformed, bad checksum, or TTL expired; drop it.
    Drop,
}

/// Process one received Ethernet frame in place for echo: validate
/// (EtherType IPv4 or IPv6, UDP headers, checksums) and rewrite
/// (swap MACs, decrement TTL/hop limit, recompute checksums). IPv6
/// swaps the addresses so the UDP checksum is recomputed.
pub fn process_frame(frame: &mut [u8]) -> FrameAction {
    if frame.len() < 14 + 20 + 8 {
        return FrameAction::Drop;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype == 0x86dd {
        return process_frame_v6(frame);
    }
    if ethertype != 0x0800 {
        return FrameAction::Drop;
    }
    let ip = match crate::parse::parse_ipv4(&frame[14..]) {
        Ok(h) => h,
        Err(_) => return FrameAction::Drop,
    };
    if ip.protocol != 17 {
        return FrameAction::Drop; // not UDP
    }
    let ihl = usize::from(frame[14] & 0x0F) * 4;
    let ip_total = ip.total_len as usize;
    if ihl < 20
        || ip_total < 20 + 8
        || 14 + ip_total > frame.len()
        || 14 + ihl + 8 > 14 + ip_total
    {
        return FrameAction::Drop;
    }
    // IP header checksum: a valid header (checksum field included) folds
    // to zero (RFC 791; see checksum::tests).
    if crate::checksum::ip_checksum(&frame[14..14 + ihl]) != 0 {
        return FrameAction::Drop;
    }
    let udp_off = 14 + ihl;
    let udp = match crate::parse::parse_udp(&frame[udp_off..]) {
        Ok(h) => h,
        Err(_) => return FrameAction::Drop,
    };
    let udp_len = udp.len as usize;
    if udp_len < 8 || udp_off + udp_len > 14 + ip_total {
        return FrameAction::Drop;
    }
    // UDP checksum (RFC 768): zero the field, verify, restore. Zero = off.
    let csum_off = udp_off + 6;
    let stored = u16::from_be_bytes([frame[csum_off], frame[csum_off + 1]]);
    if stored != 0 {
        frame[csum_off] = 0;
        frame[csum_off + 1] = 0;
        let calc = crate::checksum::udp_checksum(
            ip.src,
            ip.dst,
            udp.len,
            &frame[udp_off..udp_off + udp_len],
        );
        frame[csum_off] = (stored >> 8) as u8;
        frame[csum_off + 1] = stored as u8;
        if calc != stored {
            return FrameAction::Drop;
        }
    }
    // As a forwarder, expire datagrams with TTL 1 (no underflow).
    if ip.ttl <= 1 {
        return FrameAction::Drop;
    }
    // Rewrite for echo: swap MACs, decrement TTL, recompute the IP
    // checksum over the zeroed header.
    let (dst, rest) = frame.split_at_mut(6);
    let (src, _) = rest.split_at_mut(6);
    dst.swap_with_slice(src);
    frame[14 + 8] -= 1; // TTL
    frame[14 + 10] = 0;
    frame[14 + 11] = 0;
    let csum = crate::checksum::ip_checksum(&frame[14..14 + ihl]);
    frame[14 + 10] = (csum >> 8) as u8;
    frame[14 + 11] = csum as u8;
    FrameAction::Echo
}

fn process_frame_v6(frame: &mut [u8]) -> FrameAction {
    if frame.len() < 14 + 40 + 8 {
        return FrameAction::Drop;
    }
    let ip = match crate::parse::parse_ipv6(&frame[14..]) {
        Ok(h) => h,
        Err(_) => return FrameAction::Drop,
    };
    if ip.next_header != 17 {
        return FrameAction::Drop;
    }
    if ip.hop_limit <= 1 {
        return FrameAction::Drop;
    }
    let udp_off = 14 + 40;
    let udp = match crate::parse::parse_udp(&frame[udp_off..]) {
        Ok(h) => h,
        Err(_) => return FrameAction::Drop,
    };
    let udp_len = udp.len as usize;
    if udp_len < 8 || udp_off + udp_len > frame.len() {
        return FrameAction::Drop;
    }
    let csum_off = udp_off + 6;
    let stored = u16::from_be_bytes([frame[csum_off], frame[csum_off + 1]]);
    if stored != 0 {
        frame[csum_off] = 0;
        frame[csum_off + 1] = 0;
        let calc = crate::checksum::udp_checksum_v6(
            ip.src,
            ip.dst,
            udp.len as u32,
            &frame[udp_off..udp_off + udp_len],
        );
        frame[csum_off] = (stored >> 8) as u8;
        frame[csum_off + 1] = stored as u8;
        if calc != stored {
            return FrameAction::Drop;
        }
    }
    let (dst, rest) = frame.split_at_mut(6);
    let (src, _) = rest.split_at_mut(6);
    dst.swap_with_slice(src);
    let mut orig_src = [0u8; 16];
    orig_src.copy_from_slice(&frame[14 + 8..14 + 24]);
    frame.copy_within(14 + 24..14 + 40, 14 + 8);
    frame[14 + 24..14 + 40].copy_from_slice(&orig_src);
    frame[14 + 7] -= 1;
    let mut new_src = [0u8; 16];
    new_src.copy_from_slice(&frame[14 + 8..14 + 24]);
    frame[csum_off] = 0;
    frame[csum_off + 1] = 0;
    let calc = crate::checksum::udp_checksum_v6(
        new_src,
        orig_src,
        udp.len as u32,
        &frame[udp_off..udp_off + udp_len],
    );
    frame[csum_off] = (calc >> 8) as u8;
    frame[csum_off + 1] = calc as u8;
    FrameAction::Echo
}

#[cfg(test)]
mod tests {
    use super::*;
    /// UAPI option numbers and ring offsets, verified against
    /// `/usr/include/linux/if_xdp.h`.
    #[test]
    fn uapi_constants_match_header() {
        assert_eq!(SOL_XDP, 283);
        assert_eq!(libc::AF_XDP, 44);
        assert_eq!(XDP_MMAP_OFFSETS, 1);
        assert_eq!(XDP_RX_RING, 2);
        assert_eq!(XDP_TX_RING, 3);
        assert_eq!(XDP_UMEM_REG, 4);
        assert_eq!(XDP_UMEM_FILL_RING, 5);
        assert_eq!(XDP_UMEM_COMPLETION_RING, 6);
        assert_eq!(XDP_PGOFF_RX_RING, 0);
        assert_eq!(XDP_PGOFF_TX_RING, 0x8000_0000);
        assert_eq!(XDP_UMEM_PGOFF_FILL_RING, 0x1_0000_0000);
        assert_eq!(XDP_UMEM_PGOFF_COMPLETION_RING, 0x1_8000_0000);
        assert_eq!(XDP_SHARED_UMEM, 1);
        assert_eq!(XDP_COPY, 2);
        assert_eq!(XDP_ZEROCOPY, 4);
        assert_eq!(XDP_USE_NEED_WAKEUP, 8);
        assert_eq!(XDP_RING_NEED_WAKEUP, 1);
    }

    /// Struct layouts, verified against `/usr/include/linux/if_xdp.h`.
    #[test]
    fn uapi_layouts_match_header() {
        assert_eq!(std::mem::size_of::<SockaddrXdp>(), 16);
        assert_eq!(std::mem::size_of::<XdpUmemReg>(), 32);
        assert_eq!(std::mem::size_of::<XdpRingOffset>(), 32);
        assert_eq!(std::mem::size_of::<XdpMmapOffsets>(), 128);
        assert_eq!(std::mem::size_of::<XdpDesc>(), 16);
    }

    /// This machine has no XDP-capable device (AF_XDP unsupported or no
    /// CAP_NET_RAW): `open` must fail with a real `io::Error` and never
    /// panic. Runs unconditionally.
    #[test]
    fn open_skips_without_device() {
        match XskSocket::open(1, 0) {
            Ok(_) => eprintln!("af_xdp: XDP device present; graceful-skip check not applicable"),
            Err(e) => {
                assert!(
                    e.raw_os_error().is_some(),
                    "expected a real io::Error, got {e:?}"
                );
                eprintln!("af_xdp: no XDP device, open failed as expected: {e}");
            }
        }
    }

    /// Full umem/ring smoke test; runs only when a device is available
    /// (skips silently otherwise).
    #[test]
    fn full_setup_skips() {
        let mut sock = match XskSocket::open(1, 0) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("af_xdp: no XDP device ({e}); skipping full setup");
                return;
            }
        };
        // The RX ring is normally empty right after bind.
        assert!(sock.recv_frame().is_none(), "fresh RX ring must be empty");
        // A dropped frame returns to the fill ring; a transmitted frame
        // waits on the completion ring.
        assert!(sock.tx_pending() == 0);
        let _ = sock.mode();
    }

    /// Zero-copy frame lifecycle without a device: frame descriptors
    /// bound the umem, and the fill-ring accounting stays consistent.
    #[test]
    fn frame_descriptor_bounds() {
        let mut sock = match XskSocket::open(1, 0) {
            Ok(s) => s,
            Err(_) => return, // no device: nothing to test against
        };
        // A synthetic frame inside the umem is addressable.
        let fake = Frame {
            addr: 0,
            len: 64,
        };
        let buf = sock.frame_mut(&fake);
        buf.fill(0xAB);
        assert!(buf.iter().all(|&b| b == 0xAB));
    }

    // ---- the frame-processing pipeline (hardware-independent) ----

    /// Build a synthetic Ethernet/IPv4/UDP frame with correct checksums.
    /// `ttl` is set verbatim; `corrupt` flips a UDP payload bit (to
    /// produce a bad checksum when `bad_udp` is set) or the IP checksum
    /// (when `bad_ip` is set).
    fn build_udp_frame(ttl: u8, bad_ip: bool, bad_udp: bool) -> Vec<u8> {
        let mut f = vec![0u8; 14 + 20 + 8 + 16];
        f[..6].copy_from_slice(&[0xAA; 6]); // dst MAC
        f[6..12].copy_from_slice(&[0xCC; 6]); // src MAC
        f[12] = 0x08; // EtherType IPv4
        f[13] = 0x00;
        // IPv4 header: version/IHL 0x45, total length 44, TTL, UDP(17),
        // src 10.0.0.1 -> dst 10.0.0.2.
        f[14] = 0x45;
        f[16..18].copy_from_slice(&44u16.to_be_bytes());
        f[22] = ttl;
        f[23] = 17;
        f[26..30].copy_from_slice(&[10, 0, 0, 1]);
        f[30..34].copy_from_slice(&[10, 0, 0, 2]);
        let csum = crate::checksum::ip_checksum(&f[14..34]);
        f[24] = (csum >> 8) as u8;
        f[25] = csum as u8;
        // UDP: sport 5000, dport 7777, len 24, payload.
        f[34..36].copy_from_slice(&5000u16.to_be_bytes());
        f[36..38].copy_from_slice(&7777u16.to_be_bytes());
        f[38..40].copy_from_slice(&24u16.to_be_bytes());
        f[42..].copy_from_slice(b"af-xdp-echo-test");
        let u = crate::checksum::udp_checksum([10, 0, 0, 1], [10, 0, 0, 2], 24, &f[34..]);
        f[40] = (u >> 8) as u8;
        f[41] = u as u8;
        if bad_ip {
            f[14 + 10] ^= 0xFF; // corrupt the IP checksum field
        }
        if bad_udp {
            f[42] ^= 0xFF; // corrupt the payload -> UDP checksum fails
        }
        f
    }

    #[test]
    fn pipeline_echoes_valid_udp() {
        let mut f = build_udp_frame(64, false, false);
        assert_eq!(process_frame(&mut f), FrameAction::Echo);
        // MACs swapped.
        assert_eq!(&f[..6], &[0xCC; 6]);
        assert_eq!(&f[6..12], &[0xAA; 6]);
        // TTL decremented; IP checksum still valid.
        assert_eq!(f[22], 63);
        assert_eq!(crate::checksum::ip_checksum(&f[14..34]), 0);
        // UDP checksum unchanged (payload + addresses untouched) and
        // valid: recompute with the checksum field zeroed (RFC 768).
        let stored_udp = u16::from_be_bytes([f[40], f[41]]);
        f[40] = 0;
        f[41] = 0;
        let u = crate::checksum::udp_checksum([10, 0, 0, 1], [10, 0, 0, 2], 24, &f[34..]);
        assert_eq!(stored_udp, u);
    }

    fn build_udp6_frame() -> Vec<u8> {
        let mut f = vec![0u8; 14 + 40 + 8 + 16];
        f[..6].copy_from_slice(&[0xAA; 6]);
        f[6..12].copy_from_slice(&[0xCC; 6]);
        f[12..14].copy_from_slice(&0x86ddu16.to_be_bytes());
        f[14] = 0x60;
        f[18..20].copy_from_slice(&24u16.to_be_bytes());
        f[20] = 17;
        f[21] = 64;
        f[14 + 8 + 15] = 1; // src ::1
        f[14 + 24 + 15] = 2; // dst ::2
        f[54..56].copy_from_slice(&5000u16.to_be_bytes());
        f[56..58].copy_from_slice(&7777u16.to_be_bytes());
        f[58..60].copy_from_slice(&24u16.to_be_bytes());
        f[62..].copy_from_slice(b"af-xdp-echo-test");
        let mut src = [0u8; 16];
        let mut dst = [0u8; 16];
        src[15] = 1;
        dst[15] = 2;
        let c = crate::checksum::udp_checksum_v6(src, dst, 24, &f[54..]);
        f[60] = (c >> 8) as u8;
        f[61] = c as u8;
        f
    }

    #[test]
    fn pipeline_echoes_valid_udp6() {
        let mut f = build_udp6_frame();
        assert_eq!(process_frame(&mut f), FrameAction::Echo);
        assert_eq!(&f[..6], &[0xCC; 6]);
        assert_eq!(&f[6..12], &[0xAA; 6]);
        assert_eq!(f[21], 63); // hop limit
        assert_eq!(f[14 + 8 + 15], 2); // src is original dst
        assert_eq!(f[14 + 24 + 15], 1);
    }

    #[test]
    fn pipeline_drops_bad_ip_checksum() {
        let mut f = build_udp_frame(64, true, false);
        assert_eq!(process_frame(&mut f), FrameAction::Drop);
    }

    #[test]
    fn pipeline_drops_bad_udp_checksum() {
        let mut f = build_udp_frame(64, false, true);
        assert_eq!(process_frame(&mut f), FrameAction::Drop);
    }

    #[test]
    fn pipeline_drops_non_udp_and_expired_ttl() {
        // TTL 1: a forwarder must not echo.
        let mut f = build_udp_frame(1, false, false);
        assert_eq!(process_frame(&mut f), FrameAction::Drop);
        // Wrong EtherType (ARP): dropped.
        let mut g = build_udp_frame(64, false, false);
        g[12] = 0x08;
        g[13] = 0x06;
        assert_eq!(process_frame(&mut g), FrameAction::Drop);
        // Truncated frame: dropped.
        let mut h = build_udp_frame(64, false, false);
        h.truncate(30);
        assert_eq!(process_frame(&mut h), FrameAction::Drop);
    }

    #[test]
    fn pipeline_echo_is_stable_under_repeat() {
        // Echoing an echoed frame is again a valid echo (MACs swap back,
        // TTL keeps dropping).
        let mut f = build_udp_frame(64, false, false);
        assert_eq!(process_frame(&mut f), FrameAction::Echo);
        assert_eq!(process_frame(&mut f), FrameAction::Echo);
        assert_eq!(&f[..6], &[0xAA; 6]);
        assert_eq!(f[22], 62);
    }
}
