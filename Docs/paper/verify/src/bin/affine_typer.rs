//! The no-allocation theorem for the linear combinator calculus Λ.
//!
//! Sequential composition: t ; u means run t then u (u ∘ t in Mol).
//! Tensor of two morphisms is the bifunctor:
//!   t : A ⊸ B, u : C ⊸ D  ⇒  t ⊗ u : (A⊗C) ⊸ (B⊗D).
//! No weakening, no contraction: every variable is consumed exactly once.
//! Reduction is pure rewiring: leaf multiset preserved, node count never
//! increases. That bound is on the syntax tree, not the dataplane heap.

use std::collections::HashSet;

#[derive(Clone, PartialEq, Eq, Debug)]
enum Ty {
    Base(u32),
    Pair(Box<Ty>, Box<Ty>),
    Lin(Box<Ty>, Box<Ty>),
    Unit,
}

impl Ty {
    fn lin(from: Ty, to: Ty) -> Ty {
        Ty::Lin(Box::new(from), Box::new(to))
    }
    fn pair(a: Ty, b: Ty) -> Ty {
        Ty::Pair(Box::new(a), Box::new(b))
    }
}

#[derive(Clone, Debug)]
enum Term {
    Var(String),
    Atom(String, Ty, Ty),
    Comp(Box<Term>, Box<Term>),
    Tensor(Box<Term>, Box<Term>),
    Unit,
}

type Ctx = Vec<(String, Ty)>;

fn free_vars(t: &Term, out: &mut Vec<String>) {
    match t {
        Term::Var(x) => out.push(x.clone()),
        Term::Atom(_, _, _) | Term::Unit => {}
        Term::Comp(a, b) | Term::Tensor(a, b) => {
            free_vars(a, out);
            free_vars(b, out);
        }
    }
}

fn splits(ctx: &Ctx) -> Vec<(Ctx, Ctx)> {
    let mut result = Vec::new();
    let n = ctx.len();
    for mask in 0..(1usize << n) {
        let mut l = Vec::new();
        let mut r = Vec::new();
        for (i, (name, ty)) in ctx.iter().enumerate() {
            if mask & (1 << i) != 0 {
                l.push((name.clone(), ty.clone()));
            } else {
                r.push((name.clone(), ty.clone()));
            }
        }
        result.push((l, r));
    }
    result
}

fn infer(ctx: &Ctx, t: &Term) -> Option<Ty> {
    let mut names = HashSet::new();
    for (n, _) in ctx {
        if !names.insert(n.clone()) {
            return None;
        }
    }
    match t {
        Term::Unit => {
            if ctx.is_empty() {
                Some(Ty::lin(Ty::Unit, Ty::Unit))
            } else {
                None
            }
        }
        Term::Atom(_, a, b) => {
            if ctx.is_empty() {
                Some(Ty::lin(a.clone(), b.clone()))
            } else {
                None
            }
        }
        Term::Var(x) => {
            if ctx.len() == 1 && ctx[0].0 == *x {
                Some(ctx[0].1.clone())
            } else {
                None
            }
        }
        Term::Comp(f, g) => {
            // sequential: f : A ⊸ B, g : B ⊸ C ⇒ f;g : A ⊸ C
            for (l, r) in splits(ctx) {
                if let (Some(Ty::Lin(a, b)), Some(Ty::Lin(b2, c))) =
                    (infer(&l, f), infer(&r, g))
                {
                    if b == b2 {
                        return Some(Ty::Lin(a, c));
                    }
                }
            }
            None
        }
        Term::Tensor(f, g) => {
            // bifunctor: f : A ⊸ B, g : C ⊸ D ⇒ f⊗g : (A⊗C) ⊸ (B⊗D)
            for (l, r) in splits(ctx) {
                if let (Some(tf), Some(tg)) = (infer(&l, f), infer(&r, g)) {
                    if let (Ty::Lin(a, b), Ty::Lin(c, d)) = (tf, tg) {
                        return Some(Ty::lin(Ty::pair(*a, *c), Ty::pair(*b, *d)));
                    }
                }
            }
            None
        }
    }
}

fn step(t: &Term) -> Option<Term> {
    match t {
        Term::Comp(a, b) => {
            if let Term::Comp(f, g) = a.as_ref() {
                return Some(Term::Comp(
                    f.clone(),
                    Box::new(Term::Comp(g.clone(), b.clone())),
                ));
            }
            if let Term::Tensor(f, g) = a.as_ref() {
                if let Term::Tensor(x, y) = b.as_ref() {
                    return Some(Term::Tensor(
                        Box::new(Term::Comp(f.clone(), x.clone())),
                        Box::new(Term::Comp(g.clone(), y.clone())),
                    ));
                }
            }
            match (step(a), step(b)) {
                (Some(a1), _) => Some(Term::Comp(Box::new(a1), b.clone())),
                (None, Some(b1)) => Some(Term::Comp(a.clone(), Box::new(b1))),
                (None, None) => None,
            }
        }
        Term::Tensor(a, b) => match (step(a), step(b)) {
            (Some(a2), _) => Some(Term::Tensor(Box::new(a2), b.clone())),
            (None, Some(b2)) => Some(Term::Tensor(a.clone(), Box::new(b2))),
            (None, None) => None,
        },
        _ => None,
    }
}

fn node_count(t: &Term) -> usize {
    match t {
        Term::Var(_) | Term::Atom(..) | Term::Unit => 1,
        Term::Comp(a, b) | Term::Tensor(a, b) => 1 + node_count(a) + node_count(b),
    }
}

fn leaf_multiset(t: &Term) -> Vec<String> {
    let mut v = Vec::new();
    fn go(t: &Term, v: &mut Vec<String>) {
        match t {
            Term::Var(x) => v.push(format!("var:{x}")),
            Term::Atom(n, _, _) => v.push(format!("atom:{n}")),
            Term::Unit => v.push("unit".into()),
            Term::Comp(a, b) | Term::Tensor(a, b) => {
                go(a, v);
                go(b, v);
            }
        }
    }
    go(t, &mut v);
    v.sort();
    v
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

    let a_ = Term::Atom("a".into(), Ty::Base(1), Ty::Base(2));
    let b_ = Term::Atom("b".into(), Ty::Base(2), Ty::Base(3));
    let c_ = Term::Atom("c".into(), Ty::Base(3), Ty::Base(4));
    let d_ = Term::Atom("d".into(), Ty::Base(4), Ty::Base(5));
    let f_ = Term::Atom("f".into(), Ty::Base(1), Ty::Base(1));

    let mut linearity_ok = true;
    let mut well_typed = 0usize;
    let mol_x = Ty::lin(Ty::Base(1), Ty::Base(2));
    let mol_y = Ty::lin(Ty::Base(3), Ty::Base(4));
    let samples: Vec<(Ctx, Term)> = vec![
        (vec![("x".into(), mol_x.clone())], Term::Var("x".into())),
        (
            vec![("x".into(), mol_x.clone()), ("y".into(), mol_y.clone())],
            Term::Tensor(
                Box::new(Term::Var("x".into())),
                Box::new(Term::Var("y".into())),
            ),
        ),
        (
            vec![],
            Term::Comp(Box::new(a_.clone()), Box::new(b_.clone())),
        ),
        (vec![], Term::Unit),
        (
            vec![],
            Term::Comp(Box::new(f_.clone()), Box::new(f_.clone())),
        ),
    ];
    for (ctx, t) in &samples {
        if let Some(_ty) = infer(ctx, t) {
            well_typed += 1;
            let mut fv = Vec::new();
            free_vars(t, &mut fv);
            let mut counts: std::collections::HashMap<String, usize> = Default::default();
            for v in &fv {
                *counts.entry(v.clone()).or_insert(0) += 1;
            }
            for (_, c) in &counts {
                if *c != 1 {
                    linearity_ok = false;
                }
            }
        } else {
            linearity_ok = false;
        }
    }
    check(
        "no-allocation (a) linear typing",
        linearity_ok,
        format!("{well_typed} sample derivations, every variable used exactly once"),
    );

    let dup = Term::Tensor(
        Box::new(Term::Var("x".into())),
        Box::new(Term::Var("x".into())),
    );
    let dup_ok = infer(&vec![("x".into(), mol_x.clone())], &dup).is_none();
    check(
        "no-allocation (a) contraction rejected",
        dup_ok,
        "x ⊗ x with x declared once does not type-check (no contraction)".to_string(),
    );
    let weak = Term::Var("x".into());
    let weak_ok =
        infer(&vec![("x".into(), mol_x.clone()), ("y".into(), mol_y.clone())], &weak).is_none();
    check(
        "no-allocation (a) weakening rejected",
        weak_ok,
        "x with y unused in context does not type-check (no weakening)".to_string(),
    );

    // Well-typed pipelines: a;b : 1⊸3, c;d : 3⊸5, (a;b);(c;d) : 1⊸5.
    let ab = Term::Comp(Box::new(a_.clone()), Box::new(b_.clone()));
    let cd = Term::Comp(Box::new(c_.clone()), Box::new(d_.clone()));
    let abcd = Term::Comp(Box::new(ab.clone()), Box::new(cd.clone()));
    let typed_ok = infer(&vec![], &ab).is_some()
        && infer(&vec![], &cd).is_some()
        && infer(&vec![], &abcd).is_some();
    check(
        "no-allocation (a) sequential composition types",
        typed_ok,
        "a;b, c;d, and (a;b);(c;d) are well-typed pipelines".to_string(),
    );

    // Interchange: (a⊗f);(b⊗g) with matching types.
    // a:1⊸2, b:2⊸3 so a;b : 1⊸3.
    // f:1⊸1, g:7⊸7 cannot tensor with b unless we pick matching wires.
    // Use f:1⊸1 on a parallel identity of a different base: take
    // p: Base(8)⊸Base(8), q: Base(8)⊸Base(8).
    let p_ = Term::Atom("p".into(), Ty::Base(8), Ty::Base(8));
    let q_ = Term::Atom("q".into(), Ty::Base(8), Ty::Base(8));
    let interchange = Term::Comp(
        Box::new(Term::Tensor(Box::new(a_.clone()), Box::new(p_.clone()))),
        Box::new(Term::Tensor(Box::new(b_.clone()), Box::new(q_.clone()))),
    );
    let interchange_ok = infer(&vec![], &interchange).is_some();
    check(
        "no-allocation (a) interchange types",
        interchange_ok,
        "(a⊗p);(b⊗q) is well-typed; reduction applies TE".to_string(),
    );

    let mut no_alloc_ok = true;
    let mut steps_total = 0usize;
    let mut cases = 0usize;
    let left_assoc = Term::Comp(
        Box::new(Term::Comp(Box::new(ab.clone()), Box::new(c_.clone()))),
        Box::new(d_.clone()),
    );
    let norm_cases = vec![ab.clone(), abcd.clone(), left_assoc, interchange];
    // f;g is ill-typed (1⊸1 then 7⊸7); skip if infer fails, still reduce as a tree.
    for t in &norm_cases {
        cases += 1;
        let mut cur = t.clone();
        let mut guard = 0usize;
        while let Some(nx) = step(&cur) {
            if leaf_multiset(&nx) != leaf_multiset(&cur) {
                no_alloc_ok = false;
            }
            if node_count(&nx) > node_count(&cur) {
                no_alloc_ok = false;
            }
            cur = nx;
            steps_total += 1;
            guard += 1;
            if guard > 1000 {
                no_alloc_ok = false;
                break;
            }
        }
    }
    check(
        "no-allocation (c) no allocation during evaluation",
        no_alloc_ok,
        format!("{cases} terms normalized in {steps_total} steps: leaf multiset preserved, node count never increases (no fresh nodes)"),
    );

    if failures == 0 {
        println!("OVERALL: PASS");
    } else {
        println!("OVERALL: FAIL ({failures} failing checks)");
        std::process::exit(1);
    }
}
