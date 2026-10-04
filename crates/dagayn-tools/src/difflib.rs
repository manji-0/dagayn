//! Python's `difflib.unified_diff` over lines, with `SequenceMatcher(None,
//! a, b)` (no junk, `autojunk=True`) as CPython 3.14 implements it, so a
//! dry-run diff is byte-for-byte the one Python prints.

use std::collections::HashMap;

type Opcode = (&'static str, usize, usize, usize, usize);

struct Matcher<'a> {
    a: &'a [String],
    b: &'a [String],
    /// `b2j` without the popular elements.
    b2j: HashMap<&'a str, Vec<usize>>,
}

impl<'a> Matcher<'a> {
    /// `__chain_b`.
    fn new(a: &'a [String], b: &'a [String]) -> Self {
        let mut b2j: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, line) in b.iter().enumerate() {
            b2j.entry(line.as_str()).or_default().push(index);
        }
        if b.len() >= 200 {
            let ntest = b.len() / 100 + 1;
            b2j.retain(|_, indices| indices.len() <= ntest);
        }
        Self { a, b, b2j }
    }

    /// `find_longest_match`; with no junk, only the non-junk extension runs.
    fn find_longest_match(
        &self,
        alo: usize,
        ahi: usize,
        blo: usize,
        bhi: usize,
    ) -> (usize, usize, usize) {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for i in alo..ahi {
            let mut next: HashMap<usize, usize> = HashMap::new();
            if let Some(indices) = self.b2j.get(self.a[i].as_str()) {
                for &j in indices {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|prev| j2len.get(&prev))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    next.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = next;
        }
        while besti > alo && bestj > blo && self.a[besti - 1] == self.b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && self.a[besti + bestsize] == self.b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    fn matching_blocks(&self) -> Vec<(usize, usize, usize)> {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0, la, 0, lb)];
        let mut blocks = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks.sort_unstable();
        let (mut i1, mut j1, mut k1) = (0, 0, 0);
        let mut merged = Vec::new();
        for (i2, j2, k2) in blocks {
            if i1 + k1 == i2 && j1 + k1 == j2 {
                k1 += k2;
            } else {
                if k1 > 0 {
                    merged.push((i1, j1, k1));
                }
                (i1, j1, k1) = (i2, j2, k2);
            }
        }
        if k1 > 0 {
            merged.push((i1, j1, k1));
        }
        merged.push((la, lb, 0));
        merged
    }

    fn opcodes(&self) -> Vec<Opcode> {
        let (mut i, mut j) = (0, 0);
        let mut out = Vec::new();
        for (ai, bj, size) in self.matching_blocks() {
            let tag = if i < ai && j < bj {
                "replace"
            } else if i < ai {
                "delete"
            } else if j < bj {
                "insert"
            } else {
                ""
            };
            if !tag.is_empty() {
                out.push((tag, i, ai, j, bj));
            }
            i = ai + size;
            j = bj + size;
            if size > 0 {
                out.push(("equal", ai, i, bj, j));
            }
        }
        out
    }

    fn grouped_opcodes(&self, n: usize) -> Vec<Vec<Opcode>> {
        let mut codes = self.opcodes();
        if codes.is_empty() {
            codes.push(("equal", 0, 1, 0, 1));
        }
        if let Some(first) = codes.first_mut()
            && first.0 == "equal"
        {
            let (tag, i1, i2, j1, j2) = *first;
            *first = (
                tag,
                i1.max(i2.saturating_sub(n)),
                i2,
                j1.max(j2.saturating_sub(n)),
                j2,
            );
        }
        if let Some(last) = codes.last_mut()
            && last.0 == "equal"
        {
            let (tag, i1, i2, j1, j2) = *last;
            *last = (tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n));
        }
        let mut groups = Vec::new();
        let mut group = Vec::new();
        for (tag, mut i1, i2, mut j1, j2) in codes {
            if tag == "equal" && i2 - i1 > 2 * n {
                group.push((tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n)));
                groups.push(std::mem::take(&mut group));
                i1 = i1.max(i2.saturating_sub(n));
                j1 = j1.max(j2.saturating_sub(n));
            }
            group.push((tag, i1, i2, j1, j2));
        }
        let only_context = group.len() == 1 && group[0].0 == "equal";
        if !group.is_empty() && !only_context {
            groups.push(group);
        }
        groups
    }
}

/// `_format_range_unified`.
fn range(start: usize, stop: usize) -> String {
    let length = stop - start;
    match length {
        1 => format!("{}", start + 1),
        0 => format!("{start},0"),
        _ => format!("{},{length}", start + 1),
    }
}

/// `"".join(difflib.unified_diff(a, b, fromfile, tofile, n=n))` for lines
/// that keep their line endings.
pub(crate) fn unified_diff(
    a: &[String],
    b: &[String],
    fromfile: &str,
    tofile: &str,
    n: usize,
) -> String {
    let mut out = String::new();
    for (index, group) in Matcher::new(a, b).grouped_opcodes(n).iter().enumerate() {
        if index == 0 {
            out.push_str(&format!("--- {fromfile}\n+++ {tofile}\n"));
        }
        let (first, last) = (group[0], group[group.len() - 1]);
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            range(first.1, last.2),
            range(first.3, last.4)
        ));
        for &(tag, i1, i2, j1, j2) in group {
            if tag == "equal" {
                for line in &a[i1..i2] {
                    out.push(' ');
                    out.push_str(line);
                }
                continue;
            }
            if tag == "replace" || tag == "delete" {
                for line in &a[i1..i2] {
                    out.push('-');
                    out.push_str(line);
                }
            }
            if tag == "replace" || tag == "insert" {
                for line in &b[j1..j2] {
                    out.push('+');
                    out.push_str(line);
                }
            }
        }
    }
    out
}

/// `str.splitlines(keepends=True)`.
pub(crate) fn splitlines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        let boundary = match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    current.push('\n');
                    chars.next();
                }
                true
            }
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => true,
            _ => false,
        };
        if boundary {
            lines.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{splitlines, unified_diff};

    fn lines(text: &str) -> Vec<String> {
        splitlines(text)
    }

    #[test]
    fn splitlines_keeps_every_python_boundary() {
        assert_eq!(
            lines("a\r\nb\rc\x0bd\u{2028}e"),
            ["a\r\n", "b\r", "c\x0b", "d\u{2028}", "e"]
        );
        assert!(lines("").is_empty());
    }

    #[test]
    fn one_changed_line_gets_three_lines_of_context() {
        let a = lines("1\n2\n3\n4\n5\n6\n7\n8\n9\n");
        let b = lines("1\n2\n3\n4\nX\n6\n7\n8\n9\n");
        assert_eq!(
            unified_diff(&a, &b, "a/f", "b/f", 3),
            "--- a/f\n+++ b/f\n@@ -2,7 +2,7 @@\n 2\n 3\n 4\n-5\n+X\n 6\n 7\n 8\n"
        );
        assert_eq!(unified_diff(&a, &a, "a/f", "b/f", 3), "");
    }

    #[test]
    fn distant_changes_make_separate_hunks() {
        let a: Vec<String> = (0..20).map(|i| format!("{i}\n")).collect();
        let mut b = a.clone();
        b[1] = "x\n".into();
        b[18] = "y".into();
        let diff = unified_diff(&a, &b, "a", "b", 3);
        assert!(diff.contains("@@ -1,5 +1,5 @@\n"), "{diff}");
        assert!(diff.contains("@@ -16,5 +16,5 @@\n"), "{diff}");
        assert!(diff.ends_with("+y 19\n"), "{diff}");
    }
}
