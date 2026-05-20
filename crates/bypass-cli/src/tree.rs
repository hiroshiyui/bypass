// SPDX-License-Identifier: GPL-3.0-or-later

//! ASCII box-drawing tree rendering for `bypass ls`.
//!
//! Pure formatting: takes a list of [`RelPath`]s, returns a `String`.
//! No I/O, no business logic.

use std::collections::BTreeMap;

use bypass_core::path::RelPath;

/// Render `entries` as a `tree(1)`-style listing under `header`.
///
/// Output uses Unicode box-drawing characters (`├── `, `└── `, `│   `).
/// `entries` need not be sorted — the renderer sorts as it builds the trie,
/// so the output is always deterministic for a given input set.
pub fn render(entries: &[RelPath], header: &str) -> String {
    let mut root = Node::default();
    for e in entries {
        let mut cur = &mut root;
        for seg in e.segments() {
            cur = cur.children.entry(seg.to_owned()).or_default();
        }
    }
    let mut out = String::new();
    out.push_str(header);
    out.push('\n');
    render_node(&root, "", &mut out);
    out
}

#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
}

fn render_node(node: &Node, prefix: &str, out: &mut String) {
    let total = node.children.len();
    for (i, (name, child)) in node.children.iter().enumerate() {
        let is_last = i + 1 == total;
        let connector = if is_last { "└── " } else { "├── " };
        out.push_str(prefix);
        out.push_str(connector);
        out.push_str(name);
        out.push('\n');
        let child_prefix = if is_last {
            format!("{prefix}    ")
        } else {
            format!("{prefix}│   ")
        };
        render_node(child, &child_prefix, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(ss: &[&str]) -> Vec<RelPath> {
        ss.iter().map(|s| RelPath::new(*s).unwrap()).collect()
    }

    #[test]
    fn empty_input_renders_only_header() {
        let out = render(&[], "Password Store");
        assert_eq!(out, "Password Store\n");
    }

    #[test]
    fn single_top_level_entry() {
        let out = render(&paths(&["foo"]), "Password Store");
        assert_eq!(out, "Password Store\n└── foo\n");
    }

    #[test]
    fn two_siblings_use_middle_and_last_connectors() {
        let out = render(&paths(&["a", "b"]), "H");
        assert_eq!(out, "H\n├── a\n└── b\n");
    }

    #[test]
    fn nested_subtree_indents_children() {
        let out = render(&paths(&["a/b", "a/c"]), "H");
        // a is the only top-level → "└── a"; its children indent with 4 spaces.
        assert_eq!(out, "H\n└── a\n    ├── b\n    └── c\n");
    }

    #[test]
    fn middle_subtree_uses_pipe_indent() {
        // Two top-level subtrees a/ and b/, so a is "├── a" and its children
        // are prefixed with "│   " (not 4 spaces).
        let out = render(&paths(&["a/x", "b"]), "H");
        assert_eq!(out, "H\n├── a\n│   └── x\n└── b\n");
    }

    #[test]
    fn deep_nesting() {
        let out = render(&paths(&["a/b/c/d"]), "H");
        assert_eq!(
            out,
            "H\n└── a\n    └── b\n        └── c\n            └── d\n"
        );
    }

    #[test]
    fn output_is_sorted_regardless_of_input_order() {
        let out_a = render(&paths(&["b", "a", "c"]), "H");
        let out_b = render(&paths(&["a", "b", "c"]), "H");
        assert_eq!(out_a, out_b);
        assert_eq!(out_a, "H\n├── a\n├── b\n└── c\n");
    }
}
