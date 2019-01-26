# Useful definitions

## [`DefIndex`](src/librustc/hir/def_id.rs)

```rust
/// A DefIndex is an index into the hir-map for a crate, identifying a
/// particular definition. It should really be considered an interned
/// shorthand for a particular DefPath.
///
/// At the moment we are allocating the numerical values of DefIndexes from two
/// address spaces: DefIndexAddressSpace::Low and DefIndexAddressSpace::High.
/// This allows us to allocate the DefIndexes of all item-likes
/// (Items, TraitItems, and ImplItems) into one of these spaces and
/// consequently use a simple array for lookup tables keyed by DefIndex and
/// known to be densely populated. This is especially important for the HIR map.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Copy)]
pub struct DefIndex(u32);
```

## [`DefId`](src/librustc/hir/def_id.rs)

```rust
/// A `DefId` identifies a particular *definition*, by combining a crate
/// index and a def index.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Copy)]
pub struct DefId {
    pub krate: CrateNum,
    pub index: DefIndex,
}
```

## [`CrateNum`](src/librustc/hir/def_id.rs)

```rust
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CrateNum {
    /// Virtual crate for builtin macros
    // FIXME(jseyfried): this is also used for custom derives until proc-macro crates get
    // `CrateNum`s.
    BuiltinMacros,
    /// A special CrateNum that we use for the tcx.rcache when decoding from
    /// the incr. comp. cache.
    ReservedForIncrCompCache,
    Index(CrateId),
}
```

## [`DefPath`](src/librustc/hir/map/definitions.rs)

```rust
#[derive(Clone, Debug, Hash, RustcEncodable, RustcDecodable)]
pub struct DefPath {
    /// the path leading from the crate root to the item
    pub data: Vec<DisambiguatedDefPathData>,

    /// what krate root is this path relative to?
    pub krate: CrateNum,
}
```

## [`DisambiguatedDefPathData`](src/librustc/hir/map/definitions.rs)
```rust
/// Pair of `DefPathData` and an integer disambiguator. The integer is
/// normally 0, but in the event that there are multiple defs with the
/// same `parent` and `data`, we use this field to disambiguate
/// between them. This introduces some artificial ordering dependency
/// but means that if you have (e.g.) two impls for the same type in
/// the same module, they do get distinct def-ids.
#[derive(Clone, PartialEq, Debug, Hash, RustcEncodable, RustcDecodable)]
pub struct DisambiguatedDefPathData {
    pub data: DefPathData,
    pub disambiguator: u32
}
```

## [`DefPathData`](src/librustc/hir/map/definitions.rs)
```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash, RustcEncodable, RustcDecodable)]
pub enum DefPathData {
    // Root: these should only be used for the root nodes, because
    // they are treated specially by the `def_path` function.
    /// The crate root (marker)
    CrateRoot,
    // Catch-all for random DefId things like DUMMY_NODE_ID
    Misc,
    // Different kinds of items and item-like things:
    /// An impl
    Impl,
    /// A trait
    Trait(InternedString),
    /// An associated type **declaration** (i.e., in a trait)
    AssocTypeInTrait(InternedString),
    /// An associated type **value** (i.e., in an impl)
    AssocTypeInImpl(InternedString),
    /// An existential associated type **value** (i.e., in an impl)
    AssocExistentialInImpl(InternedString),
    /// Something in the type NS
    TypeNs(InternedString),
    /// Something in the value NS
    ValueNs(InternedString),
    /// A module declaration
    Module(InternedString),
    /// A macro rule
    MacroDef(InternedString),
    /// A closure expression
    ClosureExpr,
    // Subportions of items
    /// A type parameter (generic parameter)
    TypeParam(InternedString),
    /// A lifetime definition
    LifetimeParam(InternedString),
    /// A variant of a enum
    EnumVariant(InternedString),
    /// A struct field
    Field(InternedString),
    /// Implicit ctor for a tuple-like struct
    StructCtor,
    /// A constant expression (see {ast,hir}::AnonConst).
    AnonConst,
    /// An `impl Trait` type node
    ImplTrait,
    /// GlobalMetaData identifies a piece of crate metadata that is global to
    /// a whole crate (as opposed to just one item). GlobalMetaData components
    /// are only supposed to show up right below the crate root.
    GlobalMetaData(InternedString),
    /// A trait alias.
    TraitAlias(InternedString),
}
```

# Idea

According to
https://rust-lang.github.io/rustc-guide/lowering.html
the only thing that AST->HIR lowering changes for items are `impl Trait`-related
things:
* Universal `impl Trait`

    Converted to generic arguments (but with some flags, to know that the user didn't write them)

* Existential `impl Trait`

    Converted to a virtual `existential type` declaration

Currently definitions are created:
- with `DefCollector` for post-expansion AST nodes (in `resolve`)
- when lowering AST->HIR

### Crazy idea (lazify definitions) (NodeId -> DefId)
- use barebones parser that only parses item trees (without bodies, not including statements, e.g. `let x = `, or expressions, e.g. `if {}`)
- Create those definitions straight after parsing (pre-expansion)
- if not found, given an item path, during name resolution expand macros? until
we find a given definition

Figure out if lazy 'definition' across different files/modules might affect stability wrt incr. comp.

### HIR Map (lazify lowering) (DefId -> HirId/HirBody?)