//! The layout algorithms themselves

pub(crate) mod common;
pub(crate) mod flexbox;
pub(crate) mod leaf;

#[cfg(feature = "grid")]
pub(crate) mod grid;

use crate::data::CACHE_SIZE;
use crate::error::TaffyError;
use crate::geometry::{Point, Size};
use crate::layout::{Cache, Layout, RunMode, SizingMode};
use crate::node::Node;
use crate::style::{AvailableSpace, Display};
use crate::sys::round;
use crate::tree::LayoutTree;

#[cfg(feature = "debug")]
use crate::debug::NODE_LOGGER;

/// Updates the stored layout of the provided `node` and its children
pub fn compute_layout(
    tree: &mut impl LayoutTree,
    root: Node,
    available_space: Size<AvailableSpace>,
) -> Result<(), TaffyError> {
    // Recursively compute node layout
    let size = compute_node_layout(
        tree,
        root,
        Size::NONE,
        available_space.into_options(),
        available_space,
        RunMode::PeformLayout,
        SizingMode::InherentSize,
    );

    let layout = Layout { order: 0, size, location: Point::ZERO };
    *tree.layout_mut(root) = layout;

    // Recursively round the layout's of this node and all children
    round_layout(tree, root, 0.0, 0.0);

    Ok(())
}

/// Updates the stored layout of the provided `node` and its children
fn compute_node_layout(
    tree: &mut impl LayoutTree,
    node: Node,
    known_dimensions: Size<Option<f32>>,
    parent_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    run_mode: RunMode,
    sizing_mode: SizingMode,
) -> Size<f32> {
    #[cfg(feature = "debug")]
    NODE_LOGGER.push_node(node);
    #[cfg(feature = "debug")]
    println!();

    // First we check if we have a cached result for the given input
    let cache_run_mode = if tree.is_childless(node) { RunMode::PeformLayout } else { run_mode };
    if let Some(cached_size) =
        compute_from_cache(tree, node, known_dimensions, available_space, cache_run_mode, sizing_mode)
    {
        #[cfg(feature = "debug")]
        NODE_LOGGER.labelled_debug_log("CACHE", cached_size);
        #[cfg(feature = "debug")]
        NODE_LOGGER.labelled_debug_log("run_mode", run_mode);
        #[cfg(feature = "debug")]
        NODE_LOGGER.labelled_debug_log("sizing_mode", sizing_mode);
        #[cfg(feature = "debug")]
        NODE_LOGGER.labelled_debug_log("known_dimensions", known_dimensions);
        #[cfg(feature = "debug")]
        NODE_LOGGER.labelled_debug_log("available_space", available_space);
        #[cfg(feature = "debug")]
        NODE_LOGGER.pop_node();
        return cached_size;
    }

    #[cfg(feature = "debug")]
    NODE_LOGGER.log("COMPUTE");
    #[cfg(feature = "debug")]
    NODE_LOGGER.labelled_debug_log("run_mode", run_mode);
    #[cfg(feature = "debug")]
    NODE_LOGGER.labelled_debug_log("sizing_mode", sizing_mode);
    #[cfg(feature = "debug")]
    NODE_LOGGER.labelled_debug_log("known_dimensions", known_dimensions);
    #[cfg(feature = "debug")]
    NODE_LOGGER.labelled_debug_log("available_space", available_space);

    // If this is a leaf node we can skip a lot of this function in some cases
    let computed_size = if tree.is_childless(node) {
        #[cfg(feature = "debug")]
        NODE_LOGGER.log("Algo: leaf");
        self::leaf::compute(tree, node, known_dimensions, parent_size, available_space, run_mode, sizing_mode)
    } else {
        // println!("match {:?}", tree.style(node).display);
        match tree.style(node).display {
            Display::Flex => {
                #[cfg(feature = "debug")]
                NODE_LOGGER.log("Algo: flexbox");
                self::flexbox::compute(tree, node, known_dimensions, parent_size, available_space, run_mode)
            }
            #[cfg(feature = "grid")]
            Display::Grid => self::grid::compute(tree, node, known_dimensions, parent_size, available_space),
            Display::None => {
                #[cfg(feature = "debug")]
                NODE_LOGGER.log("Algo: none");
                perform_hidden_layout(tree, node)
            }
        }
    };

    // Cache result
    let cache_slot = compute_cache_slot(known_dimensions, available_space);
    *tree.cache_mut(node, cache_slot) =
        Some(Cache { known_dimensions, available_space, run_mode: cache_run_mode, cached_size: computed_size });

    #[cfg(feature = "debug")]
    NODE_LOGGER.labelled_debug_log("RESULT", computed_size);
    #[cfg(feature = "debug")]
    NODE_LOGGER.pop_node();

    computed_size
}

/// Return the cache slot to cache the current computed result in
///
/// ## Caching Strategy
///
/// We need multiple cache slots, because a node's size is often queried by it's parent multiple times in the course of the layout
/// process, and we don't want later results to clobber earlier ones.
///
/// The two variables that we care about when determining cache slot are:
///
///   - How many "known_dimensions" are set. In the worst case, a node may be called first with neither dimensions known known_dimensions,
///     then with one dimension known (either width of height - which doesn't matter for our purposes here), and then with both dimensions
///     known.
///   - Whether unknown dimensions are being sized under a min-content or a max-content available space constraint (definite available space
///     shares a cache slot with max-content because a node will generally be sized under one or the other but not both).
///
/// ## Cache slots:
///
/// - Slot 0: Both known_dimensions were set
/// - Slot 1: 1 of 2 known_dimensions were set and the other dimension was either a MaxContent or Definite available space constraint
/// - Slot 2: 1 of 2 known_dimensions were set and the other dimension was a MinContent constraint
/// - Slot 3: Neither known_dimensions were set and we are sizing under a MaxContent or Definite available space constraint
/// - Slot 4: Neither known_dimensions were set and we are sizing under a MinContent constraint
#[inline]
fn compute_cache_slot(known_dimensions: Size<Option<f32>>, available_space: Size<AvailableSpace>) -> usize {
    let has_known_width = known_dimensions.width.is_some();
    let has_known_height = known_dimensions.height.is_some();

    // Slot 0: Both known_dimensions were set
    if has_known_width && has_known_height {
        return 0;
    }

    // Slot 1: 1 of 2 known_dimensions were set and the other dimension was either a MaxContent or Definite available space constraint
    // Slot 2: 1 of 2 known_dimensions were set and the other dimension was a MinContent constraint
    if has_known_width || has_known_height {
        let other_dim_available_space = if has_known_width { available_space.height } else { available_space.width };
        return 1 + (other_dim_available_space == AvailableSpace::MinContent) as usize;
    }

    // Slot 3: Neither known_dimensions were set and we are sizing under a MaxContent or Definite available space constraint
    // Slot 4: Neither known_dimensions were set and we are sizing under a MinContent constraint
    3 + (available_space.width == AvailableSpace::MinContent) as usize
}

/// Try to get the computation result from the cache.
#[inline]
fn compute_from_cache(
    tree: &mut impl LayoutTree,
    node: Node,
    known_dimensions: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    run_mode: RunMode,
    sizing_mode: SizingMode,
) -> Option<Size<f32>> {
    for idx in 0..CACHE_SIZE {
        let entry = tree.cache_mut(node, idx);
        if let Some(entry) = entry {
            // Cached ComputeSize results are not valid if we are running in PerformLayout mode
            if entry.run_mode == RunMode::ComputeSize && run_mode == RunMode::PeformLayout {
                return None;
            }

            if (known_dimensions.width == entry.known_dimensions.width
                || known_dimensions.width == Some(entry.cached_size.width))
                && (known_dimensions.height == entry.known_dimensions.height
                    || known_dimensions.height == Some(entry.cached_size.height))
                && (known_dimensions.width.is_some()
                    || entry.available_space.width.is_roughly_equal(available_space.width)
                    || (sizing_mode == SizingMode::ContentSize
                        && available_space.width.is_definite()
                        && available_space.width.unwrap() >= entry.cached_size.width))
                && (known_dimensions.height.is_some()
                    || entry.available_space.height.is_roughly_equal(available_space.height)
                    || (sizing_mode == SizingMode::ContentSize
                        && available_space.height.is_definite()
                        && available_space.height.unwrap() >= entry.cached_size.height))
            {
                return Some(entry.cached_size);
            }
        }
    }

    None
}

/// Creates a layout for this node and its children, recursively.
/// Each hidden node has zero size and is placed at the origin
fn perform_hidden_layout(tree: &mut impl LayoutTree, node: Node) -> Size<f32> {
    /// Recursive function to apply hidden layout to all descendents
    fn perform_hidden_layout_inner(tree: &mut impl LayoutTree, node: Node, order: u32) {
        *tree.layout_mut(node) = Layout::with_order(order);
        for order in 0..tree.child_count(node) {
            perform_hidden_layout_inner(tree, tree.child(node, order), order as _);
        }
    }

    for order in 0..tree.child_count(node) {
        perform_hidden_layout_inner(tree, tree.child(node, order), order as _);
    }

    Size::ZERO
}

/// Rounds the calculated [`NodeData`] according to the spec
fn round_layout(tree: &mut impl LayoutTree, root: Node, abs_x: f32, abs_y: f32) {
    let layout = tree.layout_mut(root);
    let abs_x = abs_x + layout.location.x;
    let abs_y = abs_y + layout.location.y;

    layout.location.x = round(layout.location.x);
    layout.location.y = round(layout.location.y);

    layout.size.width = round(layout.size.width);
    layout.size.height = round(layout.size.height);

    // Satisfy the borrow checker here by re-indexing to shorten the lifetime to the loop scope
    for x in 0..tree.child_count(root) {
        let child = tree.child(root, x);
        round_layout(tree, child, abs_x, abs_y);
    }
}

#[cfg(test)]
mod tests {
    use super::perform_hidden_layout;
    use crate::geometry::{Point, Size};
    use crate::style::{Display, Style};
    use crate::Taffy;

    #[test]
    fn hidden_layout_should_hide_recursively() {
        let mut taffy = Taffy::new();

        let style: Style = Style { display: Display::Flex, size: Size::from_points(50.0, 50.0), ..Default::default() };

        let grandchild_00 = taffy.new_leaf(style.clone()).unwrap();
        let grandchild_01 = taffy.new_leaf(style.clone()).unwrap();
        let child_00 = taffy.new_with_children(style.clone(), &[grandchild_00, grandchild_01]).unwrap();

        let grandchild_02 = taffy.new_leaf(style.clone()).unwrap();
        let child_01 = taffy.new_with_children(style.clone(), &[grandchild_02]).unwrap();

        let root = taffy
            .new_with_children(
                Style { display: Display::None, size: Size::from_points(50.0, 50.0), ..Default::default() },
                &[child_00, child_01],
            )
            .unwrap();

        perform_hidden_layout(&mut taffy, root);

        // Whatever size and display-mode the nodes had previously,
        // all layouts should resolve to ZERO due to the root's DISPLAY::NONE
        for (node, _) in taffy.nodes.iter().filter(|(node, _)| *node != root) {
            if let Ok(layout) = taffy.layout(node) {
                assert_eq!(layout.size, Size::zero());
                assert_eq!(layout.location, Point::zero());
            }
        }
    }
}
