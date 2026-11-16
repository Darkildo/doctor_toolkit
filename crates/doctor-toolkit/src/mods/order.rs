//! Rules of the load order (format version 3) and sorting by them.
//!
//! A mod that depends on another must come after it: a **hard** rule. The
//! `load_after` and `load_before` lists of a mod are **soft** rules, as in
//! LOOT: they are kept when they can be, and a rule naming a mod that is not
//! installed is ignored. Sorting is a stable topological sort: among the mods
//! whose rules allow it, the one that is earlier now goes first, so the order
//! set by hand stays wherever the rules do not change it. A cycle of hard
//! rules is an error; a soft rule that would close a cycle is dropped, and
//! the sort says which one.

use std::collections::{BTreeSet, HashMap};

use super::ModManifest;

/// Whether a rule must be kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Strength {
    /// `depends`: the dependency must be loaded first.
    Hard,
    /// `load_after` / `load_before`.
    Soft,
}

/// `first` must be loaded before `then`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub first: String,
    pub then: String,
    pub strength: Strength,
    /// The mod whose manifest has the rule.
    pub declared_by: String,
}

impl Rule {
    /// The rule in words.
    pub fn describe(&self) -> String {
        match self.strength {
            Strength::Hard => format!(
                "`{}` needs `{}`, which must be loaded before it",
                self.then, self.first
            ),
            Strength::Soft if self.declared_by == self.then => {
                format!("`{}` asks to be loaded after `{}`", self.then, self.first)
            }
            Strength::Soft => format!("`{}` asks to be loaded before `{}`", self.first, self.then),
        }
    }
}

/// The rules among the installed mods: `mods` in load order, as (id, manifest).
pub fn rules(mods: &[(&str, &ModManifest)]) -> Vec<Rule> {
    let installed: BTreeSet<&str> = mods.iter().map(|(id, _)| *id).collect();
    let mut out = Vec::new();
    let mut push = |first: &str, then: &str, strength, by: &str| {
        if first != then && installed.contains(first) && installed.contains(then) {
            out.push(Rule {
                first: first.to_owned(),
                then: then.to_owned(),
                strength,
                declared_by: by.to_owned(),
            });
        }
    };
    for (id, m) in mods {
        for dep in m.depends.keys() {
            push(dep, id, Strength::Hard, id);
        }
    }
    for (id, m) in mods {
        for after in &m.load_after {
            push(after, id, Strength::Soft, id);
        }
        for before in &m.load_before {
            push(id, before, Strength::Soft, id);
        }
    }
    out
}

/// Result of [`sort`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sorted {
    /// Ids in the new load order.
    pub order: Vec<String>,
    /// Soft rules that could not be kept: each would close a cycle.
    pub dropped: Vec<Rule>,
}

/// Sorts `ids` (the current load order) by `rules`, keeping the current
/// order wherever the rules allow. A cycle of hard rules is an error that
/// names its mods.
pub fn sort(ids: &[&str], rules: &[Rule]) -> Result<Sorted, Vec<String>> {
    let index: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let n = ids.len();
    let mut after: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    let edge = |rule: &Rule| {
        Some((
            *index.get(rule.first.as_str())?,
            *index.get(rule.then.as_str())?,
        ))
    };
    for rule in rules.iter().filter(|r| r.strength == Strength::Hard) {
        if let Some((a, b)) = edge(rule) {
            after[a].insert(b);
        }
    }
    if let Some(cycle) = find_cycle(&after) {
        return Err(cycle.into_iter().map(|i| ids[i].to_owned()).collect());
    }
    let mut dropped = Vec::new();
    for rule in rules.iter().filter(|r| r.strength == Strength::Soft) {
        let Some((a, b)) = edge(rule) else { continue };
        if reaches(&after, b, a) {
            dropped.push(rule.clone());
        } else {
            after[a].insert(b);
        }
    }
    let mut incoming = vec![0usize; n];
    for list in &after {
        for &b in list {
            incoming[b] += 1;
        }
    }
    let mut ready: BTreeSet<usize> = (0..n).filter(|&i| incoming[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(i) = ready.pop_first() {
        order.push(ids[i].to_owned());
        for &b in &after[i] {
            incoming[b] -= 1;
            if incoming[b] == 0 {
                ready.insert(b);
            }
        }
    }
    debug_assert_eq!(order.len(), n, "no cycle is left");
    Ok(Sorted { order, dropped })
}

/// Whether `to` can be reached from `from`.
fn reaches(after: &[BTreeSet<usize>], from: usize, to: usize) -> bool {
    let mut seen = vec![false; after.len()];
    let mut stack = vec![from];
    while let Some(i) = stack.pop() {
        if i == to {
            return true;
        }
        if !std::mem::replace(&mut seen[i], true) {
            stack.extend(after[i].iter().copied());
        }
    }
    false
}

/// A cycle of the graph, if there is one, in order.
fn find_cycle(after: &[BTreeSet<usize>]) -> Option<Vec<usize>> {
    // 0: not seen, 1: on the path, 2: done.
    let mut mark = vec![0u8; after.len()];
    for start in 0..after.len() {
        if mark[start] != 0 {
            continue;
        }
        let mut path = vec![start];
        let mut iters = vec![after[start].iter()];
        mark[start] = 1;
        while let Some(it) = iters.last_mut() {
            match it.next() {
                Some(&next) if mark[next] == 1 => {
                    let at = path.iter().position(|&p| p == next).unwrap_or(0);
                    return Some(path[at..].to_vec());
                }
                Some(&next) if mark[next] == 0 => {
                    mark[next] = 1;
                    path.push(next);
                    iters.push(after[next].iter());
                }
                Some(_) => {}
                None => {
                    iters.pop();
                    if let Some(done) = path.pop() {
                        mark[done] = 2;
                    }
                }
            }
        }
    }
    None
}

/// The rules that moving `id` to `position` (in `ids` without it, as
/// [`super::ModManager::move_mod`] counts) would break.
pub fn broken_by_move<'a>(
    ids: &[&str],
    rules: &'a [Rule],
    id: &str,
    position: usize,
) -> Vec<&'a Rule> {
    let mut order: Vec<&str> = ids.iter().copied().filter(|x| *x != id).collect();
    order.insert(position.min(order.len()), id);
    broken(&order, rules)
        .into_iter()
        .filter(|r| r.first == id || r.then == id)
        .collect()
}

/// The rules `order` does not keep.
pub fn broken<'a>(order: &[&str], rules: &'a [Rule]) -> Vec<&'a Rule> {
    let at: HashMap<&str, usize> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    rules
        .iter()
        .filter(
            |r| match (at.get(r.first.as_str()), at.get(r.then.as_str())) {
                (Some(a), Some(b)) => a > b,
                _ => false,
            },
        )
        .collect()
}

/// The nearest place to its current one where `id` keeps its rules with the
/// other mods (hard ones first): after every mod it must follow, before every
/// mod it must precede. `None` when it is where it can be, or when no place
/// keeps them (sorting tells why).
pub fn place(ids: &[&str], rules: &[Rule], id: &str) -> Option<usize> {
    let current = ids.iter().position(|x| *x == id)?;
    let others: Vec<&str> = ids.iter().copied().filter(|x| *x != id).collect();
    let at: HashMap<&str, usize> = others.iter().enumerate().map(|(i, x)| (*x, i)).collect();
    // Places in `others` are gaps 0..=len.
    let range = |strengths: &[Strength]| {
        let mut low = 0usize;
        let mut high = others.len();
        for rule in rules.iter().filter(|r| strengths.contains(&r.strength)) {
            if rule.then == id
                && let Some(&a) = at.get(rule.first.as_str())
            {
                low = low.max(a + 1);
            }
            if rule.first == id
                && let Some(&b) = at.get(rule.then.as_str())
            {
                high = high.min(b);
            }
        }
        (low <= high).then_some((low, high))
    };
    let (low, high) =
        range(&[Strength::Hard, Strength::Soft]).or_else(|| range(&[Strength::Hard]))?;
    let target = current.clamp(low, high);
    (target != current).then_some(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hard(first: &str, then: &str) -> Rule {
        Rule {
            first: first.into(),
            then: then.into(),
            strength: Strength::Hard,
            declared_by: then.into(),
        }
    }

    fn soft(first: &str, then: &str) -> Rule {
        Rule {
            first: first.into(),
            then: then.into(),
            strength: Strength::Soft,
            declared_by: then.into(),
        }
    }

    #[test]
    fn sorting_keeps_the_order_the_rules_allow() {
        let ids = ["e", "a", "b", "c", "d"];
        let sorted = sort(&ids, &[]).unwrap();
        assert_eq!(sorted.order, ids);
        // `a` needs `d`: `d` comes up just before `a`... and the rest stays.
        let sorted = sort(&ids, &[hard("d", "a")]).unwrap();
        assert_eq!(sorted.order, ["e", "b", "c", "d", "a"]);
        // An already kept rule moves nothing.
        let sorted = sort(&ids, &[hard("e", "c"), soft("b", "d")]).unwrap();
        assert_eq!(sorted.order, ids);
        // Sorting twice gives the same.
        let rules = [hard("d", "a"), soft("c", "e")];
        let once = sort(&ids, &rules).unwrap().order;
        let refs: Vec<&str> = once.iter().map(String::as_str).collect();
        assert_eq!(sort(&refs, &rules).unwrap().order, once);
        assert!(broken(&refs, &rules).is_empty());
    }

    #[test]
    fn cycles_of_hard_rules_are_errors_and_soft_ones_are_dropped() {
        let ids = ["a", "b", "c"];
        let error = sort(&ids, &[hard("a", "b"), hard("b", "c"), hard("c", "a")]).unwrap_err();
        let mut names = error.clone();
        names.sort();
        assert_eq!(names, ["a", "b", "c"]);
        // A soft rule against a hard one is dropped, with the rule told.
        let sorted = sort(&ids, &[hard("a", "b"), soft("b", "a")]).unwrap();
        assert_eq!(sorted.order, ["a", "b", "c"]);
        assert_eq!(sorted.dropped, [soft("b", "a")]);
        assert!(sorted.dropped[0].describe().contains("after"));
        // Soft rules in a cycle: the last one that closes it goes.
        let sorted = sort(&ids, &[soft("c", "b"), soft("b", "a"), soft("a", "c")]).unwrap();
        assert_eq!(sorted.order, ["c", "b", "a"]);
        assert_eq!(sorted.dropped, [soft("a", "c")]);
    }

    #[test]
    fn moves_against_rules_and_the_nearest_place() {
        let ids = ["a", "b", "c", "d"];
        let rules = [hard("b", "d"), soft("a", "c")];
        assert!(broken_by_move(&ids, &rules, "d", 3).is_empty());
        let broken = broken_by_move(&ids, &rules, "d", 0);
        assert_eq!(broken, [&rules[0]]);
        assert!(broken[0].describe().contains("needs `b`"));
        assert_eq!(broken_by_move(&ids, &rules, "c", 0), [&rules[1]]);
        // Unrelated mods move freely.
        assert!(broken_by_move(&ids, &rules, "a", 1).is_empty());

        assert_eq!(place(&ids, &rules, "d"), None);
        let order = ["d", "a", "b", "c"];
        assert_eq!(place(&order, &rules, "d"), Some(2), "just after b");
        let order = ["a", "c", "b", "d"];
        assert_eq!(
            place(&order, &[hard("c", "b"), soft("b", "a")], "a"),
            Some(2)
        );
        // Soft and hard rules that cannot both hold: hard ones decide.
        let order = ["b", "a"];
        assert_eq!(place(&order, &[hard("b", "a"), soft("a", "b")], "a"), None);
        let order = ["a", "b"];
        assert_eq!(
            place(&order, &[hard("b", "a"), soft("a", "b")], "a"),
            Some(1)
        );
    }
}
