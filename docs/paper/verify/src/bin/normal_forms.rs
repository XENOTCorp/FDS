//! Theorems NT22–NT24 — normal forms for the free category on a small atom signature.
//!
//! Atom signature: f: A→B, g: B→C, h: C→D.  Composition terms are built from these
//! atoms plus the identities introduced by the rewrite rules (id_A, id_B, id_C,
//! id_D); only well-typed compositions count.
//!
//! Rewrite rules:
//!   (x∘y)∘z → x∘(y∘z)     associativity, oriented toward right-nested chains
//!   id_X∘x  → x           left identity elimination
//!   x∘id_Y  → x           right identity elimination
//!
//! Every well-typed composition term with at most MAX_DEPTH = 6 atom occurrences
//! is enumerated, normalized, and the distinct normal forms are collected.  An
//! explicit reduction loop over the three rewrite rules is additionally run on
//! every term and its result checked against the direct flattening.
//!
//! Concrete evaluation (NT22–24, small atom signature):
//!   A = {0,1}, B = {0,1,2}, C = {0,1,2,3}, D = {0,1,2,3,4}
//!   f: 0↦0, 1↦2      g: 0↦0, 1↦1, 2↦3      h: 0↦0, 1↦2, 2↦1, 3↦4
//!
//! Checks:
//!   (a) finiteness   — number of distinct normal forms is finite (count reported);
//!   (b) completeness — two terms have equal normal forms iff they denote the same
//!       composed function, asserted on every pair of the enumerated set;
//!   (c) examples     — syntactically distinct terms with equal normal forms and
//!       equal behavior.
//!
//! Std-only: no external crates; must build offline.

use std::collections::BTreeMap;

/// Maximum number of atom occurrences in an enumerated term.
const MAX_DEPTH: usize = 6;
const N_TYPES: usize = 4;
const TA: usize = 0;
const TB: usize = 1;
const TC: usize = 2;
const TD: usize = 3;

/// Carrier-set sizes for concrete evaluation.
const SIZE: [u8; 4] = [2, 3, 4, 5];

/// All leaf atoms: signature atoms f, g, h plus the identities id_A..id_D.
const ALL_ATOMS: [Atom; 7] = [
    Atom::F,
    Atom::G,
    Atom::H,
    Atom::IdA,
    Atom::IdB,
    Atom::IdC,
    Atom::IdD,
];

/// Signature atoms plus the identities used by the identity rules.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Atom {
    F,
    G,
    H,
    IdA,
    IdB,
    IdC,
    IdD,
}

impl Atom {
    fn src(self) -> usize {
        match self {
            Atom::F => TA,
            Atom::G => TB,
            Atom::H => TC,
            Atom::IdA => TA,
            Atom::IdB => TB,
            Atom::IdC => TC,
            Atom::IdD => TD,
        }
    }
    fn tgt(self) -> usize {
        match self {
            Atom::F => TB,
            Atom::G => TC,
            Atom::H => TD,
            Atom::IdA => TA,
            Atom::IdB => TB,
            Atom::IdC => TC,
            Atom::IdD => TD,
        }
    }
    fn is_identity(self) -> bool {
        matches!(
            self,
            Atom::IdA | Atom::IdB | Atom::IdC | Atom::IdD
        )
    }
    fn name(self) -> &'static str {
        match self {
            Atom::F => "f",
            Atom::G => "g",
            Atom::H => "h",
            Atom::IdA => "id_A",
            Atom::IdB => "id_B",
            Atom::IdC => "id_C",
            Atom::IdD => "id_D",
        }
    }
}

/// Composition term.  `Comp(l, r)` denotes the categorical composite r∘l:
/// apply `l` first, then `r`.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Expr {
    Leaf(Atom),
    Comp(Box<Expr>, Box<Expr>),
}

fn type_name(t: usize) -> &'static str {
    match t {
        TA => "A",
        TB => "B",
        TC => "C",
        TD => "D",
        _ => "?",
    }
}

/// Source and target of a term.  Valid because only well-typed terms are built
/// (independently verified by `check_typed`).
fn expr_type(e: &Expr) -> (usize, usize) {
    match e {
        Expr::Leaf(a) => (a.src(), a.tgt()),
        Expr::Comp(l, r) => (expr_type(l).0, expr_type(r).1),
    }
}

/// Verify well-typedness; returns (src, tgt, leaf count) on success.
fn check_typed(e: &Expr) -> Result<(usize, usize, usize), String> {
    match e {
        Expr::Leaf(a) => Ok((a.src(), a.tgt(), 1)),
        Expr::Comp(l, r) => {
            let (ls, lt, ln) = check_typed(l)?;
            let (rs, rt, rn) = check_typed(r)?;
            if lt != rs {
                return Err(format!(
                    "ill-typed composite: target of left = {}, source of right = {}",
                    type_name(lt),
                    type_name(rs)
                ));
            }
            Ok((ls, rt, ln + rn))
        }
    }
}

/// Display a term with minimal parentheses, in categorical notation (r∘l).
fn display_expr(e: &Expr) -> String {
    fn go(e: &Expr, par: bool) -> String {
        match e {
            Expr::Leaf(a) => a.name().to_string(),
            Expr::Comp(l, r) => {
                let s = format!("{}∘{}", go(r, true), go(l, true));
                if par {
                    format!("({})", s)
                } else {
                    s
                }
            }
        }
    }
    go(e, false)
}

/// Atoms of a term in application order (first-applied first), identities dropped.
fn flatten(e: &Expr) -> Vec<Atom> {
    fn go(e: &Expr, out: &mut Vec<Atom>) {
        match e {
            Expr::Leaf(a) => {
                if !a.is_identity() {
                    out.push(*a);
                }
            }
            Expr::Comp(l, r) => {
                go(l, out);
                go(r, out);
            }
        }
    }
    let mut out = Vec::new();
    go(e, &mut out);
    out
}

/// Direct normalization: the flat chain of non-identity atoms in application
/// order.  Any bracketing of the same leaf sequence reduces to the same chain,
/// which is exactly what the rewrite system produces (right-nesting via
/// (x∘y)∘z → x∘(y∘z), then id∘x → x and x∘id → x); verified against the
/// explicit reduction loop for every enumerated term.
fn normalize(e: &Expr) -> (usize, usize, Vec<Atom>) {
    let (s, t) = expr_type(e);
    (s, t, flatten(e))
}

/// Normal-form key: (source type, target type, atoms in application order).
type NfKey = (usize, usize, Vec<Atom>);

/// Display a normal-form key as a morphism, e.g. "h∘g∘f : A→D", "id_B : B→B".
fn display_nf(key: &NfKey) -> String {
    let (s, t, atoms) = key;
    if atoms.is_empty() {
        format!(
            "id_{} : {}→{}",
            type_name(*s),
            type_name(*s),
            type_name(*t)
        )
    } else {
        let names: Vec<&str> = atoms.iter().rev().map(|a| a.name()).collect();
        format!(
            "{} : {}→{}",
            names.join("∘"),
            type_name(*s),
            type_name(*t)
        )
    }
}

/// Concrete evaluation of a single atom on a carrier element.
fn apply_atom(a: Atom, x: u8) -> u8 {
    match a {
        Atom::F => match x {
            0 => 0,
            _ => 2,
        },
        Atom::G => match x {
            0 => 0,
            1 => 1,
            _ => 3,
        },
        Atom::H => match x {
            0 => 0,
            1 => 2,
            2 => 1,
            _ => 4,
        },
        Atom::IdA | Atom::IdB | Atom::IdC | Atom::IdD => x,
    }
}

/// Evaluate a well-typed term on an input of its source type.
fn eval(e: &Expr, x: u8) -> u8 {
    match e {
        Expr::Leaf(a) => apply_atom(*a, x),
        Expr::Comp(l, r) => eval(r, eval(l, x)),
    }
}

/// Semantic key: (source, target, outputs packed 3 bits each).  Distinct types
/// yield distinct keys; equal keys mean equal output on every input.
fn sem_key(e: &Expr) -> u64 {
    let (s, t) = expr_type(e);
    let mut packed: u32 = 0;
    for (i, x) in (0..SIZE[s]).enumerate() {
        packed |= (eval(e, x) as u32) << (3 * (i as u32));
    }
    ((s as u64) << 40) | ((t as u64) << 32) | (packed as u64)
}

/// One rewrite step (leftmost-outermost redex).  None if the term is irreducible.
fn rewrite_step(e: &Expr) -> Option<Expr> {
    match e {
        Expr::Leaf(_) => None,
        Expr::Comp(l, r) => {
            // (x∘y)∘z → x∘(y∘z)
            if let Expr::Comp(x, y) = l.as_ref() {
                return Some(Expr::Comp(
                    x.clone(),
                    Box::new(Expr::Comp(y.clone(), r.clone())),
                ));
            }
            // id_X∘x → x
            if let Expr::Leaf(a) = l.as_ref() {
                if a.is_identity() {
                    return Some((**r).clone());
                }
            }
            // x∘id_Y → x
            if let Expr::Leaf(a) = r.as_ref() {
                if a.is_identity() {
                    return Some((**l).clone());
                }
            }
            // Recurse: left child first, then right child.
            if let Some(l2) = rewrite_step(l) {
                return Some(Expr::Comp(Box::new(l2), r.clone()));
            }
            if let Some(r2) = rewrite_step(r) {
                return Some(Expr::Comp(l.clone(), Box::new(r2)));
            }
            None
        }
    }
}

/// Reduce by the rewrite rules until no redex remains.  Terminating: every
/// associativity step lowers the number of left-nested composites, every
/// identity step removes a leaf.
fn rewrite_to_normal(e: &Expr) -> Expr {
    let mut cur = e.clone();
    let mut steps: usize = 0;
    while let Some(next) = rewrite_step(&cur) {
        cur = next;
        steps += 1;
        assert!(steps <= 100_000, "rewrite loop did not terminate");
    }
    cur
}

/// Enumerate all well-typed composition terms with exactly `depth` atom leaves
/// (identities included).  exprs[d][s][t] holds the terms of type s→t with d
/// leaves.  Every term has a unique root split (left subtree depth, middle
/// type), so no term is generated twice.
fn enumerate() -> Vec<Vec<Vec<Vec<Expr>>>> {
    let mut exprs: Vec<Vec<Vec<Vec<Expr>>>> =
        vec![vec![vec![Vec::new(); N_TYPES]; N_TYPES]; MAX_DEPTH + 1];
    for a in ALL_ATOMS {
        exprs[1][a.src()][a.tgt()].push(Expr::Leaf(a));
    }
    for d in 2..=MAX_DEPTH {
        for d1 in 1..d {
            let d2 = d - d1;
            for s in 0..N_TYPES {
                for mid in 0..N_TYPES {
                    for t in 0..N_TYPES {
                        // Clone the source buckets so the mutable push below does
                        // not alias the immutable borrows (d1, d2 < d, so the
                        // buckets are disjoint in principle; the compiler cannot
                        // see that through runtime indices).
                        let lefts: Vec<Expr> = exprs[d1][s][mid].clone();
                        let rights: Vec<Expr> = exprs[d2][mid][t].clone();
                        for l in &lefts {
                            for r in &rights {
                                exprs[d][s][t].push(Expr::Comp(
                                    Box::new(l.clone()),
                                    Box::new(r.clone()),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    exprs
}

/// Same, but with signature atoms {f, g, h} only (strict reading of the spec;
/// the identity rules are then vacuous).
fn enumerate_sig_only() -> Vec<Vec<Vec<Vec<Expr>>>> {
    let mut exprs: Vec<Vec<Vec<Vec<Expr>>>> =
        vec![vec![vec![Vec::new(); N_TYPES]; N_TYPES]; MAX_DEPTH + 1];
    for a in [Atom::F, Atom::G, Atom::H] {
        exprs[1][a.src()][a.tgt()].push(Expr::Leaf(a));
    }
    for d in 2..=MAX_DEPTH {
        for d1 in 1..d {
            let d2 = d - d1;
            for s in 0..N_TYPES {
                for mid in 0..N_TYPES {
                    for t in 0..N_TYPES {
                        let lefts: Vec<Expr> = exprs[d1][s][mid].clone();
                        let rights: Vec<Expr> = exprs[d2][mid][t].clone();
                        for l in &lefts {
                            for r in &rights {
                                exprs[d][s][t].push(Expr::Comp(
                                    Box::new(l.clone()),
                                    Box::new(r.clone()),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    exprs
}

/// First two distinct terms (indices into `all`) with the given normal form.
fn find_pair(nf_keys: &[NfKey], target: &NfKey) -> Option<(usize, usize)> {
    let mut found = [None, None];
    for (i, k) in nf_keys.iter().enumerate() {
        if k == target {
            if found[0].is_none() {
                found[0] = Some(i);
            } else {
                found[1] = Some(i);
                break;
            }
        }
    }
    match (found[0], found[1]) {
        (Some(i), Some(j)) => Some((i, j)),
        _ => None,
    }
}

fn main() {
    println!("Theorem NT22–NT24 — normal forms for the free category on {{f,g,h}}");
    println!("==================================================================");
    println!();
    println!("Atom signature: f: A→B,  g: B→C,  h: C→D");
    println!("Rewrite rules:");
    println!("  (x∘y)∘z → x∘(y∘z)     associativity, oriented to right-nested normal forms");
    println!("  id_X∘x  → x           left identity elimination");
    println!("  x∘id_Y  → x           right identity elimination");
    println!("Normal form: right-nested chain of non-identity atoms, displayed flat");
    println!("as a morphism, e.g. h∘g∘f means apply f, then g, then h.");
    println!();
    println!("Concrete evaluation:");
    println!("  A = {{0,1}}, B = {{0,1,2}}, C = {{0,1,2,3}}, D = {{0,1,2,3,4}}");
    println!("  f: 0↦0, 1↦2");
    println!("  g: 0↦0, 1↦1, 2↦3");
    println!("  h: 0↦0, 1↦2, 2↦1, 3↦4");
    println!();

    let exprs = enumerate();

    // Collect all terms in a deterministic order (depth, then source, then target).
    let mut all: Vec<(usize, Expr)> = Vec::new();
    let mut per_depth = [0usize; MAX_DEPTH + 1];
    for d in 1..=MAX_DEPTH {
        for s in 0..N_TYPES {
            for t in 0..N_TYPES {
                per_depth[d] += exprs[d][s][t].len();
                for e in &exprs[d][s][t] {
                    all.push((d, e.clone()));
                }
            }
        }
    }
    let total = all.len();

    println!("Enumeration: all well-typed composition terms with 1..={MAX_DEPTH} atom");
    println!("occurrences (leaves from {{f, g, h, id_A, id_B, id_C, id_D}}; the");
    println!("identities are introduced by the rewrite rules):");
    for d in 1..=MAX_DEPTH {
        println!("  depth {d:>2}: {:>5} terms", per_depth[d]);
    }
    println!("  total: {total} terms");
    println!();

    // Supplementary: strict reading — leaves from {f,g,h} only.
    let sig_exprs = enumerate_sig_only();
    let mut sig_total = 0usize;
    let mut sig_nfs: BTreeMap<NfKey, usize> = BTreeMap::new();
    for d in 1..=MAX_DEPTH {
        for s in 0..N_TYPES {
            for t in 0..N_TYPES {
                sig_total += sig_exprs[d][s][t].len();
                for e in &sig_exprs[d][s][t] {
                    *sig_nfs.entry(normalize(e)).or_insert(0) += 1;
                }
            }
        }
    }
    println!("Supplementary — strict reading, leaves from {{f,g,h}} only:");
    println!("  {sig_total} terms; distinct normal forms: {} (all of depth ≤ 3)", sig_nfs.len());
    println!("  (without identities no term of depth 4..=6 is well typed, so the depth");
    println!("   bound is only meaningful once identities participate)");
    println!();

    // Normalize and evaluate every term; verify well-typedness, leaf counts,
    // unique generation, and agreement of the explicit rewrite loop with the
    // direct flattening.
    let nf_keys: Vec<NfKey> = all.iter().map(|(_, e)| normalize(e)).collect();
    let sem_keys: Vec<u64> = all.iter().map(|(_, e)| sem_key(e)).collect();

    let mut ok_setup = true;
    let mut uniq: BTreeMap<String, usize> = BTreeMap::new();
    for (i, (depth, e)) in all.iter().enumerate() {
        match check_typed(e) {
            Err(msg) => {
                println!("  ERROR term #{i}: {msg}");
                ok_setup = false;
            }
            Ok((s, t, leaves)) => {
                if leaves != *depth {
                    println!("  ERROR term #{i}: {leaves} leaves != declared depth {depth}");
                    ok_setup = false;
                }
                let atoms = &nf_keys[i].2;
                let chain_ok = if atoms.is_empty() {
                    s == t
                } else {
                    atoms[0].src() == s
                        && atoms.last().map(|a| a.tgt()) == Some(t)
                        && atoms.windows(2).all(|w| w[0].tgt() == w[1].src())
                };
                if !chain_ok {
                    println!("  ERROR term #{i}: normal-form chain does not type to the term type");
                    ok_setup = false;
                }
            }
        }
        if flatten(&rewrite_to_normal(e)) != nf_keys[i].2 {
            println!("  ERROR term #{i}: rewrite loop and direct normalization disagree");
            ok_setup = false;
        }
        *uniq.entry(display_expr(e)).or_insert(0) += 1;
    }
    let dup = uniq.len() != total;
    if dup {
        println!("  ERROR: enumeration generated duplicate terms");
        ok_setup = false;
    }
    println!(
        "Every term well typed, leaf count matches depth, rewrite-system normal form",
    );
    println!(
        "matches direct normalization, and every term generated exactly once: {}",
        if ok_setup { "PASS" } else { "FAIL" }
    );
    println!();

    // (a) Finiteness of normal forms.
    let mut groups: BTreeMap<NfKey, usize> = BTreeMap::new();
    for k in &nf_keys {
        *groups.entry(k.clone()).or_insert(0) += 1;
    }
    let nf_count = groups.len();
    println!("(a) Finiteness of normal forms");
    println!("    {total} terms reduce to {nf_count} distinct normal forms (finite;");
    println!("    every term terminates under the rewrite rules):");
    let mut seen_types: BTreeMap<(usize, usize), NfKey> = BTreeMap::new();
    let mut type_conflict = false;
    for (k, cnt) in &groups {
        println!("      {:<24} {:>4} term(s)", display_nf(k), cnt);
        let ty = (k.0, k.1);
        if let Some(other) = seen_types.get(&ty) {
            type_conflict = true;
            println!(
                "      note: distinct normal forms {} and {} share the type {}→{}",
                display_nf(other),
                display_nf(k),
                type_name(ty.0),
                type_name(ty.1)
            );
        } else {
            seen_types.insert(ty, k.clone());
        }
    }
    if !type_conflict {
        println!("    all {nf_count} normal forms have pairwise distinct types; distinct");
        println!("    normal forms can therefore never agree extensionally (as arrows).");
    }
    let a_ok = nf_count > 0 && nf_count < total;
    println!(
        "    (a) finiteness: {}",
        if a_ok { "PASS" } else { "FAIL" }
    );
    println!();

    // (b) Completeness: equal normal forms IFF equal composed function, on all pairs.
    let mut ok_b = true;
    let mut ok_count: usize = 0;
    let mut shown: usize = 0;
    for i in 0..total {
        for j in (i + 1)..total {
            let nf_eq = nf_keys[i] == nf_keys[j];
            let sem_eq = sem_keys[i] == sem_keys[j];
            if nf_eq == sem_eq {
                ok_count += 1;
            } else {
                ok_b = false;
                if shown < 8 {
                    shown += 1;
                    println!(
                        "    mismatch #{shown}: term #{i} ({}) nf={} sem={:016x}  vs  \
                         term #{j} ({}) nf={} sem={:016x}",
                        display_expr(&all[i].1),
                        display_nf(&nf_keys[i]),
                        sem_keys[i],
                        display_expr(&all[j].1),
                        display_nf(&nf_keys[j]),
                        sem_keys[j],
                    );
                }
            }
        }
    }
    let pairs = total * (total - 1) / 2;
    println!("(b) Completeness: equal normal forms <-> equal composed function");
    println!("    terms: {total}; unordered pairs asserted: {pairs}");
    println!(
        "    iff holds on {ok_count}/{pairs} pairs: {}",
        if ok_b { "PASS" } else { "FAIL" }
    );
    println!();

    // (c) Examples: syntactically distinct terms, equal normal forms, equal behavior.
    let targets: [NfKey; 3] = [
        (TA, TB, vec![Atom::F]),
        (TA, TC, vec![Atom::F, Atom::G]),
        (TA, TD, vec![Atom::F, Atom::G, Atom::H]),
    ];
    let mut ok_c = true;
    println!("(c) Examples: syntactically distinct terms with equal normal forms and");
    println!("    equal behavior");
    for (n, target) in targets.iter().enumerate() {
        match find_pair(&nf_keys, target) {
            Some((i, j)) => {
                let e1 = &all[i].1;
                let e2 = &all[j].1;
                let (s, _) = expr_type(e1);
                let mut table = String::new();
                for x in 0..SIZE[s] {
                    if x > 0 {
                        table.push_str(", ");
                    }
                    table.push_str(&format!("{}↦{}", x, eval(e1, x)));
                }
                let syn_diff = display_expr(e1) != display_expr(e2);
                let same_sem = sem_keys[i] == sem_keys[j];
                let ok = syn_diff && same_sem;
                if !ok {
                    ok_c = false;
                }
                println!(
                    "  Example {}: normal form {} — {}",
                    n + 1,
                    display_nf(target),
                    if ok { "PASS" } else { "FAIL" }
                );
                println!("    term A (depth {}): {}", all[i].0, display_expr(e1));
                println!("    term B (depth {}): {}", all[j].0, display_expr(e2));
                println!("    equal behavior on all inputs of {}: {}", type_name(s), table);
            }
            None => {
                ok_c = false;
                println!(
                    "  Example {}: no pair found for normal form {} — FAIL",
                    n + 1,
                    display_nf(target)
                );
            }
        }
    }
    println!();

    let overall = ok_setup && a_ok && ok_b && ok_c;
    println!(
        "Overall: {}",
        if overall { "PASS" } else { "FAIL" }
    );
    std::process::exit(if overall { 0 } else { 1 });
}
