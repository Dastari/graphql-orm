//! Source-qualified admission: see the adjacent accounting reference.
use super::*;
pub(super) const WORK: u64 = 16_777_216;
pub(super) const TEXT: u64 = 67_108_864;
#[derive(Default, Clone, Copy)]
pub(super) struct Cost {
    pub w: u64,
    pub t: u64,
}
impl Cost {
    pub fn new(w: u64, t: u64) -> Self {
        Self { w, t }
    }
    pub fn add(self, b: Self) -> R<Self> {
        Ok(Self {
            w: add(self.w, b.w)?,
            t: add(self.t, b.t)?,
        })
    }
    pub fn times(self, n: u64) -> R<Self> {
        Ok(Self {
            w: mul(self.w, n)?,
            t: mul(self.t, n)?,
        })
    }
}
pub(super) fn add(a: u64, b: u64) -> R<u64> {
    a.checked_add(b).ok_or(E::LimitExceeded)
}
pub(super) fn mul(a: u64, b: u64) -> R<u64> {
    a.checked_mul(b).ok_or(E::LimitExceeded)
}
pub(super) struct Ledger {
    pub used: Cost,
}
impl Default for Ledger {
    fn default() -> Self {
        Self {
            used: Cost::new(4096, 256),
        }
    }
}
impl Ledger {
    pub fn reserve(&mut self, c: Cost) -> R<()> {
        let next = self.used.add(c)?;
        if next.w > WORK || next.t > TEXT {
            return Err(E::LimitExceeded);
        }
        self.used = next;
        Ok(())
    }
}
#[derive(Default, Clone, Copy)]
pub(super) struct Size {
    pub n: u64,
    pub b: u64,
    pub longest: u64,
}
impl Size {
    pub fn m(self) -> R<u64> {
        add(self.n, self.b)
    }
    pub fn debug(self) -> R<u64> {
        add(mul(256, self.n)?, mul(10, self.b)?)
    }
    pub fn copy(self) -> R<Cost> {
        Ok(Cost::new(mul(3, self.n)?, self.b))
    }
    pub fn cmp(self, b: Self) -> R<u64> {
        add(1, add(self.m()?, b.m()?)?)
    }
}
pub(super) struct Inventory<'a> {
    pub size: Size,
    ledger: &'a mut Ledger,
    cap: u64,
}
impl<'a> Inventory<'a> {
    pub fn new(ledger: &'a mut Ledger, cap: usize) -> Self {
        Self {
            size: Size::default(),
            ledger,
            cap: cap as u64,
        }
    }
    pub fn scalar(&mut self) -> R<()> {
        self.ledger.reserve(Cost::new(64, 0))?;
        self.size.n = add(self.size.n, 1)?;
        if self.size.n > 2_000_000 {
            return Err(E::LimitExceeded);
        }
        Ok(())
    }
    pub fn ownership(&mut self, set: &ManagedTableSet) -> R<()> {
        self.ledger.reserve(ownership_tree(set.0.len() as u64)?)?;
        self.scalar()?;
        for name in set.tables() {
            self.string(name)?;
        }
        Ok(())
    }
    pub fn member(&mut self, key: &str) -> R<()> {
        self.string(key)
    }
    pub fn string(&mut self, s: &str) -> R<()> {
        self.scalar()?;
        let n = s.len() as u64;
        if n > 8_388_608 {
            return Err(E::LimitExceeded);
        }
        self.ledger.reserve(Cost::new(add(128, mul(4, n)?)?, 0))?;
        self.size.b = add(self.size.b, n)?;
        if self.size.b > self.cap {
            return Err(E::LimitExceeded);
        }
        self.size.longest = self.size.longest.max(n);
        Ok(())
    }
}
pub(super) fn size<T: wire::Wire>(x: &T, l: &mut Ledger, cap: usize) -> R<Size> {
    let mut i = Inventory::new(l, cap);
    x.inventory(&mut i)?;
    Ok(i.size)
}
fn sort(n: u64, key: u64) -> R<Cost> {
    let (c, m) = if n < 2 {
        (0, 1)
    } else if n <= 20 {
        (mul(n, n - 1)? / 2, add(mul(2, mul(n, n)?)?, mul(2, n)?)?)
    } else {
        let q = 64 - (n + 1).leading_zeros() as u64;
        let c = add(add(mul(4, mul(n, n)?)?, mul(8, mul(n, q)?)?)?, mul(64, n)?)?;
        (c, mul(4, c)?)
    };
    Ok(Cost::new(add(m, mul(c, add(key, 2)?)?)?, 0))
}
fn op(n: u64, key: u64) -> R<Cost> {
    Ok(Cost::new(mul(add(n, 1)?, add(64, key)?)?, 0))
}
fn build(n: u64, key: u64) -> R<Cost> {
    // Sum the arithmetic progression; FromIterator's hidden stable sort is additional.
    sort(n, key)?.add(Cost::new(mul(mul(n, add(n, 1)?)? / 2, add(64, key)?)?, 0))
}
fn norm(s: &str, check: bool, l: &mut Ledger) -> R<Cost> {
    l.reserve(Cost::new(add(64, mul(4, s.len() as u64)?)?, 0))?;
    let n = s.len() as u64;
    let a = s.bytes().filter(|b| *b == b'(').count() as u64;
    let b = s.bytes().filter(|b| *b == b')').count() as u64;
    let r = a.min(b).min(n / 2);
    let work = add(
        if check { 256 } else { 512 },
        mul(64, mul(add(n, 1)?, add(r, 1)?)?)?,
    )?;
    Ok(Cost::new(
        add(work, mul(2, n)?)?,
        if check {
            mul(64, add(n, 1)?)?
        } else {
            add(64, mul(8, mul(add(n, 1)?, add(r, 1)?)?)?)?
        },
    ))
}
fn norms(table: &OwnedTableModel, l: &mut Ledger) -> R<Cost> {
    l.reserve(Cost::new(
        mul(
            128,
            add(
                table.columns.len() as u64,
                table.check_constraints.len() as u64,
            )?,
        )?,
        0,
    ))?;
    let mut c = Cost::default();
    for col in &table.columns {
        if let Some(s) = &col.default {
            c = c.add(norm(s, false, l)?)?;
        }
    }
    for check in &table.check_constraints {
        c = c.add(norm(&check.expression, true, l)?)?;
    }
    Ok(c)
}
fn table_size(t: &OwnedTableModel, l: &mut Ledger, cap: usize) -> R<Size> {
    size(t, l, cap)
}
/// These helpers are upper bounds from counts/maxima only, with no matching,
/// ownership decision, speculative step, normalization, rendering or hashing.
pub(super) fn profile(s: Size) -> R<Cost> {
    Ok(Cost::new(add(mul(64, s.n)?, mul(20, s.b)?)?, s.debug()?))
}
pub(super) fn target(s: &OwnedSchemaModel, sz: Size, l: &mut Ledger, cap: usize) -> R<Cost> {
    l.reserve(Cost::new(mul(256, sz.n)?, 0))?;
    let t = s.tables.len() as u64;
    let key = add(2, mul(2, sz.longest)?)?;
    let mut c = build(t, key)?.add(Cost::new(mul(128, sz.n)?, add(256, mul(4, sz.b)?)?))?;
    for table in &s.tables {
        let z = table_size(table, l, cap)?;
        let cols = table.columns.len() as u64;
        // Every member-set insertion, nested source/target column search, alternative
        // unique key and FK semantic-key clone. Full subtree comparisons cover keys.
        let members = table
            .indexes
            .iter()
            .map(|i| i.columns.len() as u64)
            .sum::<u64>();
        c = c
            .add(op(cols, key)?.times(members)?)?
            .add(Cost::new(mul(members, mul(cols, key)?)?, 0))?;
        for fk in &table.foreign_keys {
            let k = fk.column_pairs.len() as u64;
            c = c
                .add(op(t, key)?)?
                .add(op(table.foreign_keys.len() as u64, z.cmp(z)?)?)?;
            c = c.add(op(k, key)?.times(mul(2, k)?)?)?;
            c = c.add(Cost::new(
                mul(add(1, k)?, mul(add(sz.n, 1)?, add(sz.m()?, 1)?)?)?,
                mul(2, z.b)?,
            ))?;
        }
    }
    Ok(c)
}
pub(super) fn effects(
    a: &OwnedSchemaModel,
    b: &OwnedSchemaModel,
    sa: Size,
    sb: Size,
    owners: u64,
    l: &mut Ledger,
    cap: usize,
) -> R<Cost> {
    l.reserve(Cost::new(mul(256, add(sa.n, sb.n)?)?, 0))?;
    let key = add(2, mul(2, sa.longest.max(sb.longest))?)?;
    let mut c = op(owners, key)?.times(add(a.tables.len() as u64, b.tables.len() as u64)?)?;
    c = c.add(Cost::new(mul(add(sa.n, 1)?, add(sb.m()?, 1)?)?, 0))?;
    // An incoming unmanaged FK can repeat complete table/column matching and
    // TWO default normalizers per present default on each side.
    let edges = a
        .tables
        .iter()
        .map(|t| t.foreign_keys.len() as u64)
        .sum::<u64>();
    // Each possible unmanaged incoming FK repeats the ownership lookup.
    c = c.add(op(owners, key)?.times(edges)?)?;
    let mut columns = Cost::default();
    for t in a.tables.iter().chain(&b.tables) {
        let z = table_size(t, l, cap)?;
        columns = columns
            .add(norms(t, l)?.times(2)?)?
            .add(z.copy()?.times(2)?)?;
    }
    c = c.add(columns.times(add(edges, 1)?)?)?;
    c = c.add(Cost::new(
        mul(edges, mul(add(sa.n, 1)?, add(sb.m()?, 1)?)?)?,
        0,
    ))?;
    // Scoping: all lookups/searches and full source clone, never ownership credit.
    c = c
        .add(sa.copy()?)?
        .add(op(owners, key)?.times(a.tables.len() as u64)?)?;
    Ok(c)
}
pub(super) fn diff(
    a: &OwnedSchemaModel,
    b: &OwnedSchemaModel,
    sa: Size,
    sb: Size,
    l: &mut Ledger,
    cap: usize,
) -> R<Cost> {
    l.reserve(Cost::new(mul(256, add(sa.n, sb.n)?)?, 0))?;
    let key = add(2, mul(2, sa.longest.max(sb.longest))?)?;
    let mut c = Cost::default();
    for (s, z) in [(a, sa), (b, sb)] {
        let t = s.tables.len() as u64;
        let f = s
            .tables
            .iter()
            .map(|x| x.foreign_keys.len() as u64)
            .sum::<u64>();
        c = c
            .add(build(t, key)?)?
            .add(build(s.extensions.len() as u64, key)?)?;
        c = c.add(op(t, key)?.times(add(mul(3, add(t, f)?)?, add(f, mul(3, t)?)?)?)?)?;
        c = c
            .add(z.copy()?.times(4)?)?
            .add(Cost::new(mul(256, z.n)?, mul(4, z.b)?))?;
    }
    // Conservative common-table envelope: repeat each side's complete table
    // descriptor against every possible opposite table, without comparing names.
    for (s, other) in [(a, b), (b, a)] {
        for t in &s.tables {
            let z = table_size(t, l, cap)?;
            let mut pair = Cost::default();
            for n in [
                t.columns.len(),
                t.indexes.len(),
                t.search_indexes.len(),
                t.foreign_keys.len(),
            ] {
                let n = n as u64;
                pair = pair
                    .add(build(n, z.cmp(z)?)?)?
                    .add(op(n, z.cmp(z)?)?.times(mul(4, add(n, 1)?)?)?)?;
            }
            let mut token_key = 0;
            for check in &t.check_constraints {
                token_key = add(token_key, mul(66, add(check.expression.len() as u64, 1)?)?)?;
            }
            pair = pair.add(sort(
                t.check_constraints.len() as u64,
                add(2, mul(2, token_key)?)?,
            )?)?;
            // FK grouping on both sides, union-key collect, per-group constraint sorts.
            let f = t.foreign_keys.len() as u64;
            pair = pair
                .add(build(mul(2, f)?, z.cmp(z)?)?)?
                .add(sort(f, z.cmp(z)?)?.times(mul(2, add(f, 1)?)?)?)?;
            pair = pair.add(norms(t, l)?.times(4)?)?.add(z.copy()?.times(8)?)?;
            pair = pair.add(Cost::new(mul(256, z.n)?, mul(4, z.b)?))?;
            c = c.add(pair.times(other.tables.len() as u64)?)?;
        }
    }
    Ok(c)
}
pub(super) fn schema_hash(s: &OwnedSchemaModel, z: Size, l: &mut Ledger, cap: usize) -> R<Cost> {
    l.reserve(Cost::new(mul(256, z.n)?, 0))?;
    let key = add(2, mul(2, z.longest)?)?;
    let mut c = sort(s.tables.len() as u64, key)?.add(sort(s.extensions.len() as u64, key)?)?;
    let mut checks = 0;
    let mut defaults = 0;
    for t in &s.tables {
        let tz = table_size(t, l, cap)?;
        c = c.add(norms(t, l)?)?;
        for n in [
            t.columns.len(),
            t.indexes.len(),
            t.search_indexes.len(),
            t.foreign_keys.len(),
        ] {
            c = c.add(sort(n as u64, tz.cmp(tz)?)?)?;
        }
        let mut checkbytes = 0;
        for k in &t.check_constraints {
            checkbytes = add(checkbytes, mul(65, add(k.expression.len() as u64, 1)?)?)?;
        }
        checks = add(checks, checkbytes)?;
        c = c.add(sort(
            t.check_constraints.len() as u64,
            add(2, mul(2, checkbytes)?)?,
        )?)?;
        defaults = add(
            defaults,
            t.columns.iter().filter(|x| x.default.is_some()).count() as u64,
        )?;
        for search in &t.search_indexes {
            let iz = size(search, l, cap)?;
            for n in [
                search.fields.len(),
                search.json_paths.len(),
                search.relations.len(),
            ] {
                c = c.add(sort(n as u64, iz.cmp(iz)?)?)?;
            }
        }
    }
    let h = add(
        add(mul(256, z.n)?, mul(2, z.b)?)?,
        add(checks, mul(32, defaults)?)?,
    )?;
    // Canonical buffer, joins, token joins/fallback copies, numbers, append visits,
    // FNV and hex; canonical checks may expand far beyond their original strings.
    c.add(Cost::new(
        add(mul(128, z.n)?, add(mul(16, z.b)?, mul(4, h)?)?)?,
        add(add(mul(4, h)?, mul(4, z.b)?)?, 16)?,
    ))
}
pub(super) fn render(
    steps: &[OwnedMigrationStep],
    a: &OwnedSchemaModel,
    b: &OwnedSchemaModel,
    l: &mut Ledger,
    cap: usize,
) -> R<Cost> {
    l.reserve(Cost::new(
        mul(256, add(a.tables.len() as u64, b.tables.len() as u64)?)?,
        0,
    ))?;
    let mut c = Cost::default();
    let mut max = Size::default();
    let mut maxnorm = Cost::default();
    for t in a.tables.iter().chain(&b.tables) {
        let z = table_size(t, l, cap)?;
        max.n = max.n.max(z.n);
        max.b = max.b.max(z.b);
        max.longest = max.longest.max(z.longest);
        let n = norms(t, l)?;
        maxnorm.w = maxnorm.w.max(n.w);
        maxnorm.t = maxnorm.t.max(n.t);
    }
    let count = steps.len() as u64;
    let key = add(2, mul(2, max.longest)?)?;
    c = c
        .add(build(a.tables.len() as u64, key)?)?
        .add(build(b.tables.len() as u64, key)?)?
        .add(build(count, key)?)?
        .add(op(count, key)?.times(mul(4, count)?)?)?;
    for step in steps {
        let z = size(step, l, cap)?;
        // Closed renderer literal/escaping envelope. 4096 per slot covers all pinned
        // SQL templates, repeated placeholders, joins, naming hashes, fragments and
        // search JSON->SQL escaping (8 materialization layers). 512 per input byte
        // dominates quote/path/hex/name and nested config materializations.
        let frame = add(mul(4096, z.n)?, mul(512, z.b)?)?;
        c = c.add(Cost::new(mul(2, frame)?, frame))?;
        // Every step conservatively prepays a complete SQLite rebuild, including
        // full column Cartesian search, INSERT's repeated column list and all target
        // index/append-only children. No deduplication or backend branch discount.
        let rebuild = add(mul(4096, max.n)?, mul(512, max.b)?)?;
        c = c
            .add(Cost::new(
                add(mul(2, rebuild)?, mul(max.n, max.m()?)?)?,
                rebuild,
            ))?
            .add(maxnorm.times(4)?)?;
        // AlterColumn's second column comparison/default target clone and renderer
        // normalization; all other variant payload defaults/checks prepaid as well.
        if let OwnedMigrationStep::AlterColumn { before, after, .. } = step {
            for col in [before, after] {
                if let Some(v) = &col.default {
                    c = c.add(norm(v, false, l)?.times(4)?)?;
                }
            }
        }
        if let OwnedMigrationStep::CreateTable(t) = step {
            c = c.add(norms(t, l)?.times(4)?)?;
        }
        if let OwnedMigrationStep::AddColumn { column, .. } = step {
            if let Some(v) = &column.default {
                c = c.add(norm(v, false, l)?.times(2)?)?;
            }
        }
    }
    c.add(Cost::new(128, 64))
}
pub(super) fn integrity(
    steps: &[OwnedPlannedMigrationStep],
    sql: &[String],
    identity: Size,
    l: &mut Ledger,
    cap: usize,
) -> R<Cost> {
    let mut lines = 0;
    let mut work = 0;
    for step in steps {
        let z = size(&step.step, l, cap)?;
        lines = add(lines, add(256, add(z.debug()?, step.reason.len() as u64)?)?)?;
        work = add(work, add(mul(32, z.n)?, mul(16, z.b)?)?)?;
    }
    let mut p = add(32, lines)?;
    for s in sql {
        p = add(p, add(s.len() as u64, 1)?)?;
    }
    let wrapper = add(256, identity.b)?;
    Cost::new(
        add(work, add(mul(4, p)?, mul(4, wrapper)?)?)?,
        add(add(lines, p)?, add(mul(2, wrapper)?, 32)?)?,
    )
    .add(ownership_tree(identity.n)?)
}

pub(super) fn ownership_cost(n: u64, longest: u64, bytes: u64) -> R<Cost> {
    op(n, add(2, mul(2, longest)?)?)?
        .times(n)?
        .add(Cost::new(mul(4, bytes)?, 0))
}
pub(super) fn admit_ownership(names: &[String], l: &mut Ledger) -> R<()> {
    if names.len() > 512 {
        return Err(E::LimitExceeded);
    }
    l.reserve(Cost::new(mul(128, names.len() as u64)?, 0))?;
    let longest = names.iter().map(|s| s.len() as u64).max().unwrap_or(0);
    l.reserve(ownership_cost(
        names.len() as u64,
        longest,
        names.iter().map(|s| s.len() as u64).sum(),
    )?)
}

pub(super) fn ownership_tree(n: u64) -> R<Cost> {
    Ok(Cost::new(mul(64, add(n, 1)?)?, 0))
}
