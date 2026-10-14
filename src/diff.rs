//! Read-only unified diff coordinates and paired alignment. Git supplies the diff
//! algorithm; this module only projects its bounded, internally generated patch.
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Header,
    Context,
    Add,
    Delete,
}
#[derive(Clone, Debug)]
pub struct Row {
    pub kind: Kind,
    pub old: Option<usize>,
    pub new: Option<usize>,
    /// Nearest following new line, clamped by the source buffer at navigation.
    pub anchor: usize,
    pub text: String,
    pub no_newline: bool,
}
#[derive(Clone, Debug)]
pub struct Pair {
    pub left: Option<usize>,
    pub right: Option<usize>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Added,
    Modified,
    Deleted,
}
#[derive(Clone, Debug, Default)]
pub struct Diff {
    pub identity: Option<crate::review::Identity>,
    pub locations: crate::review::Locations,
    pub left_label: String,
    pub right_label: String,
    pub rows: Vec<Row>,
    pub pairs: Vec<Pair>,
    pub hunks: Vec<usize>,
    pub marks: BTreeMap<usize, Mark>,
    pub source_available: bool,
}
impl Diff {
    pub fn parse(patch: &str) -> Result<Self, String> {
        let mut diff = Self::default();
        let (mut old, mut new) = (0, 0);
        let (mut old_left, mut new_left) = (0, 0);
        for line in patch.split_terminator('\n') {
            if line.starts_with("@@ ") {
                if old_left != 0 || new_left != 0 {
                    return Err("Incomplete Git hunk".into());
                }
                let mut parts = line.split_whitespace();
                parts.next();
                let a = range(parts.next().ok_or("Missing old range")?)?;
                let b = range(parts.next().ok_or("Missing new range")?)?;
                (old, old_left) = a;
                (new, new_left) = b;
                diff.hunks.push(diff.rows.len());
                diff.rows.push(Row {
                    kind: Kind::Header,
                    old: None,
                    new: None,
                    anchor: new,
                    text: line.into(),
                    no_newline: false,
                });
            } else if !diff.hunks.is_empty() {
                if line.starts_with("\\ No newline") {
                    if let Some(row) = diff.rows.last_mut() {
                        row.no_newline = true;
                    }
                    continue;
                }
                let kind = match line.as_bytes().first() {
                    Some(b' ') => Kind::Context,
                    Some(b'+') => Kind::Add,
                    Some(b'-') => Kind::Delete,
                    _ => return Err("Unsupported Git patch record".into()),
                };
                let has_old = kind != Kind::Add;
                let has_new = kind != Kind::Delete;
                if (has_old && old_left == 0) || (has_new && new_left == 0) {
                    return Err("Invalid Git hunk lengths".into());
                }
                diff.rows.push(Row {
                    kind,
                    old: has_old.then_some(old),
                    new: has_new.then_some(new),
                    anchor: new,
                    text: line[1..].into(),
                    no_newline: false,
                });
                if has_old {
                    old += 1;
                    old_left -= 1;
                }
                if has_new {
                    new += 1;
                    new_left -= 1;
                }
            }
            if diff.rows.len() > 100_000 {
                return Err("Unsupported diff: exceeds 100,000 rows".into());
            }
        }
        if old_left != 0 || new_left != 0 {
            return Err("Incomplete Git hunk".into());
        }
        let mut index = 0;
        while index < diff.rows.len() {
            if matches!(diff.rows[index].kind, Kind::Header | Kind::Context) {
                diff.pairs.push(Pair {
                    left: Some(index),
                    right: Some(index),
                });
                index += 1;
                continue;
            }
            let mut deleted = Vec::new();
            let mut added = Vec::new();
            while index < diff.rows.len()
                && matches!(diff.rows[index].kind, Kind::Add | Kind::Delete)
            {
                if diff.rows[index].kind == Kind::Delete {
                    deleted.push(index);
                } else {
                    added.push(index);
                }
                index += 1;
            }
            for offset in 0..deleted.len().max(added.len()) {
                diff.pairs.push(Pair {
                    left: deleted.get(offset).copied(),
                    right: added.get(offset).copied(),
                });
            }
            for row in &added {
                diff.marks.insert(
                    diff.rows[*row].anchor,
                    if deleted.is_empty() {
                        Mark::Added
                    } else {
                        Mark::Modified
                    },
                );
            }
            if !deleted.is_empty() && added.is_empty() {
                diff.marks
                    .insert(diff.rows[deleted[0]].anchor, Mark::Deleted);
            }
        }
        Ok(diff)
    }
    pub fn nearest(&self, source_line: usize) -> usize {
        self.rows
            .iter()
            .enumerate()
            .min_by_key(|(_, row)| (row.anchor.abs_diff(source_line), row.kind == Kind::Header))
            .map_or(0, |(i, _)| i)
    }
    pub fn pair_index(&self, row: usize) -> usize {
        self.pairs
            .iter()
            .position(|p| p.left == Some(row) || p.right == Some(row))
            .unwrap_or(0)
    }
    /// Map old-side coordinates to new-side coordinates, including omitted
    /// unchanged ranges. Deleted-only coordinates anchor to the following line.
    pub fn map_old(&self, line: usize) -> usize {
        let mut shift = 0isize;
        for row in &self.rows {
            if row.kind == Kind::Header {
                continue;
            }
            if let Some(old) = row.old {
                if old > line {
                    break;
                }
                if old == line {
                    return row.anchor;
                }
                shift = row.anchor as isize - old as isize - isize::from(row.kind == Kind::Delete);
            } else if row.kind == Kind::Add {
                shift += 1;
            }
        }
        line.saturating_add_signed(shift)
    }
}
fn range(value: &str) -> Result<(usize, usize), String> {
    let value = value.get(1..).ok_or("Invalid hunk range")?;
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    let start: usize = start.parse().map_err(|_| "Invalid hunk start")?;
    let count: usize = count.parse().map_err(|_| "Invalid hunk count")?;
    Ok((
        if count == 0 {
            start
        } else {
            start.saturating_sub(1)
        },
        count,
    ))
}
