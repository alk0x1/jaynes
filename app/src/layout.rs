//! How a tab's terminals share the middle, as Herdr keeps it in its `herdr-app` document: a tree whose leaves are
//! terminals (blocks) and whose nodes put their children side by side (a row) or one above the other (a column), each
//! child with its share of the room (its weight). Read here for the live terminals only, and changed here when the
//! person splits a pane, keeping every field the app does not know.

use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dir {
    /// Side by side: Herdr's "row", a split to the right.
    Row,
    /// One above the other: Herdr's "column", a split down.
    Column,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Layout {
    Pane(String),
    Split(Dir, Vec<(f32, Layout)>),
}

impl Layout {
    /// The terminals it holds, in reading order.
    pub fn panes(&self) -> Vec<String> {
        match self {
            Layout::Pane(a) => vec![a.clone()],
            Layout::Split(_, kids) => kids.iter().flat_map(|(_, k)| k.panes()).collect(),
        }
    }
}

/// A tab's layout with only the live terminals; None when none of its terminals lives. A node left with one child is
/// that child, as Herdr shows it.
pub fn read(node: &Value, live: &dyn Fn(&str) -> bool) -> Option<Layout> {
    if let Some(t) = node["content"]["terminal"].as_str() {
        return live(t).then(|| Layout::Pane(t.to_owned()));
    }
    let dir = if node["type"] == "column" { Dir::Column } else { Dir::Row };
    let kids: Vec<(f32, Layout)> = node["children"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| read(c, live).map(|l| (c["size"]["weight"].as_f64().unwrap_or(1.) as f32, l)))
        .collect();
    match kids.len() {
        0 => None,
        1 => kids.into_iter().next().map(|(_, l)| l),
        _ => Some(Layout::Split(dir, kids)),
    }
}

/// The document with a new terminal split beside `target` in a tab: to its right (Row) or under it (Column). When
/// the target already sits in a node of that direction, the new block goes right after it and the two share the
/// target's room; otherwise the target's block becomes a node of that direction holding both, half each. None when
/// the tab or the target is not there.
pub fn split(doc: &Value, tab: &str, target: &str, terminal: &str, block: &str, node: &str, dir: Dir) -> Option<Value> {
    let mut out = doc.clone();
    let tabs = out["spaces"].as_array_mut()?.iter_mut().flat_map(|s| s["tabs"].as_array_mut().into_iter().flatten());
    let t = tabs.into_iter().find(|t| t["id"] == tab)?;
    let ty = if dir == Dir::Row { "row" } else { "column" };
    let new_block = |w: f64| json!({"type": "block", "id": block, "size": {"weight": w}, "content": {"terminal": terminal}});
    fn walk(n: &mut Value, target: &str, ty: &str, new_block: &dyn Fn(f64) -> Value, node: &str) -> bool {
        let same = n["type"] == ty;
        let Some(kids) = n["children"].as_array_mut() else { return false };
        if let Some(i) = kids.iter().position(|k| k["content"]["terminal"] == target) {
            if same {
                // the same direction: the new one takes half of the target's room, right after it
                let w = kids[i]["size"]["weight"].as_f64().unwrap_or(1.) / 2.;
                kids[i]["size"] = json!({"weight": w});
                kids.insert(i + 1, new_block(w));
            } else {
                let mut old = kids[i].clone();
                let w = old["size"].clone();
                old["size"] = json!({"weight": 0.5});
                kids[i] = json!({"type": ty, "id": node, "size": w, "children": [old, new_block(0.5)]});
                if kids[i]["size"].is_null() {
                    kids[i].as_object_mut().map(|o| o.remove("size"));
                }
            }
            return true;
        }
        kids.iter_mut().any(|k| walk(k, target, ty, new_block, node))
    }
    walk(&mut t["root"], target, ty, &new_block, node).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::{Dir, Layout, read, split};
    use serde_json::json;

    fn p(a: &str) -> Layout {
        Layout::Pane(a.into())
    }

    #[test]
    fn a_tab_split_in_herdr_reads_as_its_tree() {
        // the real shape of jaynes/tenure's tab: two terminals side by side, half each
        let root = json!({"type": "row", "id": "tb1", "children": [
            {"type": "block", "id": "p1", "size": {"weight": 0.5}, "content": {"terminal": "t10w"}},
            {"type": "column", "id": "c1", "size": {"weight": 0.5}, "children": [
                {"type": "block", "id": "p2", "content": {"terminal": "t111"}},
                {"type": "block", "id": "p3", "content": {"terminal": "t9"}}
            ]}
        ]});
        let all = read(&root, &|_| true).unwrap();
        assert_eq!(all, Layout::Split(Dir::Row, vec![(0.5, p("t10w")), (0.5, Layout::Split(Dir::Column, vec![(1., p("t111")), (1., p("t9"))]))]));
        assert_eq!(all.panes(), ["t10w", "t111", "t9"]);
        // a finished terminal leaves, and a node with one child left is that child
        assert_eq!(read(&root, &|t| t != "t9"), Some(Layout::Split(Dir::Row, vec![(0.5, p("t10w")), (0.5, p("t111"))])));
        assert_eq!(read(&root, &|t| t == "t111"), Some(p("t111")));
        assert_eq!(read(&root, &|_| false), None);
    }

    fn doc() -> serde_json::Value {
        json!({"spaces": [{"id": "s1", "extra": 1, "tabs": [{"id": "tb1", "name": "", "root": {"type": "row", "id": "tb1", "children": [
            {"type": "block", "id": "p1", "content": {"terminal": "t1"}}
        ]}}]}]})
    }

    #[test]
    fn splitting_right_beside_a_lone_pane_shares_the_row() {
        let out = split(&doc(), "tb1", "t1", "t2", "p2", "n1", Dir::Row).unwrap();
        let root = &out["spaces"][0]["tabs"][0]["root"];
        assert_eq!(read(root, &|_| true), Some(Layout::Split(Dir::Row, vec![(0.5, p("t1")), (0.5, p("t2"))])));
        // what the app does not know stays
        assert_eq!(out["spaces"][0]["extra"], 1);
    }

    #[test]
    fn splitting_down_makes_a_column_where_the_pane_was() {
        let out = split(&doc(), "tb1", "t1", "t2", "p2", "n1", Dir::Column).unwrap();
        let root = &out["spaces"][0]["tabs"][0]["root"];
        assert_eq!(read(root, &|_| true), Some(Layout::Split(Dir::Column, vec![(0.5, p("t1")), (0.5, p("t2"))])));
        assert_eq!(root["children"][0]["type"], "column");
        assert_eq!(root["children"][0]["id"], "n1");
        assert_eq!(split(&doc(), "tb1", "t9", "t2", "p2", "n1", Dir::Row), None);
        assert_eq!(split(&doc(), "tb9", "t1", "t2", "p2", "n1", Dir::Row), None);
    }
}
