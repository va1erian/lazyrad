//! Tree depth: a deep chain is refused, not recursed into.
//!
//! `Node` owns its children, so the compiler-generated `Drop`, `Clone`,
//! `PartialEq` and `Debug` recurse once per level. A 10 000-deep chain used to
//! validate and then overflow the stack when it was dropped.

use lazyrad_view::{MAX_TREE_DEPTH, Node, Tree, TreeError};

/// A chain `n0 <- n1 <- ... <- n{depth-1}` built without recursion.
fn chain(depth: usize) -> Node {
    let mut node = Node::new("n0", "Box");
    for level in 1..depth {
        node = Node::new(format!("n{level}"), "Box").with_child(node);
    }
    node
}

fn on_one_mebibyte_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(body)
        .expect("spawn")
        .join()
        .expect("no overflow");
}

#[test]
fn a_very_deep_chain_is_refused_and_dropped_without_overflowing() {
    on_one_mebibyte_stack(|| {
        let error = Tree::new(chain(10_000)).unwrap_err();
        let TreeError::TooDeep { depth, max, .. } = error else {
            panic!("expected TooDeep, got {error:?}");
        };
        assert_eq!((depth, max), (MAX_TREE_DEPTH + 1, MAX_TREE_DEPTH));
        // Dropping a deep `Node` on its own is iterative too.
        drop(chain(100_000));
    });
}

#[test]
fn the_limit_is_exact_and_a_validated_tree_is_safe_to_copy_and_compare() {
    on_one_mebibyte_stack(|| {
        let deepest = Tree::new(chain(MAX_TREE_DEPTH)).expect("exactly at the limit");
        let copy = deepest.clone();
        assert_eq!(copy, deepest);
        assert!(!format!("{copy:?}").is_empty());
        assert_eq!(deepest.len(), MAX_TREE_DEPTH);
        // The deepest leaf can still be found.
        assert!(deepest.get("n0").is_some());

        let too_deep = Tree::new(chain(MAX_TREE_DEPTH + 1)).unwrap_err();
        assert_eq!(
            too_deep.to_string(),
            format!(
                "node `n0` is nested {} levels deep; the limit is {MAX_TREE_DEPTH}",
                MAX_TREE_DEPTH + 1
            )
        );
    });
}

#[test]
fn breadth_is_not_depth() {
    // Ten thousand siblings are fine: only nesting is limited.
    let mut root = Node::new("root", "Column");
    for i in 0..10_000 {
        root = root.with_child(Node::new(format!("c{i}"), "Label"));
    }
    let tree = Tree::new(root).expect("wide tree");
    assert_eq!(tree.len(), 10_001);
}
