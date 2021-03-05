#![feature(box_syntax, box_patterns, const_mut_refs, label_break_value)]
#![allow(dead_code, unused_parens)]
use std::rc::Rc;
use std::fmt;
use std::fmt::Debug;
use std::mem;

extern crate ramp;
extern crate dyn_clone;
extern crate linkme;
use ramp::Int;

use Skew::*;
use Twist::*;
mod jets;
use jets::{Jet, Jetted};
mod turboprop;
use turboprop::{turboprop};

#[derive(Clone, PartialEq, Debug)]
pub enum Skew {
    S,
    K,
    E,
    W,
    X,
    Q,
    A(Rc<Int>),
    // Lazy(Box<dyn Stand>) that we decompose into a concrete Skew?
    // how do we do this efficiently in the Skew::reduce function -
    // since we're always matching the header we just expand a Lazy
    // if we have one, and then expand them if we need conditional matching
    // for e.g. the Jet arity, but not hint?
    // (this is so we can have a jet return a native HashMap that can be used with
    // skew and a native get/set jet on the HashMap, with lazy reduction down to
    // actual skew if needed)
}

#[derive(Clone, PartialEq)]
pub enum Twist {
    Expr(Rc<Vec<Twist>>),
    N(Skew),
    J(Jet),
    Turbo(&'static dyn Jetted), // A jet we register at compile-time and special-case in the vm
}

#[inline]
fn cons(mut exprs: Vec<Twist>) -> Twist {
    match &mut exprs.as_mut_slice() {
        // flatten ((x y) z) -> (x y z)
        // XXX: have pre-registered jets have a fastpath so we dont reallocate
        // them to append arguments?
        [Expr(ref mut head), tail @ ..] if tail.len() > 0 => {
            //println!("flattening {:?} {:?}", head, tail);
            let l = Rc::make_mut(head);
            l.extend_from_slice(tail);
            //println!("-> {:?}", l);
            return Expr(Rc::new(l.to_vec()))
        },
        _ => Expr(Rc::new(exprs))
    }
}

#[macro_export]
macro_rules! skew {
    (S) => { N(S) };
    (K) => { N(K) };
    (E) => { N(E) };
    (W) => { N(W) };
    (X) => { N(X) };
    (Q) => { N(Q) };
    ( ($( $x:tt ),+)) => {
        {
            let mut temp_vec: Vec<Twist> = Vec::new();
            $(
                temp_vec.push(skew!($x));
            )+
            cons(temp_vec)
        }
    };
    ({A $x:expr}) => { Twist::atom($x) };
    ({$x:expr}) => { $x.clone() };
}

// skew is defined as left-associative: (x y z) is grouped as ((x y) z)
// this is a problem for pattern matching because you have to chase for the combinator
// tag multiple times, since it's not always at the top level.
// we represent ((x y) z) as vec![x,y,z] and (x (y z)) as vec![x,vec![y,z]] instead.

// how to make this faster:
// have exprs been a stack, and apply reductions at the *tail* so that
// we can do [... z y x S] -> [... (z y) (z x)] reductions in-place with make_mut
//
// we should also be using a stack vm for this, for E argument reduction
// without blowing the call stack.
// currently "evaluate arguments" for E means you have to call reduce() in a loop
// until it returns None, which means its recursive and will build deep call stacks.
//
// problem: "evaluating" is defined as running until fixpoint.
// this means that "data structures" are evaluated - (get my_map 'key) would need
// my_map in a way that doesn't reduce.
// we could just have it as `E A(1) K (map data)` with no arguments?
// then all get's impl has to match on `E A(1) _ J(Jet(map))` instead, but we keep
// semantics.
// can we just use (K map) instead? does that ruin codegen? idk how SKI compilers
// work.
//
// use smarter Rc<Ints> that dont require cloning if theyre <64 bits - we also
// don't need *signed* ints

impl Twist {
    /// Generate a new atom from a number
    fn atom(n: usize) -> Self {
        N(A(Rc::new(Int::from(n))))
    }
    /// Reduce until there's nothing left
    fn boil(&mut self) {
        let mut curr = N(K);
        mem::swap(&mut curr, self);
        loop {
            println!("boiling {:?}", curr);
            if let Some(next) = curr.reduce() {
                curr = next;
            } else {
                mem::swap(self, &mut curr);
                break;
            }
        }
    }
    /// Reduce a Twist one step
    #[inline]
    fn reduce(&self) -> Option<Self> {
        if let Expr(exprs) = self {
            let o: Option<Self> = match &exprs.as_slice() {
                prop @ [Turbo(_), ..] => turboprop::turboprop(prop),
                [N(K), x, _y, z @ ..] => {
                    if z.len() > 0 {
                        let mut v = vec![x.clone()];
                        v.extend_from_slice(z);
                        Some(cons(v))
                    } else {
                        Some(x.clone())
                    }
                },
                [N(S), x, y, z, w @ ..] => {
                    let mut s = vec![];
                    let mut xz = vec![x.clone(), z.clone()];
                    s.append(&mut xz);
                    let yz = vec![y.clone(), z.clone()];
                    s.push(cons(yz));
                    if(w.len() != 0){
                        s.extend_from_slice(w);
                    }
                    Some(cons(s))
                },
                [N(E), N(A(n)), _t, f, x @ ..] if x.len() >= **n => {
                    let mut arity = n;
                    let mut jetted = None;
                    println!("jet arity {}", arity);
                    let s: usize = (&**n).into();
                    let new_x = &mut x.to_owned();
                    // this leads to unnecessary allocations i think
                    for item in new_x[..s].iter_mut() {
                        item.boil();
                    }
                    println!("after reduction {:?}", new_x);
                    if let J(jet) = f {
                        if jet.0.arity() == **arity {
                            jetted = jet.0.call(&mut new_x[..s])
                        } else {
                            println!("jet arity doesnt match");
                        }
                    }
                    if let Some(jet_val) = jetted {
                        if(new_x.len() > s){
                            println!("function call with too many arguments");
                            let mut many = vec![jet_val];
                            many.extend_from_slice(&mut new_x[s..]);
                            return Some(cons(many));
                        } else {
                            return Some(jet_val);
                        }
                    } else {
                        // we didn't have a Jet as a function, but still have a hint
                        // search for it in the jet registry?
                        // this actually has to be f.deoptimize()
                        let mut unjetted = vec![f.clone()];
                        unjetted.extend_from_slice(new_x);
                        return Some(cons(unjetted));
                    }
                },
                [N(X), N(A(n)), x @ ..] => {
                    Some(N(A(Rc::new((**n).clone()+1))))
                },
                // does this work? do i need to eval for e first?
                // do i add a `if e.len() >= n`?
                [N(W), N(A(n)), Expr(e), x @ ..] => {
                    let s: usize = (&**n).into();
                    if x.len() > 0 {
                        let mut r = vec![e[s].clone()];
                        r.extend_from_slice(x.clone());
                        Some(cons(r))
                    } else {
                        Some(e[s].clone())
                    }
                },
                [N(Q), n, m, x @ ..] => {
                    (||{
                        if let (N(A(n)), N(A(m))) = (n, m) {
                            if n == m {
                                return Some(Twist::atom(0))
                            }
                        }
                        return Some(Twist::atom(1))
                    })()
                }
                //// these rules force reduction of E arguments first
                //// it also ruins our cache coherency though :(
                //[x @ .., y @ _] if cons(x.into()).reduce().is_some() => {
                //    let mut xy: Vec<Twist> = vec![cons(x.into()).reduce().unwrap()];
                //    xy.push(y.clone());
                //    Some(cons(xy))
                //},
                //[x @ .., y @ _] if y.reduce().is_some() => {
                //    let mut xy: Vec<Twist> = x.into();
                //    xy.push(y.reduce().unwrap());
                //    Some(cons(xy))
                //},

                // probably need a J(jet) => jet.deoptimize() rule here
                _ => None,
            };
            //if let Some(x) = o.clone() {
            //    println!("reducing {:?}", self);
            //    println!("-> {:?}", x);
            //}
            o
        } else {
            None
        }
    }
}

impl fmt::Debug for Twist {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            N(c) => c.fmt(f),
            Expr(expr) => {
                write!(f, "(")?;
                let mut first = true;
                for e in expr.iter() {
                    if first {
                        first = false;
                    } else {
                        write!(f, " ")?;
                    }
                    std::fmt::Debug::fmt(e, f)?;
                }
                write!(f, ")")
            },
            J(j) => {
                write!(f, "{}", j.0.name())
            },
            Turbo(prop) => {
                write!(f, "{}", prop.name())
            }
        }
    }
}

mod lambda;
use lambda::*;
use lambda::{Lambda, LTerm};
fn main() {
    let mut lam = Lambda::Func("x", box Lambda::Term(LTerm::Var("x")));
    assert_eq!(lam.transform().open(), skew![(S, K, K)]);

    let mut swap = Lambda::Func("x", box Lambda::Func("y",
        box Lambda::App(
            box Lambda::Term(LTerm::Var("y")),
            box Lambda::Term(LTerm::Var("x"))
        )
    ));

    println!("before: {:?}", swap);
    let twist_swap = swap.transform().open();
    println!("after: {:?}", twist_swap);

    let mut test_swap = skew![({twist_swap}, {A 1}, {A 2})];
    println!("test swap: {:?}", test_swap);
    test_swap.boil();
    assert_eq!(test_swap, skew![({A 2}, {A 1})]);
}

mod test {
    use crate::Skew::*;
    use crate::Twist::*;
    use crate::Twist;
    use crate::Jet;
    use crate::cons;
    #[test]
    fn test_k() {
        let t = skew![(K, K, K)].reduce().unwrap();
        assert_eq!(t, N(K));
    }
    #[test]
    fn test_k_applies_first() {
        let t = skew![(K, K, (S, K, K))].reduce().unwrap();
        assert_eq!(t, N(K));
    }
    #[test]
    fn test_s() {
        let t1 = skew![(S, K, (S, K), (S, K, K))].reduce().unwrap();
        assert_eq!(t1, skew![(K, (S, K, K), (S, K, (S, K, K)))]);
        let t2 = t1.reduce().unwrap();
        assert_eq!(t2, skew![(S, K, K)]);
    }

    #[test]
    fn test_s2() {
        let t1 = skew![(S, K, K, K)].reduce().unwrap();
        assert_eq!(t1, skew![(K, K, (K, K))]);
    }
    #[test]
    pub fn test_e() {
        //crate::main()
    }
    #[test]
    pub fn test_q() {
        let t1 = skew![(Q, {A 5}, {A 5})].reduce().unwrap();
        assert_eq!(t1, Twist::atom(0));
        let t2 = skew![(Q, {A 5}, {A 6})].reduce().unwrap();
        assert_eq!(t2, Twist::atom(1));
    }
    #[test]
    fn test_increment() {
        let t1 = cons(vec![N(X), Twist::atom(1)]).reduce().unwrap();
        assert_eq!(t1, Twist::atom(2));
    }

    // TODO: add I and Swap jets, fast-path them in reduce() pattern matching?
    // maybe add C and B combiniator jets too
    #[test]
    fn test_i() {
        let i = skew![(S,K,K)];
        let mut apply_i = cons(vec![i.clone(), Twist::atom(1)]);
        apply_i.boil();
        assert_eq!(apply_i, Twist::atom(1));
    }

    #[test]
    fn test_swap() {
        let i = skew![(S,K,K)];
        let swap = skew![(S, (K, (S, {i})), K)];
        let mut apply_swap = skew![({swap}, {A 1}, K, K)];
        for i in 0..6 {
            println!("before reduce {:?}", apply_swap);
            apply_swap = apply_swap.reduce().unwrap();
            println!("reduce swap {:?}", apply_swap);
        }
        assert_eq!(apply_swap, cons(vec![N(K), Twist::atom(1), N(K)]));
    }

    #[test]
    fn test_pick() {
        let t1 = skew![(W, {A 1}, ({A 0}, {A 1}, {A 2}))];
        assert_eq!(t1.reduce().unwrap(), Twist::atom(1));
    }
}
