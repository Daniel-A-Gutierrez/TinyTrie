    ///internal (non-leaf) split. Y is full and gaining a child `(sep_in, child_in)` from below.
    /// CLONE Y (Y stays intact & wired → tree walkable, `fixup_moved_run`'s `parent()` works via
    /// Y); split the clone into n1 (left)/n2 (right) + `sep`. place n1, n2 at their PRE-routing
    /// median gaps (before sep is inserted into Y) — Y's left/right-half children, reachable via the 
    /// intact Y (computing the anchor POST-routing could pick the incoming `child_in`, which isn't a Y child 
    /// and isn't reachable via Y's tree). 
    /// `insert_2` places both (only the root pinned; Y may move )
    /// insert_2 re-descends from the root between placements); 
    /// `child_in` rides as a floating handle. 
    /// THEN route `(sep_in, child_in)` into the placed owning half (logical `Node::insert`, no
    /// slide — both halves have room: DEGREE>=4 → 2 children each, +1 = 3 < DEGREE). 
    /// then `block.remove(Y)`, rewire. `path` = child indices root→Y (empty iff Y is the root);
    /// `root_h` = tree height (the driver captured it at descent start). returns None if Y was
    /// the root (new root promoted at the freed root vaddr, inv 2; height bumped), else
    /// `Some((sep, p2))` for the driver to insert into Y's parent (the p1-replacement
    /// `grandparent.children[y_idx] = p1` is done here, child-count-neutral).
    fn split_internal<'s>(
        &'s mut self,
        path: &[usize],
        root_h: TreeP<'a, T>,
        sep_in: TreeK<'a, T>,
        mut child_in: TreeP<'a, T>,
    ) -> Option<(TreeK<'a, T>, TreeP<'a, T>)>
    where 'a: 's {
        let root = self.root();
        //re-derive Y's current vaddr (Y may have moved in a prior split): re-descend root→Y.
        self.set_position(root);
        self.set_height(root_h);
        while self.pop().is_some() {}
        for &ci in path {
            self.descend(ci);
        }
        let y_v = self.position();
        // 1. clone Y (intact, wired); split clone → n1 (left), n2 (right), sep.
        let mut n1 = self.block().get(y_v).clone();
        let (n2, sep) = n1.split();
        // 2. PRE-routing median children (Y's left/right halves) — the insert_2 anchors.
        let n1_n = n1.children().len();
        let n2_n = n2.children().len();
        let mid1 = n1_n >> 1;
        let mid2 = n2_n >> 1;
        let child_idx1 = mid1 - 1;
        let child_idx2 = n1_n + mid2 - 1;
        // 3. place n1, n2. child_in (placed-but-unwired, from below) → floating handle.
        let mut child_in_opt = Some(child_in);
        let (p1, p2) = self.insert_2(path, child_idx1, n1, child_idx2, n2, &mut child_in_opt);
        child_in = child_in_opt.expect("split_internal: child_in handle vanished");
        // 4. route (sep_in, child_in) into the owning half AFTER placement (logical; no slide).
        let half_v = if sep_in < sep { p1 } else { p2 };
        let ovf = self.block_mut().get_mut(half_v).insert(sep_in, Payload::Child(child_in));
        debug_assert!(ovf.is_none(), "split_internal: half overflowed (DEGREE>=4 guarantees room)");
        // 5. re-derive Y (may have moved during insert_2's slides), free it (block primitive).
        self.set_position(root);
        self.set_height(root_h);
        while self.pop().is_some() {}
        for &ci in path {
            self.descend(ci);
        }
        let y_cur = self.position();
        let _removed = self.block_mut().remove(y_cur);
        // 6. rewire.
        if path.is_empty() {
            // Y was the root (pinned → didn't move; y_cur == root). new root adopts the root
            // vaddr (inv 2): placed at the freed root slot.
            let mut new_root: TreeT<'a, T> = Default::default();
            let _ = new_root.insert(sep, Payload::Child(p2));   // keys[0]=sep, leaves[1]=p2, nchildren=2
            new_root.update_child(0, p1);                        // leaves[0]=p1
            let phys = self.block().v2p(root);
            let new_root_v = self.block_mut().insert(new_root, crate::block::OpenSlot(phys));
            debug_assert_eq!(new_root_v, root, "split_internal: new root not at root vaddr");
            self.bump_height();
            None
        } else {
            // non-root: grandparent.children[y_idx] = p1 (replace Y). re-derive grandparent.
            self.set_position(root);
            self.set_height(root_h);
            while self.pop().is_some() {}
            for &ci in &path[..path.len() - 1] {
                self.descend(ci);
            }
            let grandparent_v = self.position();
            let y_idx = path[path.len() - 1];
            self.block_mut().get_mut(grandparent_v).update_child(y_idx, p1);
            Some((sep, p2))
        }
    }