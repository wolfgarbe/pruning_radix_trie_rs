//! # Pruning Radix Trie
//!
//! A radix trie for top-k prefix completion over a `term -> frequency` dictionary.
//! Every node stores the maximum frequency found in its subtree, and children are
//! kept in descending order of their bound (own count or subtree maximum). During
//! lookup, whole branches are skipped (pruned) as soon as they cannot beat the
//! current k-th best result. The trie is mutable: terms can be added at any time
//! and queried immediately.
//!
//! ```
//! use pruning_radix_trie::PruningRadixTrie;
//!
//! let mut trie = PruningRadixTrie::new();
//! trie.add_term("the", 100);
//! trie.add_term("their", 70);
//! trie.add_term("there", 50);
//!
//! let (top, prefix_count) = trie.get_top_k_terms_for_prefix("the", 2, true);
//! assert_eq!(top, vec![("the".to_string(), 100), ("their".to_string(), 70)]);
//! assert_eq!(prefix_count, 100);
//! ```

use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;

/// Trie node. A key (edge label) lives in the parent's `children` vector.
#[derive(Debug, Default)]
struct Node {
    /// Children sorted descending by `bound()` (their own count or subtree max,
    /// whichever is larger), so the first child that cannot beat the current
    /// k-th result proves that all its later siblings cannot either.
    children: Vec<(String, Node)>,
    /// First char of each child's key, parallel to `children`. Sibling keys never
    /// share a first char, so prefix descent scans this small contiguous array
    /// instead of dereferencing every sibling's key string.
    first_chars: Vec<char>,
    /// 0: not a word; >0: is a word (its term frequency count).
    term_frequency_count: u64,
    /// Maximum term frequency count among all descendants.
    term_frequency_count_child_max: u64,
}

impl Node {
    fn new(term_frequency_count: u64) -> Self {
        Node {
            children: Vec::new(),
            first_chars: Vec::new(),
            term_frequency_count,
            term_frequency_count_child_max: 0,
        }
    }

    /// Upper bound for any term in this node or below.
    #[inline]
    fn bound(&self) -> u64 {
        self.term_frequency_count
            .max(self.term_frequency_count_child_max)
    }

    /// Index of the child whose key starts with `first`, if any.
    #[inline]
    fn find_child(&self, first: char) -> Option<usize> {
        self.first_chars.iter().position(|&c| c == first)
    }

    /// Sort descending by inclusive bound so lookup starts with the most promising
    /// branch and can stop at the first branch that is pruned; then resync
    /// `first_chars`.
    fn sort_children(&mut self) {
        self.children.sort_by(|a, b| b.1.bound().cmp(&a.1.bound()));
        self.first_chars.clear();
        self.first_chars
            .extend(self.children.iter().map(|(k, _)| first_char(k)));
    }

    fn set_children(&mut self, children: Vec<(String, Node)>) {
        self.children = children;
        self.sort_children();
    }
}

#[inline]
fn first_char(s: &str) -> char {
    s.chars().next().expect("keys are never empty")
}

/// Length in bytes of the longest common prefix (always on a char boundary).
fn common_prefix_len(a: &str, b: &str) -> usize {
    let mut n = 0;
    for (x, y) in a.chars().zip(b.chars()) {
        if x == y {
            n += x.len_utf8();
        } else {
            break;
        }
    }
    n
}

/// Pruning radix trie for top-k prefix completion.
#[derive(Debug, Default)]
pub struct PruningRadixTrie {
    term_count: u64,
    term_count_loaded: u64,
    trie: Node,
}

impl PruningRadixTrie {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct terms in the trie.
    pub fn term_count(&self) -> u64 {
        self.term_count
    }

    /// Insert a term. If it already exists, the counts are summed.
    /// Empty terms are ignored.
    pub fn add_term(&mut self, term: &str, term_frequency_count: u64) {
        if term.is_empty() {
            return;
        }
        Self::insert(&mut self.trie, term, term_frequency_count, &mut self.term_count);
    }

    /// Returns the value that has to be propagated into the subtree maxima of
    /// `curr` and all its ancestors. `term` is never empty.
    fn insert(curr: &mut Node, term: &str, count: u64, term_count: &mut u64) -> u64 {
        if let Some(j) = curr.find_child(first_char(term)) {
            let common = common_prefix_len(term, &curr.children[j].0);
            debug_assert!(common > 0);
            let key_len = curr.children[j].0.len();

            let propagate = if common == term.len() && common == key_len {
                // term already existed
                // existing ab / new ab
                let node = &mut curr.children[j].1;
                let bound_before = node.bound();
                if node.term_frequency_count == 0 {
                    *term_count += 1;
                }
                node.term_frequency_count = node.term_frequency_count.saturating_add(count);
                let new_count = node.term_frequency_count;
                if node.bound() != bound_before {
                    curr.sort_children();
                }
                new_count
            } else if common == term.len() {
                // new is a proper prefix of the existing key
                // existing abcd / new ab
                let (old_key, old_node) =
                    std::mem::replace(&mut curr.children[j], (String::new(), Node::new(0)));
                let mut child = Node::new(count);
                child.term_frequency_count_child_max = old_node
                    .term_frequency_count_child_max
                    .max(old_node.term_frequency_count);
                child.set_children(vec![(old_key[common..].to_string(), old_node)]);
                curr.children[j] = (term[..common].to_string(), child);
                curr.sort_children();
                *term_count += 1;
                count
            } else if common == key_len {
                // existing key is a proper prefix of new: descend
                // existing te / new test
                let bound_before = curr.children[j].1.bound();
                let propagate =
                    Self::insert(&mut curr.children[j].1, &term[common..], count, term_count);
                // The child's bound may have grown: restore descending order
                // so lookup keeps starting with the most promising branch.
                if curr.children[j].1.bound() != bound_before {
                    curr.sort_children();
                }
                propagate
            } else {
                // common prefix, different suffixes: split
                // existing test / new team
                let (old_key, old_node) =
                    std::mem::replace(&mut curr.children[j], (String::new(), Node::new(0)));
                let mut child = Node::new(0);
                child.term_frequency_count_child_max = old_node
                    .term_frequency_count_child_max
                    .max(count)
                    .max(old_node.term_frequency_count);
                child.set_children(vec![
                    (old_key[common..].to_string(), old_node),
                    (term[common..].to_string(), Node::new(count)),
                ]);
                curr.children[j] = (term[..common].to_string(), child);
                curr.sort_children();
                *term_count += 1;
                count
            };

            curr.term_frequency_count_child_max =
                curr.term_frequency_count_child_max.max(propagate);
            return propagate;
        }

        // no shared prefix with any child: new child
        curr.children.push((term.to_string(), Node::new(count)));
        curr.sort_children();
        *term_count += 1;
        curr.term_frequency_count_child_max = curr.term_frequency_count_child_max.max(count);
        count
    }

    /// Returns up to `top_k` completions of `prefix`, ordered by descending
    /// frequency, plus the frequency of `prefix` itself if it is a term in the
    /// dictionary (0 otherwise, even if it didn't make the top-k).
    ///
    /// `top_k == 0` returns *all* completions (unordered, no pruning).
    /// `pruning == false` disables early termination (useful for benchmarking).
    pub fn get_top_k_terms_for_prefix(
        &self,
        prefix: &str,
        top_k: usize,
        pruning: bool,
    ) -> (Vec<(String, u64)>, u64) {
        let mut search = Search {
            top_k,
            pruning,
            prefix_count: 0,
            buf: String::new(),
            results: Vec::with_capacity(top_k.min(4096)),
        };
        search.visit(&self.trie, prefix);
        (search.results, search.prefix_count)
    }

    /// Write all terms as `term\tcount` lines. Skipped if nothing was added
    /// since the last `read_terms_from_file`.
    pub fn write_terms_to_file<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        if self.term_count_loaded == self.term_count {
            return Ok(());
        }
        let mut w = BufWriter::new(File::create(path)?);
        let mut buf = String::new();
        Self::write_node(&self.trie, &mut buf, &mut w)?;
        w.flush()
    }

    fn write_node<W: Write>(node: &Node, buf: &mut String, w: &mut W) -> io::Result<()> {
        for (key, child) in &node.children {
            let len = buf.len();
            buf.push_str(key);
            if child.term_frequency_count > 0 {
                writeln!(w, "{}\t{}", buf, child.term_frequency_count)?;
            }
            Self::write_node(child, buf, w)?;
            buf.truncate(len);
        }
        Ok(())
    }

    /// Load `term\tcount` lines (malformed lines are skipped).
    pub fn read_terms_from_file<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        let reader = BufReader::new(File::open(path)?);
        for line in reader.lines() {
            let line = line?;
            let mut parts = line.split('\t');
            if let (Some(term), Some(count), None) = (parts.next(), parts.next(), parts.next()) {
                if let Ok(count) = count.trim().parse::<u64>() {
                    self.add_term(term, count);
                }
            }
        }
        self.term_count_loaded = self.term_count;
        Ok(())
    }
}

/// State of one top-k lookup.
struct Search {
    top_k: usize,
    pruning: bool,
    prefix_count: u64,
    /// Term being assembled while walking down the trie.
    buf: String,
    /// Descending by count.
    results: Vec<(String, u64)>,
}

impl Search {
    /// True if nothing with this bound can enter the (full) top-k list.
    #[inline]
    fn pruned(&self, bound: u64) -> bool {
        self.pruning
            && self.top_k > 0
            && self.results.len() == self.top_k
            && bound <= self.results[self.top_k - 1].1
    }

    /// Visit the children of `curr` that match the remaining `prefix`
    /// (all children if it is empty).
    fn visit(&mut self, curr: &Node, prefix: &str) {
        // pruning/early termination in radix trie lookup
        if self.pruned(curr.term_frequency_count_child_max) {
            return;
        }

        if prefix.is_empty() {
            for (key, node) in &curr.children {
                // Children are sorted descending by bound, so once one child cannot
                // beat the k-th result, none of its later siblings can either.
                if self.pruned(node.bound()) {
                    break;
                }
                self.take(key, node, false);
            }
            return;
        }

        // At most one child can match: sibling keys have distinct first chars.
        let Some(j) = curr.find_child(first_char(prefix)) else {
            return;
        };
        let (key, node) = &curr.children[j];
        if self.pruned(node.bound()) {
            return;
        }
        if key.starts_with(prefix) {
            self.take(key, node, prefix == key.as_str());
        } else if prefix.starts_with(key.as_str()) && !node.children.is_empty() {
            let len = self.buf.len();
            self.buf.push_str(key);
            self.visit(node, &prefix[key.len()..]);
            self.buf.truncate(len);
        }
    }

    /// `key` fully matches the prefix: record the term and visit its subtree.
    fn take(&mut self, key: &str, node: &Node, is_exact_prefix: bool) {
        let len = self.buf.len();
        self.buf.push_str(key);

        if node.term_frequency_count > 0 {
            if is_exact_prefix {
                self.prefix_count = node.term_frequency_count;
            }
            // candidate
            if self.top_k > 0 {
                Self::add_top_k_suggestion(
                    &self.buf,
                    node.term_frequency_count,
                    self.top_k,
                    &mut self.results,
                );
            } else {
                self.results.push((self.buf.clone(), node.term_frequency_count));
            }
        }

        if !node.children.is_empty() {
            self.visit(node, "");
        }
        self.buf.truncate(len);
    }

    /// Insert into the descending-sorted top-k list. On equal counts the new
    /// term is placed before the existing ones.
    fn add_top_k_suggestion(
        term: &str,
        count: u64,
        top_k: usize,
        results: &mut Vec<(String, u64)>,
    ) {
        if results.len() < top_k {
            let index = results.partition_point(|p| p.1 > count);
            results.insert(index, (term.to_string(), count));
        } else if count >= results[top_k - 1].1 {
            // Full: recycle the evicted entry's String instead of allocating.
            let (mut recycled, _) = results.pop().expect("top_k > 0");
            recycled.clear();
            recycled.push_str(term);
            let index = results.partition_point(|p| p.1 > count);
            results.insert(index, (recycled, count));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn t(s: &str, c: u64) -> (String, u64) {
        (s.to_string(), c)
    }

    #[test]
    fn basic_topk() {
        let mut trie = PruningRadixTrie::new();
        for (w, c) in [("the", 100), ("their", 70), ("there", 50), ("then", 10), ("test", 5), ("team", 8)] {
            trie.add_term(w, c);
        }
        assert_eq!(trie.term_count(), 6);
        let (r, p) = trie.get_top_k_terms_for_prefix("the", 2, true);
        assert_eq!(r, vec![t("the", 100), t("their", 70)]);
        assert_eq!(p, 100);
        let (r, p) = trie.get_top_k_terms_for_prefix("te", 10, true);
        assert_eq!(r, vec![t("team", 8), t("test", 5)]);
        assert_eq!(p, 0);
        let (r, _) = trie.get_top_k_terms_for_prefix("x", 5, true);
        assert!(r.is_empty());
        // first char matches but the rest doesn't
        let (r, _) = trie.get_top_k_terms_for_prefix("thx", 5, true);
        assert!(r.is_empty());
    }

    #[test]
    fn duplicate_terms_sum() {
        let mut trie = PruningRadixTrie::new();
        trie.add_term("abc", 1);
        trie.add_term("abc", 4);
        assert_eq!(trie.term_count(), 1);
        assert_eq!(trie.get_top_k_terms_for_prefix("ab", 5, true).0, vec![t("abc", 5)]);
    }

    #[test]
    fn incremental_add_and_query() {
        let mut trie = PruningRadixTrie::new();
        trie.add_term("apple", 5);
        assert_eq!(trie.get_top_k_terms_for_prefix("ap", 3, true).0, vec![t("apple", 5)]);
        trie.add_term("apply", 9);
        assert_eq!(
            trie.get_top_k_terms_for_prefix("ap", 3, true).0,
            vec![t("apply", 9), t("apple", 5)]
        );
        trie.add_term("apple", 10);
        assert_eq!(
            trie.get_top_k_terms_for_prefix("ap", 3, true).0,
            vec![t("apple", 15), t("apply", 9)]
        );
    }

    #[test]
    fn unicode_split_on_char_boundary() {
        let mut trie = PruningRadixTrie::new();
        trie.add_term("straße", 3);
        trie.add_term("strasse", 2);
        trie.add_term("日本語", 5);
        trie.add_term("日本酒", 4);
        assert_eq!(trie.get_top_k_terms_for_prefix("日本", 5, true).0, vec![t("日本語", 5), t("日本酒", 4)]);
        assert_eq!(trie.get_top_k_terms_for_prefix("stra", 5, true).0.len(), 2);
    }

    #[test]
    fn siblings_sharing_first_utf8_byte() {
        // 'é' (C3 A9) and 'ê' (C3 AA) share their first UTF-8 byte but are distinct chars
        let mut trie = PruningRadixTrie::new();
        trie.add_term("éa", 5);
        trie.add_term("êa", 3);
        trie.add_term("éb", 4);
        assert_eq!(trie.get_top_k_terms_for_prefix("é", 5, true).0, vec![t("éa", 5), t("éb", 4)]);
        assert_eq!(trie.get_top_k_terms_for_prefix("ê", 5, true).0, vec![t("êa", 3)]);
    }

    fn brute_force_check(alphabet: &[char], seed: u64) {
        let mut state = seed;
        let mut next = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            state >> 33
        };
        let mut trie = PruningRadixTrie::new();
        let mut map: HashMap<String, u64> = HashMap::new();
        for _ in 0..3000 {
            let len = 1 + (next() % 6) as usize;
            let w: String = (0..len)
                .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                .collect();
            let c = 1 + next() % 1000;
            trie.add_term(&w, c);
            *map.entry(w).or_insert(0) += c;
        }
        assert_eq!(trie.term_count() as usize, map.len());

        let mut prefixes = vec![String::new(), "zzz".to_string()];
        for &a in alphabet {
            prefixes.push(a.to_string());
            for &b in alphabet {
                prefixes.push(format!("{a}{b}"));
                prefixes.push(format!("{a}{b}{a}"));
            }
        }
        for prefix in &prefixes {
            for k in [1usize, 5, 20] {
                let mut expected: Vec<u64> = map
                    .iter()
                    .filter(|(w, _)| w.starts_with(prefix.as_str()))
                    .map(|(_, c)| *c)
                    .collect();
                expected.sort_by(|a, b| b.cmp(a));
                expected.truncate(k);
                for pruning in [true, false] {
                    let (res, pc) = trie.get_top_k_terms_for_prefix(prefix, k, pruning);
                    let got: Vec<u64> = res.iter().map(|r| r.1).collect();
                    assert_eq!(got, expected, "prefix={prefix:?} k={k} pruning={pruning}");
                    assert_eq!(pc, map.get(prefix).copied().unwrap_or(0));
                }
            }
            // top_k == 0 returns everything
            assert_eq!(
                trie.get_top_k_terms_for_prefix(prefix, 0, true).0.len(),
                map.keys().filter(|w| w.starts_with(prefix.as_str())).count()
            );
        }
    }

    #[test]
    fn matches_brute_force_ascii() {
        brute_force_check(&['a', 'b', 'c'], 12345);
    }

    #[test]
    fn matches_brute_force_unicode_siblings() {
        brute_force_check(&['é', 'ê', 'e', '日'], 777);
    }

    fn assert_invariants(node: &Node) -> u64 {
        let firsts: Vec<char> = node.children.iter().map(|(k, _)| first_char(k)).collect();
        assert_eq!(node.first_chars, firsts, "first_chars out of sync");
        let mut uniq = firsts.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), firsts.len(), "sibling keys share a first char");
        for w in node.children.windows(2) {
            assert!(w[0].1.bound() >= w[1].1.bound(), "children not sorted descending by bound");
        }
        let mut subtree_max = 0;
        for (_, child) in &node.children {
            let below = assert_invariants(child);
            assert_eq!(child.term_frequency_count_child_max, below);
            subtree_max = subtree_max.max(below).max(child.term_frequency_count);
        }
        subtree_max
    }

    #[test]
    fn invariants_hold_after_every_insert() {
        let mut state = 99u64;
        let mut next = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            state >> 33
        };
        let alphabet = ['a', 'b', 'é', 'ê'];
        let mut trie = PruningRadixTrie::new();
        for _ in 0..3000 {
            let len = 1 + (next() % 7) as usize;
            let w: String = (0..len).map(|_| alphabet[(next() % 4) as usize]).collect();
            trie.add_term(&w, 1 + next() % 1000);
            assert_invariants(&trie.trie);
        }
    }

    #[test]
    fn file_roundtrip() {
        let path = std::env::temp_dir().join("pruning_radix_trie_test.txt");
        let mut a = PruningRadixTrie::new();
        a.add_term("hello", 10);
        a.add_term("help", 7);
        a.add_term("world", 3);
        a.write_terms_to_file(&path).unwrap();

        let mut b = PruningRadixTrie::new();
        b.read_terms_from_file(&path).unwrap();
        assert_eq!(b.term_count(), 3);
        assert_eq!(b.get_top_k_terms_for_prefix("hel", 5, true).0, vec![t("hello", 10), t("help", 7)]);
        let _ = std::fs::remove_file(path);
    }
}
