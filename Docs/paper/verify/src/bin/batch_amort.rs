//! The syscall-amortization theorem.
//!
//! Cost model: sending one datagram costs `d`; each syscall carries a fixed
//! overhead `h`. Sending `n` datagrams one syscall each costs `n * (h + d)`.
//! Batching them into a single syscall costs `h + n * d`.
//!
//! Verified properties (for h in {100.0, 1000.0}, d in {1.0, 10.0}, n in 1..=1024):
//!   (a) h + n*d <= n*(h+d) for every n: batching is never worse;
//!   (b) (h + n*d)/n is non-increasing in n: amortized cost decreases;
//!   (c) amortized cost at n = 1024 approaches d: the per-datagram floor.
//!
//! All arithmetic is f64, applied consistently. All values here (100, 1000, 1,
//! 10, n up to 1024) are exactly representable in f64, so comparisons are
//! effectively exact; EPS only guards against division rounding.

const MAX_N: usize = 1024;
const EPS: f64 = 1e-9;

/// Cost of sending `n` datagrams through one batched syscall.
fn batch_cost(h: f64, d: f64, n: usize) -> f64 {
    h + (n as f64) * d
}

/// Cost of sending `n` datagrams as `n` individual syscalls.
fn individual_cost(h: f64, d: f64, n: usize) -> f64 {
    (n as f64) * (h + d)
}

fn main() {
    println!("Syscall-amortization theorem");
    println!();
    println!("Cost model: per-datagram cost d, per-syscall overhead h.");
    println!("  individual: n datagrams in n syscalls => n * (h + d)");
    println!("  batched:    n datagrams in 1 syscall  => h + n * d");
    println!("Bound (a): h + n*d <= n*(h+d)  <=>  h <= n*h, which holds for all n >= 1");
    println!("  (equality at n = 1; strict for n > 1).");
    println!("Bound (b): (h + n*d)/n = d + h/n, and h/n decreases as n grows.");
    println!("Bound (c): as n grows, h/n -> 0, so amortized cost tends to d.");
    println!();
    let mut all_ok = true;
    for h in [100.0, 1000.0] {
        for d in [1.0, 10.0] {
            all_ok &= check_pair(h, d);
        }
    }

    println!();
    println!("Overall: {}", if all_ok { "PASS" } else { "FAIL" });
    std::process::exit(if all_ok { 0 } else { 1 });
}

/// Check properties (a), (b), (c) for one (h, d) pair; returns true if all pass.
fn check_pair(h: f64, d: f64) -> bool {
    println!("  h = {}, d = {}", fmt(h), fmt(d));

    // (a) Batching never worse than individual calls, for every n in 1..=1024.
    let mut ok_a = true;
    for n in 1..=MAX_N {
        let b = batch_cost(h, d, n);
        let ind = individual_cost(h, d, n);
        if b > ind + EPS {
            ok_a = false;
            println!("    FAIL (a) at n={}: batch {} > individual {}", n, fmt(b), fmt(ind));
        }
    }
    println!(
        "    (a) batch_cost(n) <= n*(h+d) for all n in 1..={}: {}",
        MAX_N,
        if ok_a { "PASS" } else { "FAIL" }
    );

    // (b) Amortized cost (h + n*d)/n is non-increasing in n.
    let mut ok_b = true;
    let mut prev = batch_cost(h, d, 1);
    for n in 2..=MAX_N {
        let cur = batch_cost(h, d, n) / (n as f64);
        if cur > prev + EPS {
            ok_b = false;
            println!("    FAIL (b) at n={}: amortized {} > previous {}", n, fmt(cur), fmt(prev));
        }
        prev = cur;
    }
    println!(
        "    (b) batch_cost(n)/n non-increasing in n: {}",
        if ok_b { "PASS" } else { "FAIL" }
    );

    // (c) Limit behaviour: amortized cost at n = 1024 vs. the limit d.
    let am = batch_cost(h, d, MAX_N) / (MAX_N as f64);
    println!(
        "    (c) amortized cost at n={}: {} (d = {}, excess h/n = {}, limit as n->inf = {})",
        MAX_N,
        fmt(am),
        fmt(d),
        fmt(am - d),
        fmt(d)
    );

    ok_a && ok_b
}

/// Format a cost value without spurious floating-point noise.
fn fmt(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{:.1}", x)
    } else {
        format!("{:.6}", x)
    }
}
