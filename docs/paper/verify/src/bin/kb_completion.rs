//! Theorem NT15 — the ring-buffer equational theory admits a complete rewrite
//! system, verified by Knuth-Bendix completion (std-only Rust, no crates).
//!
//! Single sort; two unary operations `push` and `pop`. Terms are trees over
//! {push, pop} and variables (x, y, ...). The equations, oriented left->right:
//!
//!     push(pop(x)) -> x
//!     pop(push(x)) -> x
//!
//! The tool implements term representation, substitution, matching, one-step
//! rewriting, normalization to normal form, critical-pair computation (a
//! generic overlap finder for unary terms), and a Knuth-Bendix-style
//! completion loop: each critical pair is normalized; if the two sides differ
//! they are oriented by a size-lexicographic well-founded order (smaller size
//! first, ties broken lexicographically) and added as a new rule, then
//! critical pairs are recomputed. The rule count is capped at 100 to
//! guarantee the tool terminates. It then checks (a) termination of the final
//! rule set, (b) local confluence, (c) presence of the original rules, and
//! (d) that the system decides the theory on sample terms.

/// A term over {push, pop} and variables Var(i), displayed as x, y, z, ...
#[derive(Clone, PartialEq, Eq, Debug)]
enum Term {
    Push(Box<Term>),
    Pop(Box<Term>),
    Var(usize),
}

impl Term {
    fn push(t: Term) -> Term {
        Term::Push(Box::new(t))
    }

    fn pop(t: Term) -> Term {
        Term::Pop(Box::new(t))
    }

    fn var(i: usize) -> Term {
        Term::Var(i)
    }

    /// Number of function symbols (variables count as zero).
    fn size(&self) -> usize {
        match self {
            Term::Var(_) => 0,
            Term::Push(t) | Term::Pop(t) => 1 + t.size(),
        }
    }

    /// Collect the indices of all variables occurring in the term.
    fn vars(&self, out: &mut Vec<usize>) {
        match self {
            Term::Var(i) => out.push(*i),
            Term::Push(t) | Term::Pop(t) => t.vars(out),
        }
    }

    /// Rename every variable by adding `offset` (fresh-variable renaming).
    fn rename(&self, offset: usize) -> Term {
        match self {
            Term::Var(i) => Term::var(*i + offset),
            Term::Push(t) => Term::push(t.rename(offset)),
            Term::Pop(t) => Term::pop(t.rename(offset)),
        }
    }
}

impl std::fmt::Display for Term {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Term::Var(i) => {
                if *i < 26 {
                    write!(f, "{}", (b'x' + *i as u8) as char)
                } else {
                    write!(f, "x{}", i)
                }
            }
            Term::Push(t) => write!(f, "push({})", t),
            Term::Pop(t) => write!(f, "pop({})", t),
        }
    }
}

/// Rename variables by order of first occurrence (Var(i) -> Var(k), k in
/// 0..n), for readable output.
fn canonicalize(t: &Term) -> Term {
    fn canon_rec(t: &Term, map: &mut Vec<Option<usize>>, next: &mut usize) -> Term {
        match t {
            Term::Var(i) => {
                if map.len() <= *i {
                    map.resize(*i + 1, None);
                }
                match map[*i] {
                    Some(k) => Term::var(k),
                    None => {
                        let k = *next;
                        *next += 1;
                        map[*i] = Some(k);
                        Term::var(k)
                    }
                }
            }
            Term::Push(x) => Term::push(canon_rec(x, map, next)),
            Term::Pop(x) => Term::pop(canon_rec(x, map, next)),
        }
    }
    let mut map = Vec::new();
    let mut next = 0;
    canon_rec(t, &mut map, &mut next)
}

/// A substitution: subs[i] is the term replacing Var(i).
type Subst = Vec<Term>;

/// Apply a substitution to a term.
fn apply(subs: &[Term], t: &Term) -> Term {
    match t {
        Term::Var(i) => subs.get(*i).cloned().unwrap_or_else(|| Term::var(*i)),
        Term::Push(x) => Term::push(apply(subs, x)),
        Term::Pop(x) => Term::pop(apply(subs, x)),
    }
}

/// Largest variable index occurring in `t` (0 if none).
fn max_var(t: &Term) -> usize {
    let mut vs = Vec::new();
    t.vars(&mut vs);
    vs.iter().copied().max().unwrap_or(0)
}

/// Match pattern `p` against term `t`, recording bindings into `subs`
/// (sized to the pattern's variables, all None initially). A variable
/// already bound must match the recorded term (non-linear patterns).
fn match_pat(p: &Term, t: &Term, subs: &mut [Option<Term>]) -> bool {
    match p {
        Term::Var(i) => match &subs[*i] {
            Some(s) => s == t,
            None => {
                subs[*i] = Some(t.clone());
                true
            }
        },
        Term::Push(x) => match t {
            Term::Push(y) => match_pat(x, y, subs),
            _ => false,
        },
        Term::Pop(x) => match t {
            Term::Pop(y) => match_pat(x, y, subs),
            _ => false,
        },
    }
}

/// Chase variables through the current bindings (Var(i) bound to Var(j) is
/// resolved to the end of the chain).
fn resolve(t: &Term, subs: &[Option<Term>]) -> Term {
    match t {
        Term::Var(i) => match subs.get(*i).and_then(|s| s.as_ref()) {
            Some(s) => resolve(s, subs),
            None => t.clone(),
        },
        Term::Push(x) => Term::push(resolve(x, subs)),
        Term::Pop(x) => Term::pop(resolve(x, subs)),
    }
}

/// Does Var(i) occur in `t`? (occur check for unification)
fn occurs(i: usize, t: &Term) -> bool {
    match t {
        Term::Var(j) => *j == i,
        Term::Push(x) | Term::Pop(x) => occurs(i, x),
    }
}

/// First-order unification of `a` and `b`, extending `subs` (all None
/// initially; resized on demand). On success every variable of `a` is bound.
fn unify(a: &Term, b: &Term, subs: &mut Vec<Option<Term>>) -> bool {
    match (a, b) {
        (Term::Var(i), Term::Var(j)) if i == j => true,
        (Term::Var(i), _) => bind(*i, b, subs),
        (_, Term::Var(j)) => bind(*j, a, subs),
        (Term::Push(x), Term::Push(y)) => unify(x, y, subs),
        (Term::Pop(x), Term::Pop(y)) => unify(x, y, subs),
        _ => false,
    }
}

/// Bind Var(i) to `t`, or unify with its existing binding.
fn bind(i: usize, t: &Term, subs: &mut Vec<Option<Term>>) -> bool {
    let resolved = resolve(t, subs);
    match subs.get(i).and_then(|s| s.clone()) {
        Some(s) => unify(&s, &resolved, subs),
        None => {
            if let Term::Var(j) = &resolved {
                if *j == i {
                    return true;
                }
            }
            if occurs(i, &resolved) {
                return false;
            }
            if subs.len() <= i {
                subs.resize(i + 1, None);
            }
            subs[i] = Some(resolved);
            true
        }
    }
}

/// A rewrite rule lhs -> rhs.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Rule {
    lhs: Term,
    rhs: Term,
}

impl Rule {
    fn new(lhs: Term, rhs: Term) -> Rule {
        Rule { lhs, rhs }
    }
}

impl std::fmt::Display for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} -> {}", self.lhs, self.rhs)
    }
}

/// One step of rewriting: apply a rule at the outermost redex on the spine
/// (root first, then the unique subterm). Returns the rewritten term, or
/// None if no rule applies (the term is in normal form).
fn rewrite_once(t: &Term, rules: &[Rule]) -> Option<Term> {
    for r in rules {
        let mut subs = vec![None; max_var(&r.lhs) + 1];
        if match_pat(&r.lhs, t, &mut subs) {
            let subs: Subst = subs.into_iter().map(|s| s.unwrap()).collect();
            return Some(apply(&subs, &r.rhs));
        }
    }
    match t {
        Term::Push(x) => rewrite_once(x, rules).map(Term::push),
        Term::Pop(x) => rewrite_once(x, rules).map(Term::pop),
        Term::Var(_) => None,
    }
}

/// Normalize a term to normal form by repeated one-step rewriting.
fn normalize(t: &Term, rules: &[Rule]) -> Term {
    let mut cur = t.clone();
    while let Some(next) = rewrite_once(&cur, rules) {
        cur = next;
    }
    cur
}

/// Spine symbols, root to leaf; the bottom variable is included.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SpineSym {
    Push,
    Pop,
    Var(usize),
}

fn spine(t: &Term, out: &mut Vec<SpineSym>) {
    match t {
        Term::Push(x) => {
            out.push(SpineSym::Push);
            spine(x, out);
        }
        Term::Pop(x) => {
            out.push(SpineSym::Pop);
            spine(x, out);
        }
        Term::Var(i) => out.push(SpineSym::Var(*i)),
    }
}

/// Size-lexicographic well-founded term order: smaller size first; on equal
/// size, spine symbol sequences compared lexicographically with
/// push > pop > var. `gt(a, b)` iff `a` is strictly greater than `b`.
fn gt(a: &Term, b: &Term) -> bool {
    let (sa, sb) = (a.size(), b.size());
    if sa != sb {
        return sa > sb;
    }
    let (mut spa, mut spb) = (Vec::new(), Vec::new());
    spine(a, &mut spa);
    spine(b, &mut spb);
    for (x, y) in spa.iter().zip(&spb) {
        match (x, y) {
            (SpineSym::Push, SpineSym::Pop) => return true,
            (SpineSym::Pop, SpineSym::Push) => return false,
            (SpineSym::Var(_), SpineSym::Push) | (SpineSym::Var(_), SpineSym::Pop) => {
                return false
            }
            (SpineSym::Push, SpineSym::Var(_)) | (SpineSym::Pop, SpineSym::Var(_)) => return true,
            _ => {}
        }
    }
    false
}

/// All non-variable subterms along the spine of `t`: the root and each
/// proper subterm (the bottom variable is excluded).
fn spine_subterms(t: &Term) -> Vec<Term> {
    let mut out = Vec::new();
    let mut cur = t.clone();
    while let Term::Push(x) | Term::Pop(x) = &cur {
        out.push(cur.clone());
        cur = (**x).clone();
    }
    out
}

/// A critical pair arising from an overlap of two rule LHSs.
struct CriticalPair {
    /// The overlap term l2σ.
    overlap: Term,
    /// l2σ[r1σ]_p: rewriting with the inner rule at the overlap position.
    by_inner: Term,
    /// r2σ: rewriting with the outer rule at the root.
    by_outer: Term,
}

/// Replace the subterm of `t` that sits where the subterm `sub` sits in
/// `pattern` (t and pattern share the same shape) with `repl`.
fn replace_at(t: &Term, pattern: &Term, sub: &Term, repl: &Term) -> Term {
    if pattern == sub {
        return repl.clone();
    }
    match (t, pattern) {
        (Term::Push(x), Term::Push(p)) => Term::push(replace_at(x, p, sub, repl)),
        (Term::Pop(x), Term::Pop(p)) => Term::pop(replace_at(x, p, sub, repl)),
        _ => t.clone(),
    }
}

/// All critical pairs of the rule set: for every pair of rules i, j (j
/// renamed to disjoint variables) and every non-variable subterm position p
/// of l2 that unifies with l1, the pair (l2σ[r1σ]_p, r2σ).
fn critical_pairs(rules: &[Rule]) -> Vec<CriticalPair> {
    let mut pairs = Vec::new();
    for i in 0..rules.len() {
        for j in 0..rules.len() {
            let l1 = &rules[i].lhs;
            let r1 = &rules[i].rhs;
            let offset = max_var(l1).max(max_var(r1)) + 1;
            let l2 = rules[j].lhs.rename(offset);
            let r2 = rules[j].rhs.rename(offset);
            for sub in spine_subterms(&l2) {
                let mut subs = Vec::new();
                if !unify(l1, &sub, &mut subs) {
                    continue;
                }
                let sigma: Subst = subs
                    .into_iter()
                    .map(|s| s.unwrap_or_else(|| Term::var(0)))
                    .collect();
                let r1s = apply(&sigma, r1);
                let r2s = apply(&sigma, &r2);
                let overlap = apply(&sigma, &l2);
                let by_inner = replace_at(&overlap, &l2, &sub, &r1s);
                pairs.push(CriticalPair {
                    overlap,
                    by_inner,
                    by_outer: r2s,
                });
            }
        }
    }
    pairs
}

fn main() {
    let x = Term::var(0);
    let initial = vec![
        Rule::new(Term::push(Term::pop(x.clone())), x.clone()),
        Rule::new(Term::pop(Term::push(x.clone())), x.clone()),
    ];

    println!("Theorem NT15 - the ring-buffer equational theory admits a complete rewrite");
    println!("system, verified by Knuth-Bendix completion (std-only Rust).");
    println!();
    println!("Signature: single sort; unary operations {{push, pop}}; variables x, y, ...");
    println!("Initial rules (equations oriented left->right):");
    for r in &initial {
        println!("  {}", r);
    }
    println!();

    // --- Knuth-Bendix-style completion loop ---
    let mut rules = initial.clone();
    let mut capped = false;
    let mut pass = 0usize;
    loop {
        pass += 1;
        let cps = critical_pairs(&rules);
        println!(
            "Completion pass {}: {} critical pair(s) considered",
            pass,
            cps.len()
        );
        let mut added = 0usize;
        for cp in &cps {
            let nf_inner = normalize(&cp.by_inner, &rules);
            let nf_outer = normalize(&cp.by_outer, &rules);
            if nf_inner == nf_outer {
                println!(
                    "  overlap {}: joins (normal form {})",
                    canonicalize(&cp.overlap),
                    canonicalize(&nf_inner)
                );
                continue;
            }
            // Orient by the size-lexicographic well-founded order.
            let (l, r) = if gt(&nf_inner, &nf_outer) {
                (nf_inner, nf_outer)
            } else if gt(&nf_outer, &nf_inner) {
                (nf_outer, nf_inner)
            } else {
                println!(
                    "  overlap {}: normal forms incomparable, skipped",
                    canonicalize(&cp.overlap)
                );
                continue;
            };
            let new_rule = Rule::new(l, r);
            if !rules.contains(&new_rule) {
                println!(
                    "  overlap {}: adds rule {}",
                    canonicalize(&cp.overlap),
                    new_rule
                );
                rules.push(new_rule);
                added += 1;
            }
        }
        if added == 0 {
            println!("Completion converged: all critical pairs join, no new rules needed.");
            break;
        }
        if rules.len() >= 100 {
            capped = true;
            println!("Rule cap (100) reached; stopping completion.");
            break;
        }
    }
    println!();

    println!("Final rule set ({} rule(s)):", rules.len());
    for r in &rules {
        println!("  {}", r);
    }
    println!();

    // --- Check (a): the final rule set is terminating ---
    println!("Check (a) - termination: every rule strictly decreases the size-lex order");
    let mut ok_a = true;
    for r in &rules {
        let dec = gt(&r.lhs, &r.rhs);
        ok_a &= dec;
        println!(
            "  {} (size {} -> size {}): {}",
            r,
            r.lhs.size(),
            r.rhs.size(),
            if dec { "decreases" } else { "FAIL: does not decrease" }
        );
    }
    println!("  (a) terminating: {}", if ok_a { "PASS" } else { "FAIL" });
    println!();

    // --- Check (b): local confluence (all critical pairs join) ---
    println!("Check (b) - local confluence: every critical pair joins");
    let cps = critical_pairs(&rules);
    let mut ok_b = true;
    for cp in &cps {
        let nf_inner = normalize(&cp.by_inner, &rules);
        let nf_outer = normalize(&cp.by_outer, &rules);
        let joins = nf_inner == nf_outer;
        ok_b &= joins;
        println!(
            "  overlap {}: nf(inner) = {} == {} = nf(outer): {}",
            canonicalize(&cp.overlap),
            canonicalize(&nf_inner),
            canonicalize(&nf_outer),
            if joins { "join" } else { "FAIL: do not join" }
        );
    }
    println!(
        "  (b) all {} critical pair(s) join: {}",
        cps.len(),
        if ok_b { "PASS" } else { "FAIL" }
    );
    println!();

    // --- Check (c): the original two rules are present ---
    println!("Check (c) - the original two rules are present");
    let mut ok_c = true;
    for r in &initial {
        let present = rules.contains(r);
        ok_c &= present;
        println!(
            "  {} : {}",
            r,
            if present { "present" } else { "FAIL: missing" }
        );
    }
    println!(
        "  (c) original rules present: {}",
        if ok_c { "PASS" } else { "FAIL" }
    );
    println!();

    // --- Check (d): the system decides the theory on samples ---
    println!("Check (d) - the system decides the theory on samples");
    let samples: Vec<(Term, Term)> = vec![
        (
            Term::push(Term::pop(Term::push(x.clone()))),
            Term::push(x.clone()),
        ),
        (
            Term::pop(Term::push(Term::pop(x.clone()))),
            Term::pop(x.clone()),
        ),
        (
            Term::push(Term::push(Term::pop(Term::pop(x.clone())))),
            x.clone(),
        ),
        (
            Term::push(Term::pop(Term::pop(Term::push(x.clone())))),
            Term::pop(Term::push(x.clone())),
        ),
        (
            Term::push(Term::pop(Term::push(Term::pop(x.clone())))),
            x.clone(),
        ),
    ];
    let mut ok_d = true;
    for (s, t) in &samples {
        let nf_s = normalize(s, &rules);
        let nf_t = normalize(t, &rules);
        let eq = nf_s == nf_t;
        ok_d &= eq;
        println!(
            "  normalize({}) == normalize({}): {} == {}: {}",
            canonicalize(s),
            canonicalize(t),
            canonicalize(&nf_s),
            canonicalize(&nf_t),
            if eq { "PASS" } else { "FAIL" }
        );
    }
    let np = normalize(&Term::push(x.clone()), &rules);
    let nq = normalize(&Term::pop(x.clone()), &rules);
    let distinct = np != nq;
    ok_d &= distinct;
    println!(
        "  normalize(push(x)) != normalize(pop(x)): {} != {}: {}",
        canonicalize(&np),
        canonicalize(&nq),
        if distinct { "PASS" } else { "FAIL" }
    );
    println!(
        "  (d) samples decided correctly: {}",
        if ok_d { "PASS" } else { "FAIL" }
    );
    println!();

    let all = ok_a && ok_b && ok_c && ok_d;
    println!("Overall: {}", if all { "PASS" } else { "FAIL" });
    println!();
    println!("Explanation:");
    println!("  The rules push(pop(x)) -> x and pop(push(x)) -> x say that push and pop are");
    println!("  mutual inverses. The only overlaps of the two LHSs are the unary terms");
    println!("  push(pop(push(x))) and pop(push(pop(x))): in both cases the two ways of");
    println!("  rewriting coincide (both sides normalize to push(x) resp. pop(x)), so every");
    println!("  critical pair joins and completion adds no new rule. Each rule strictly");
    println!("  decreases the size-lexicographic order (size 2 -> size 0), so the system is");
    println!("  terminating; by Newman's lemma termination + local confluence = confluence,");
    println!("  i.e. the system is complete. Normal forms are the alternating-free stacks");
    println!("  x, push(x), push(push(x)), ..., pop(x), pop(pop(x)), ..., and two terms are");
    println!("  equal in the theory iff their normal forms coincide, so the system decides");
    println!("  the ring-buffer equational theory.");
    if capped {
        println!("  Note: completion stopped at the 100-rule cap; checks ran on the capped set.");
    }
}
