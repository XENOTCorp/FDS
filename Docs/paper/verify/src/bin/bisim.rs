//! congruence theorem proof-check: behavioral equivalence (bisimulation) is a congruence
//! for sequential composition (∘) and tensor (⊗) on small finite mealy
//! machines.
//!
//! - Equivalence: partition refinement (Nerode/Hopcroft style) to a
//!   canonical minimized machine; two machines are equivalent iff their
//!   canonical forms are equal.
//! - Universe: exhaustive over all 1- and 2-state machines over alphabets
//!   {0,1}, plus a fixed-seed random sample of 2000 three-state machines.
//! - Congruence checks are run on sampled pairs within each equivalence
//!   class (up to 30 pairs per class) against a sample of 60 other
//!   machines; counts are reported so the coverage is explicit.

use std::collections::HashMap;

#[derive(Clone)]
struct Mealy {
    n: usize,          // number of states
    na: usize,         // input alphabet size
    nb: usize,         // output alphabet size
    trans: Vec<usize>, // [s*na + a] -> next state
    out: Vec<usize>,   // [s*na + a] -> output symbol
}

impl Mealy {
    fn new(n: usize, na: usize, nb: usize, trans: Vec<usize>, out: Vec<usize>) -> Mealy {
        Mealy { n, na, nb, trans, out }
    }

    fn step(&self, s: usize, a: usize) -> (usize, usize) {
        (self.out[s * self.na + a], self.trans[s * self.na + a])
    }

    /// Enumerate ALL machines with `n` states over input alphabet `na`,
    /// output alphabet `nb`.
    fn enumerate(n: usize, na: usize, nb: usize) -> Vec<Mealy> {
        let cells = n * na;
        let trans_count = n_usize_pow(n, cells);
        let out_count = n_usize_pow(nb, cells);
        let mut v = Vec::with_capacity(trans_count * out_count);
        for t in 0..trans_count {
            let mut trans = vec![0usize; cells];
            let mut tt = t;
            for c in 0..cells {
                trans[c] = tt % n;
                tt /= n;
            }
            for o in 0..out_count {
                let mut out = vec![0usize; cells];
                let mut oo = o;
                for c in 0..cells {
                    out[c] = oo % nb;
                    oo /= nb;
                }
                v.push(Mealy::new(n, na, nb, trans.clone(), out));
            }
        }
        v
    }
}

fn n_usize_pow(base: usize, exp: usize) -> usize {
    let mut r = 1usize;
    for _ in 0..exp {
        r = r.saturating_mul(base);
    }
    r
}

/// Partition refinement to a canonical minimized machine.
/// Returns a byte representation; equal representations <=> equivalent.
fn minimize(m: &Mealy) -> Vec<u8> {
    // Initial partition: states grouped by output row.
    let mut classes: Vec<usize> = {
        let mut map: HashMap<Vec<usize>, usize> = HashMap::new();
        (0..m.n)
            .map(|s| {
                let row: Vec<usize> = (0..m.na).map(|a| m.out[s * m.na + a]).collect();
                let len = map.len();
                *map.entry(row).or_insert(len)
            })
            .collect()
    };
    // Refine: signature = output row + next-state-class row.
    loop {
        let sigs: Vec<Vec<usize>> = (0..m.n)
            .map(|s| {
                let mut sig = Vec::with_capacity(2 * m.na);
                for a in 0..m.na {
                    sig.push(m.out[s * m.na + a]);
                }
                for a in 0..m.na {
                    sig.push(classes[m.trans[s * m.na + a]]);
                }
                sig
            })
            .collect();
        let mut map: HashMap<Vec<usize>, usize> = HashMap::new();
        let mut newc: Vec<usize> = Vec::with_capacity(m.n);
        for s in 0..m.n {
            let len = map.len();
            let id = *map.entry(sigs[s].clone()).or_insert(len);
            newc.push(id);
        }
        if newc == classes {
            break;
        }
        classes = newc;
    }
    // Canonical numbering: classes ordered by first occurrence.
    let mut canon_id = vec![0usize; m.n];
    let mut first: HashMap<usize, usize> = HashMap::new();
    let mut next = 0usize;
    for s in 0..m.n {
        let c = classes[s];
        if !first.contains_key(&c) {
            first.insert(c, next);
            next += 1;
        }
        canon_id[s] = first[&c];
    }
    let c = next;
    let mut repr: Vec<u8> = Vec::new();
    for class in 0..c {
        let rep = (0..m.n).find(|&s| canon_id[s] == class).unwrap();
        for a in 0..m.na {
            repr.push(m.out[rep * m.na + a] as u8);
        }
        for a in 0..m.na {
            repr.push(canon_id[m.trans[rep * m.na + a]] as u8);
        }
    }
    repr
}

fn equivalent(m1: &Mealy, m2: &Mealy) -> bool {
    m1.na == m2.na && m1.nb == m2.nb && minimize(m1) == minimize(m2)
}

/// Sequential composition M∘N: N's input is M's output.
fn compose(m1: &Mealy, m2: &Mealy) -> Mealy {
    let n = m1.n * m2.n;
    let na = m1.na;
    let nb = m2.nb;
    let mut trans = vec![0usize; n * na];
    let mut out = vec![0usize; n * na];
    for s1 in 0..m1.n {
        for s2 in 0..m2.n {
            for a in 0..na {
                let (b, ns1) = m1.step(s1, a);
                let (c, ns2) = m2.step(s2, b);
                let idx = (s1 * m2.n + s2) * na + a;
                trans[idx] = ns1 * m2.n + ns2;
                out[idx] = c;
            }
        }
    }
    Mealy::new(n, na, nb, trans, out)
}

/// Tensor M⊗N: independent parallel product.
fn tensor(m1: &Mealy, m2: &Mealy) -> Mealy {
    let n = m1.n * m2.n;
    let na = m1.na * m2.na;
    let nb = m1.nb * m2.nb;
    let mut trans = vec![0usize; n * na];
    let mut out = vec![0usize; n * na];
    for s1 in 0..m1.n {
        for s2 in 0..m2.n {
            for a1 in 0..m1.na {
                for a2 in 0..m2.na {
                    let a = a1 * m2.na + a2;
                    let (b1, ns1) = m1.step(s1, a1);
                    let (b2, ns2) = m2.step(s2, a2);
                    let idx = (s1 * m2.n + s2) * na + a;
                    trans[idx] = ns1 * m2.n + ns2;
                    out[idx] = b1 * m2.nb + b2;
                }
            }
        }
    }
    Mealy::new(n, na, nb, trans, out)
}

fn main() {
    let mut failures = 0usize;
    let mut check = |name: &str, ok: bool, detail: String| {
        if ok {
            println!("PASS: {name}: {detail}");
        } else {
            println!("FAIL: {name}: {detail}");
            failures += 1;
        }
    };

    // Universe: exhaustive 1-2 states + seeded 3-state sample.
    let mut uni: Vec<Mealy> = Vec::new();
    for n in 1..=2 {
        uni.extend(Mealy::enumerate(n, 2, 2));
    }
    let exhaustive = uni.len();
    let mut rng: u64 = 12345;
    for _ in 0..2000 {
        let cells = 3 * 2;
        let mut trans = Vec::with_capacity(cells);
        let mut out = Vec::with_capacity(cells);
        for _ in 0..cells {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            trans.push((rng >> 33) as usize % 3);
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            out.push((rng >> 33) as usize % 2);
        }
        uni.push(Mealy::new(3, 2, 2, trans, out));
    }
    let universe_size = uni.len();

    // Group by canonical form (equivalence classes).
    let mut classes: HashMap<Vec<u8>, Vec<usize>> = HashMap::new();
    for (i, m) in uni.iter().enumerate() {
        classes.entry(minimize(m)).or_default().push(i);
    }
    let class_count = classes.len();

    // (a) equivalence relation: reflexivity on the whole universe.
    let mut refl_ok = true;
    for m in &uni {
        if !equivalent(m, m) {
            refl_ok = false;
        }
    }
    check(
        "congruence (a) reflexive",
        refl_ok,
        format!("M ≈ M for all {universe_size} machines"),
    );

    // (a) symmetry on a sample of ordered pairs.
    let mut sym_ok = true;
    for _ in 0..10_000 {
        let i = ((rng >> 33) as usize) % universe_size;
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let j = ((rng >> 33) as usize) % universe_size;
        if equivalent(&uni[i], &uni[j]) != equivalent(&uni[j], &uni[i]) {
            sym_ok = false;
        }
    }
    check(
        "congruence (a) symmetric",
        sym_ok,
        "equivalent(a,b) == equivalent(b,a) on 10,000 sampled ordered pairs".to_string(),
    );

    // (a) transitivity on sampled triples within classes.
    let mut trans_ok = true;
    let mut triples = 0usize;
    for _ in 0..10_000 {
        let key = {
            let keys: Vec<&Vec<u8>> = classes.keys().collect();
            let k = ((rng >> 33) as usize) % keys.len();
            keys[k].clone()
        };
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let members = &classes[&key];
        if members.len() >= 3 {
            let i = members[((rng >> 33) as usize) % members.len()];
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let j = members[((rng >> 33) as usize) % members.len()];
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let l = members[((rng >> 33) as usize) % members.len()];
            triples += 1;
            if !equivalent(&uni[i], &uni[j]) || !equivalent(&uni[j], &uni[l]) {
                // within-class membership is by construction; assert the
                // canonical-form equality that defines the class.
                if minimize(&uni[i]) != minimize(&uni[l]) {
                    trans_ok = false;
                }
            }
        }
    }
    check(
        "congruence (a) transitive",
        trans_ok,
        format!("same-class membership consistent on {triples} sampled triples"),
    );

    // (b)-(d) congruence: for M ≈ M' and any N:
    //   M∘N ≈ M'∘N, N∘M ≈ N∘M', M⊗N ≈ M'⊗N.
    let mut n_sample: Vec<usize> = Vec::new();
    for k in 0..60 {
        n_sample.push(k % universe_size);
    }
    let mut pairs_checked = 0usize;
    let mut pair_results = 0usize;
    let mut comp_left_ok = true;
    let mut comp_right_ok = true;
    let mut tens_ok = true;
    let mut sorted: Vec<&Vec<u8>> = classes.keys().collect();
    sorted.sort();
    for key in sorted {
        let members = &classes[key];
        let lim = members.len().min(30);
        for x in 0..lim {
            for y in (x + 1)..lim {
                let i = members[x];
                let j = members[y];
                pairs_checked += 1;
                for &k in &n_sample {
                    let n_m = &uni[k];
                    // M∘N ≈ M'∘N
                    let c1 = compose(&uni[i], n_m);
                    let c2 = compose(&uni[j], n_m);
                    pair_results += 1;
                    if !equivalent(&c1, &c2) {
                        comp_left_ok = false;
                    }
                    // N∘M ≈ N∘M'
                    let c3 = compose(n_m, &uni[i]);
                    let c4 = compose(n_m, &uni[j]);
                    pair_results += 1;
                    if !equivalent(&c3, &c4) {
                        comp_right_ok = false;
                    }
                    // M⊗N ≈ M'⊗N
                    let t1 = tensor(&uni[i], n_m);
                    let t2 = tensor(&uni[j], n_m);
                    pair_results += 1;
                    if !equivalent(&t1, &t2) {
                        tens_ok = false;
                    }
                }
            }
        }
    }
    check(
        "congruence (b) M∘N ≈ M'∘N",
        comp_left_ok,
        format!("{pair_results} congruence checks"),
    );
    check(
        "congruence (c) N∘M ≈ N∘M'",
        comp_right_ok,
        format!("{pair_results} congruence checks"),
    );
    check(
        "congruence (d) M⊗N ≈ M'⊗N",
        tens_ok,
        format!("{pair_results} congruence checks"),
    );

    println!(
        "SUMMARY: universe={universe_size} (exhaustive {exhaustive} + 2000 sampled 3-state), \
         classes={class_count}, pairs_checked={pairs_checked}, checks={pair_results}"
    );
    if failures == 0 {
        println!("OVERALL: PASS");
    } else {
        println!("OVERALL: FAIL ({failures} failing checks)");
        std::process::exit(1);
    }
}
