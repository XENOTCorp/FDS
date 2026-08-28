//! Compile-verification of the authoring templates (spec §4.6).
//!
//! Each template is compiled verbatim as a module file (`#[path]`, so the
//! templates' file-level `//!` docs stay valid), which requires every
//! template to compile against the public `mol` API as shipped; the
//! templates' bundled unit tests also run here. This is the guard that
//! template changes never drift from the crate's API.

#[path = "../../../templates/pure_atom.rs"]
mod pure_atom;

#[path = "../../../templates/effectful_atom.rs"]
mod effectful_atom;

#[path = "../../../templates/hybrid_molecule.rs"]
mod hybrid_molecule;

#[path = "../../../templates/reactor_loop.rs"]
mod reactor_loop;
