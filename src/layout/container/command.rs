use super::{ContainerTree, LayoutElement, NodeData, NodeKey};

/// How the root node participates in command handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::layout) enum RootPolicy {
    /// The root is a workspace implementation detail; commands at that level
    /// operate through the workspace context.
    ImplicitWorkspace,
    /// The root is an addressable container, as in the floating layer.
    MaterialContainer,
}

/// Resolved target for commands that may operate on a workspace, container, or leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::layout) enum TreeCommandTarget {
    Workspace,
    Container(NodeKey),
    Leaf(NodeKey),
}

impl<W: LayoutElement> ContainerTree<W> {
    pub(in crate::layout) fn command_target(&self, root_policy: RootPolicy) -> TreeCommandTarget {
        if let Some(selected_key) = self.selected_node_key() {
            if matches!(self.get_node(selected_key), Some(NodeData::Container(_))) {
                if Some(selected_key) == self.root_node_key()
                    && root_policy == RootPolicy::ImplicitWorkspace
                {
                    return TreeCommandTarget::Workspace;
                }

                return TreeCommandTarget::Container(selected_key);
            }
        }

        self.focused_node_key()
            .map(TreeCommandTarget::Leaf)
            .unwrap_or(TreeCommandTarget::Workspace)
    }
}
