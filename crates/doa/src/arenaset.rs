//!ArenaSet: the block-of-blocks tier — many small BlockSets (u8 names,
//!cap 256) under one flat index, the shape the substrate was built for.
//!routing: a `BTreeMap<min-key clone, block id>` — the floor entry (the
//!greatest min ≤ k) owns k; k below every min routes to the first block
//!(a below-min insert, the only min-changing case — the index entry is
//!replaced). the index's order IS the block order: iteration walks its
//!values.
//!full blocks split: k beyond the tail extreme or below the min spawns
//!a FRESH block holding just k (no renames — k never landed); a mid-gap
//!k cleaves the full block 50/50 and ONE closed-form event reports it:
//!every element at position ≥ split_pos (mapped through from_tr) now
//!lives in the new block WITH THE SAME NAME (the halves copy as-is and
//!the translator's offset shift preserves names) — survivors and
//!boundary spawns rename nothing.
//!the per-block z-gate cannot fire on split-born halves (a dense
//!128-prefill needs len > 255 to trip — unreachable), so the ARENA
//!flips eagerly: a below-min k into a non-reversed block past warmup
//!reverses it first (one O(len) flip beats a per-insert front slide).
//!removal/merging: v0 (ids and names immortal, the arena never
//!shrinks). deltas: the touched block's own slot carries its name
//!swaps; `take_split` yields the cleave event — apply both before the
//!next insert.

use std::collections::BTreeMap;

use crate::blockset::{BlockSet, Deltas, FREE_STRIDE};
use crate::serialize::Archive;
use crate::translator::Translator;

///the cleave event: every element of `from_aa` at position ≥ `split_pos`
///(mapped through `from_tr`) now lives in `to_aa` **with the same
///name** — the halves copy slots, heap, and translator as-is (the
///offset shift preserves names), so only the block id changes.
///elements below `split_pos` keep their names and block; boundary
///spawns record no split (k never landed in the old block)
pub struct Split {
    pub from_aa:   u32,
    pub from_tr:   Translator<u8>,
    pub to_aa:     u32,
    pub split_pos: usize,
}

impl Split {
    ///the holder's predicate: did the name move to `to_aa`? (kept as a
    ///method because the translator's maps are crate-private — the raw
    ///fields are for in-crate tests)
    pub fn moved(&self, name: u8) -> bool {
        self.from_tr.a2p(name).0 >= self.split_pos
    }
}

///block-of-blocks set over archived keys (see the module header)
pub struct ArenaSet<K: Archive> {
    ///the blocks; ids are Vec indices, immortal (v0: no removal, no reuse)
    arena:   Vec<BlockSet<K, u8>>,
    ///min-key clone → block id, one entry per block; the map order IS
    ///the block order
    index:   BTreeMap<K, u32>,
    len:     usize,
    ///the last-mutated block — its deltas slot holds the insert's event
    touched: u32,
    ///the last cleave event, until `take_split`
    split:   Option<Split>,
}

impl<K: Archive + Clone + Ord> ArenaSet<K> {
    pub fn new() -> Self {
        ArenaSet { arena: Vec::new(), index: BTreeMap::new(), len: 0, touched: 0, split: None }
    }

    fn block(&self, aa: u32) -> &BlockSet<K, u8> {
        &self.arena[aa as usize]
    }

    fn block_mut(&mut self, aa: u32) -> &mut BlockSet<K, u8> {
        &mut self.arena[aa as usize]
    }

    fn next_aa(&self) -> u32 {
        self.arena.len() as u32
    }

    pub fn len(&self) -> usize {
        self.len
    }

    ///block count (bench/diagnostic)
    #[doc(hidden)]
    pub fn blocks(&self) -> usize {
        self.arena.len()
    }

    ///the last-mutated block — the insert's name swaps live in its slot
    pub fn touched(&self) -> u32 {
        self.touched
    }

    ///the touched block's event slot (apply + exhaust before the next
    ///insert into it). panics on a fresh arena — no insert has touched
    ///anything yet
    pub fn deltas(&mut self) -> &mut Deltas<u8> {
        self.arena
            .get_mut(self.touched as usize)
            .expect("deltas: no insert yet")
            .deltas()
    }

    ///the pending cleave event, if the last insert split mid-gap — take
    ///and apply before the next insert
    pub fn take_split(&mut self) -> Option<Split> {
        self.split.take()
    }

    ///k's full address: (block id, in-block name)
    pub fn find(&self, k: &K) -> Option<(u32, u8)> {
        //k below every min ⇒ k below the global min ⇒ absent
        let aa = self.index.range(..=k).next_back().map(|(_, &aa)| aa)?;
        self.block(aa).find(k).map(|ba| (aa, ba))
    }

    pub fn contains(&self, k: &K) -> bool {
        self.find(k).is_some()
    }

    ///the entry at a full address
    pub fn get(&self, addr: (u32, u8)) -> Option<K::View<'_>> {
        self.arena.get(addr.0 as usize)?.get(addr.1)
    }

    ///first→last over all blocks — the index order IS the block order
    pub fn iter(&self) -> impl Iterator<Item = K::View<'_>> + '_ {
        self.index.values().flat_map(|&aa| self.block(aa).iter())
    }

    ///Err returns k (dup). the consumer contract: drain the touched
    ///block's deltas and take the split event before the next insert
    pub fn insert(&mut self, k: K) -> Result<(u32, u8), K> {
        assert!(self.split.is_none(), "pending split event: take + apply before the next insert");
        //route: the floor entry owns k; below every min → the first
        //block (the only min-changing insert)
        let (aa, old_min) = match self.index.range(..=&k).next_back() {
            Some((_, &aa)) => (aa, None),
            None => match self.index.iter().next() {
                Some((min, &aa)) => (aa, Some((min.clone(), aa))),
                None => return self.spawn(k),
            },
        };
        let min_clone = old_min.as_ref().map(|_| k.clone());
        if old_min.is_some() {
            //the eager flip (see the module header): the per-block gate
            //is dead on split-born halves, so the arena owns the flip
            let b = self.block_mut(aa);
            if !b.reversed() && b.len() > 4 * FREE_STRIDE {
                b.reverse();
            }
        }
        match self.block_mut(aa).insert(k) {
            Ok(ba) => {
                if let Some((old_min, aa_min)) = old_min {
                    self.index.remove(&old_min);
                    self.index.insert(min_clone.unwrap(), aa_min);
                }
                self.touched = aa;
                self.len += 1;
                Ok((aa, ba))
            }
            Err(k) => {
                //a dup into a FULL block Errs at the capacity check before
                //the dup check — discriminate by lookup (the miss reads
                //as capacity, cleaves a block for a key it already has,
                //then Errs again on the re-route)
                if self.block(aa).len() >= BlockSet::<K, u8>::max_capacity()
                    && !self.block(aa).contains(&k)
                {
                    return self.split_route(aa, k);
                }
                Err(k) //dup
            }
        }
    }

    ///a fresh block holds k alone (empty arena, or a full block met k at
    ///its boundary — no cleave, no renames)
    fn spawn(&mut self, k: K) -> Result<(u32, u8), K> {
        let mut fresh = BlockSet::new();
        let min = k.clone();
        let ba = match fresh.insert(k) {
            Ok(ba) => ba,
            Err(_) => unreachable!("fresh block"),
        };
        let aa = self.arena.len() as u32;
        self.arena.push(fresh);
        self.index.insert(min, aa);
        self.touched = aa;
        self.len += 1;
        Ok((aa, ba))
    }

    ///a full block met k: beyond the tail extreme or below the min ⇒ a
    ///fresh block takes k alone (no renames — k never landed); a mid-gap
    ///k cleaves the block 50/50, records the split event, and re-routes
    ///into the owning half
    fn split_route(&mut self, aa: u32, k: K) -> Result<(u32, u8), K> {
        let (beyond, below) = {
            let b = self.block(aa);
            let below = b.min_key().is_some_and(|min| k < min);
            (b.beyond_tail(&k), below)
        };
        if beyond || below {
            return self.spawn(k);
        }
        //mid-gap: cleave the full block at its half, then re-route
        let at = self.block(aa).len() / 2;
        let from_tr = self.block(aa).translator();
        let right = self.block_mut(aa).cleave(at);
        let right_min = match right.min_key() {
            Some(min) => min,
            None => unreachable!("half of a full block"),
        };
        let to_aa = self.next_aa();
        //BOTH index entries update: the source's old min moved with the
        //right half — a stale source entry routes k into the wrong block
        //(and a colliding min key silently replaces it, orphaning the
        //source from routing entirely)
        let old_key = self
            .index
            .iter()
            .find_map(|(key, &a)| (a == aa).then(|| key.clone()))
            .expect("every block is indexed");
        self.index.remove(&old_key);
        let source_min = match self.block(aa).min_key() {
            Some(min) => min,
            None => unreachable!("half of a full block"),
        };
        self.index.insert(source_min, aa);
        self.arena.push(right);
        self.index.insert(right_min, to_aa);
        self.split = Some(Split { from_aa: aa, from_tr, to_aa, split_pos: at });
        //re-route k into the owning half — no dup (k wasn't in the
        //block) and both halves have room
        let aa2 = self
            .index
            .range(..=&k)
            .next_back()
            .or_else(|| self.index.iter().next())
            .map(|(_, &a)| a)
            .expect("index nonempty");
        let ba = match self.block_mut(aa2).insert(k) {
            Ok(ba) => ba,
            //k wasn't in the full block (the dup arm discriminates
            //before the cleave), and both halves sit below the ceiling
            Err(_) => unreachable!("half has room, no dup"),
        };
        self.touched = aa2;
        self.len += 1;
        Ok((aa2, ba))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    ///the holder mirror's full contract per insert: the split predicate
    ///(old names of the source block), then the touched block's own
    ///event, then every held name resolves to its key. returns whether a
    ///split fired
    fn apply_events(holder: &mut Vec<((u32, u8), u64)>, arena: &mut ArenaSet<u64>) -> bool {
        let mut split_fired = false;
        if let Some(s) = arena.take_split() {
            split_fired = true;
            for ((aa, ba), _) in &mut *holder {
                if *aa == s.from_aa {
                    let p = s.from_tr.a2p(*ba);
                    if p.0 >= s.split_pos {
                        //the name rides along unchanged — only the id
                        *aa = s.to_aa;
                    }
                }
            }
        }
        let touched = arena.touched();
        for (from, to) in arena.deltas().iter() {
            for ((aa, ba), _) in &mut *holder {
                if *aa == touched && *ba == from {
                    *ba = to;
                    break;
                }
            }
        }
        split_fired
    }

    fn assert_matches_model(arena: &ArenaSet<u64>, model: &BTreeSet<u64>) {
        assert_eq!(arena.len(), model.len());
        let max = model.last().copied().unwrap_or(0) + 1;
        for k in 0..=max {
            assert_eq!(arena.contains(&k), model.contains(&k), "k {k}");
            if let Some(addr) = arena.find(&k) {
                assert_eq!(arena.get(addr), Some(&k), "addr {k}");
            }
        }
        let got: Vec<u64> = arena.iter().copied().collect();
        let want: Vec<u64> = model.iter().copied().collect();
        assert_eq!(got, want, "iter order");
    }

    ///insert stream vs a BTreeSet model with a full holder mirror —
    ///every name verified after every insert, splits and block deltas
    ///applied exactly as a consumer would
    fn torture(keys: impl IntoIterator<Item = u64>) {
        let mut arena = ArenaSet::new();
        let mut model = BTreeSet::new();
        let mut holder: Vec<((u32, u8), u64)> = Vec::new();
        for k in keys {
            match arena.insert(k) {
                Ok(addr) => {
                    apply_events(&mut holder, &mut arena);
                    holder.push((addr, k));
                    model.insert(k);
                }
                Err(_) => {
                    apply_events(&mut holder, &mut arena);
                }
            }
            //every held name resolves to its key, after every insert
            for (i2, ((aa, ba), k)) in holder.iter().enumerate() {
                if arena.get((*aa, *ba)) != Some(k) {
                    println!(
                        "HOLDER MISS i={i2} k={k}: held ({aa},{ba}) resolves {:?}, find {:?}, blocks {}",
                        arena.get((*aa, *ba)).copied(),
                        arena.find(k).map(|(a, b)| (a, b)),
                        arena.blocks(),
                    );
                    panic!("holder name for {k}");
                }
            }
        }
        assert_matches_model(&arena, &model);
    }

    #[test]
    fn ascending_boundary_spawns() {
        let mut arena = ArenaSet::new();
        for k in 0..1000u64 {
            arena.insert(k).unwrap();
            assert!(arena.take_split().is_none(), "ascending spawns never cleave");
            arena.deltas().clear(); //no-op events pend too (the contract)
        }
        //4 blocks: three full (256), the last holds the remainder
        assert_eq!(arena.blocks(), 4);
        assert!(arena.iter().copied().eq(0..1000));
    }

    ///descending stream: below-min inserts flip fresh blocks (the z-gate
    ///past warmup) and full blocks spawn below — no cleaves either
    #[test]
    fn descending_spawns_below() {
        let mut arena = ArenaSet::new();
        for k in (0..1000u64).rev() {
            arena.insert(k).unwrap();
            assert!(arena.take_split().is_none(), "descending spawns never cleave");
            arena.deltas().clear(); //no-op events pend too (the contract)
        }
        assert_eq!(arena.blocks(), 4);
        assert!(arena.iter().copied().eq(0..1000));
    }

    #[test]
    fn matches_model_ascending() {
        torture(0..700);
    }

    #[test]
    fn matches_model_descending() {
        torture((0..700).rev());
    }

    #[test]
    fn matches_model_random() {
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        //random over a range with dups — the model tracks the accepted
        torture((0..1500u64).map(|_| rnd() % 1400));
    }

    ///mid-gap splits: a scattered stream with runs — full blocks cleave
    ///at least once, and the holder survives every predicate
    #[test]
    fn mid_cleaves_fire() {
        let mut arena = ArenaSet::new();
        let mut model = BTreeSet::new();
        let mut holder: Vec<((u32, u8), u64)> = Vec::new();
        let mut cleaves = 0;
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        for i in 0..900u64 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            //50-key chunks from a shuffled chunk order, half reversed —
            //fills blocks past 256 across scattered ranges
            let k = (i / 50) * 90 + s % 80;
            match arena.insert(k) {
                Ok(addr) => {
                    cleaves += apply_events(&mut holder, &mut arena) as usize;
                    holder.push((addr, k));
                    model.insert(k);
                }
                Err(_) => {
                    apply_events(&mut holder, &mut arena);
                }
            }
        }
        assert!(cleaves > 0, "scattered runs must cleave mid-gap");
        assert_matches_model(&arena, &model);
        for ((aa, ba), k) in &holder {
            assert_eq!(arena.get((*aa, *ba)), Some(k), "holder name for {k}");
        }
        assert!(arena.arena.iter().all(|b| b.len() <= 256), "block ceiling");
    }

    ///string keys: the cleave's heap rebuild round-trips archived keys
    ///(materialize + re-archive into both halves' fresh compact heaps —
    ///handles are heap-relative, so the copied slots' handles die with
    ///the old heap)
    #[test]
    fn string_cleave_rebuilds_heaps() {
        let mut arena = ArenaSet::<String>::new();
        let mut model = BTreeSet::new();
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..600 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            //long keys past the inline cutoff, scattered range
            let k = format!("key-{:04}-{}", s % 900, "x".repeat((s % 20) as usize));
            if arena.insert(k.clone()).is_ok() {
                model.insert(k);
            }
            arena.take_split();
            arena.deltas().clear();
        }
        assert_eq!(arena.len(), model.len());
        assert!(arena.blocks() > 1, "a cleave must have fired");
        for k in &model {
            assert!(arena.contains(k), "contains {k}");
        }
        let got: Vec<&str> = arena.iter().collect();
        let want: Vec<&str> = model.iter().map(|s| s.as_str()).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn duplicates_rejected() {
        let mut arena = ArenaSet::new();
        arena.insert(1u64).unwrap();
        assert!(arena.insert(1).is_err());
        assert_eq!(arena.len(), 1);
    }
}