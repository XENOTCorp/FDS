//! The iteration-bound theorem: k(alpha, eps, d0) iterations suffice for a contraction mapping.
//!
//! For a contraction with Lipschitz constant `alpha` in (0, 1) and initial
//! distance `d0` from the fixed point, the iteration-bound theorem gives the number of iterations
//! needed to get within `eps` of the fixed point:
//!
//!     k(alpha, eps, d0) = ceil( ln(eps * (1 - alpha) / d0) / ln(alpha) )
//!
//! Derivation: the orbit tail after `k` steps is bounded by the geometric sum
//! `d0 * alpha^k / (1 - alpha)`; requiring that tail `<= eps` and solving for
//! `k` yields the formula. `ln(alpha) < 0` for `alpha` in (0, 1), so the ratio
//! is well defined.
//!
//! Checks performed:
//!   (a) the bound is finite and `>= 1` for every sample;
//!   (b) simulation: iterate `x_{n+1} = alpha * x_n` (exact fixed point 0)
//!       from `x_0 = d0` for `k` steps; assert `|x_k| <= eps` within a
//!       relative tolerance of `1e-12`;
//!   (c) monotonicity: for fixed (eps, d0), `k` never decreases as `alpha`
//!       increases;
//!   (d) tightening: for fixed (alpha, d0), `k` is non-decreasing as `eps`
//!       decreases.
//!
//! Std-only: no external crates (must build offline).

/// Iteration-bound formula: `ceil(ln(eps * (1 - alpha) / d0) / ln(alpha))`.
/// Returns `None` when the bound is not a finite value `>= 1`.
fn bound_k(alpha: f64, eps: f64, d0: f64) -> Option<i64> {
    let num = (eps * (1.0 - alpha) / d0).ln();
    let den = alpha.ln();
    let kf = num / den;
    if !kf.is_finite() {
        return None;
    }
    let k = kf.ceil();
    if k >= 1.0 && k <= i64::MAX as f64 {
        Some(k as i64)
    } else {
        None
    }
}

/// Iterate `x_{n+1} = alpha * x_n` from `x_0 = d0` for `k` steps and return
/// the final value.
fn simulate(alpha: f64, d0: f64, k: i64) -> f64 {
    let mut x = d0;
    for _ in 0..k {
        x *= alpha;
    }
    x
}

fn main() {
    let alphas: [f64; 9] = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9];
    let epsilons: [f64; 3] = [1e-3, 1e-6, 1e-9];
    let d0s: [f64; 2] = [1.0, 10.0];

    let mut fail_a = false; // (a) bound finite and >= 1 everywhere
    let mut fail_b = false; // (b) every simulation within tolerance
    let mut fail_c = false; // (c) non-decreasing in alpha
    let mut fail_d = false; // (d) non-decreasing as eps decreases

    println!("Iteration-bound theorem: k(alpha, eps, d0) = ceil(ln(eps*(1-alpha)/d0) / ln(alpha))");
    println!(
        "Samples: alpha in 0.1..=0.9 (step 0.1), eps in {{1e-3, 1e-6, 1e-9}}, d0 in {{1.0, 10.0}}"
    );
    println!();

    // (a) + (b): per-sample bound validity and simulation.
    println!("(a)(b) per sample:");
    let mut n = 0u32;
    for &d0 in &d0s {
        for &eps in &epsilons {
            for &alpha in &alphas {
                n += 1;
                let k = match bound_k(alpha, eps, d0) {
                    Some(k) => k,
                    None => {
                        fail_a = true;
                        println!(
                            "  alpha={:.1} eps={:.0e} d0={:4.1}  k=??   (a) FAIL",
                            alpha, eps, d0
                        );
                        continue;
                    }
                };
                let xk = simulate(alpha, d0, k);
                let sim = xk.abs() <= eps * (1.0 + 1e-12);
                if !sim {
                    fail_b = true;
                }
                println!(
                    "  alpha={:.1} eps={:.0e} d0={:4.1}  k={:3}  |x_k|={:.3e}  (a) PASS  (b) {}",
                    alpha,
                    eps,
                    d0,
                    k,
                    xk.abs(),
                    if sim { "PASS" } else { "FAIL" }
                );
            }
        }
    }
    println!();

    // (c) monotonicity: fixed (eps, d0), alpha ascending -> k non-decreasing.
    for &eps in &epsilons {
        for &d0 in &d0s {
            let mut prev = 0i64;
            for &alpha in &alphas {
                let k = bound_k(alpha, eps, d0).unwrap_or(prev);
                if k < prev {
                    fail_c = true;
                }
                prev = k;
            }
        }
    }

    // (d) tightening: fixed (alpha, d0), eps decreasing (1e-3, 1e-6, 1e-9)
    //     -> k non-decreasing.
    for &alpha in &alphas {
        for &d0 in &d0s {
            let mut prev = 0i64;
            for &eps in &epsilons {
                let k = bound_k(alpha, eps, d0).unwrap_or(prev);
                if k < prev {
                    fail_d = true;
                }
                prev = k;
            }
        }
    }

    // Representative subset: k table for d0 = 1.0.
    println!("Representative k values (d0 = 1.0):");
    print!("{:>11}", "alpha \\ eps");
    for &eps in &epsilons {
        print!(" {:>9}", format!("{:.0e}", eps));
    }
    println!();
    for &alpha in &alphas {
        print!("{:>11}", format!("{:.1}", alpha));
        for &eps in &epsilons {
            let k = bound_k(alpha, eps, 1.0).unwrap_or(-1);
            print!(" {:>9}", k);
        }
        println!();
    }
    println!();

    // Summary.
    let pass_a = !fail_a;
    let pass_b = !fail_b;
    let pass_c = !fail_c;
    let pass_d = !fail_d;
    println!("SUMMARY ({} samples):", n);
    println!(
        "  (a) bound finite and >= 1 for every sample  : {}",
        if pass_a { "PASS" } else { "FAIL" }
    );
    println!(
        "  (b) simulation |x_k| <= eps (rel. tol. 1e-12): {}",
        if pass_b { "PASS" } else { "FAIL" }
    );
    println!(
        "  (c) k never decreases as alpha increases    : {}",
        if pass_c { "PASS" } else { "FAIL" }
    );
    println!(
        "  (d) k non-decreasing as eps decreases      : {}",
        if pass_d { "PASS" } else { "FAIL" }
    );

    if pass_a && pass_b && pass_c && pass_d {
        println!("iteration-bound theorem: ALL CHECKS PASS");
    } else {
        println!("iteration-bound theorem: CHECKS FAILED");
        std::process::exit(1);
    }
}
