use std::{
    any::TypeId,
    collections::{HashMap, HashSet, VecDeque},
    fmt::Debug,
};

use crate::{
    error::CycleFound,
    vtable::{self, Object, Tag, VTable},
    ContainerError,
};

#[derive(Default)]
pub struct Container {
    objects: HashMap<Tag, Object>,
}

impl Container {
    /// Provide the given value to the container. Should be used for
    /// for input traits.
    pub fn with<T: 'static>(mut self, tag: Tag, value: T) -> Self {
        let type_id = TypeId::of::<T>();
        assert_eq!(tag.type_id(), type_id, "Type mismatch.");
        self.objects.insert(tag, Object::new(value));
        self
    }

    /// Returns a reference to the given value from the tag.
    pub fn get<T: 'static>(&self, tag: Tag) -> &T {
        self.objects
            .get(&tag)
            .expect("{tag:?} not initialized in the container.")
            .downcast::<T>()
    }

    /// Initialize the container based on the provided dependency graph.
    pub fn initialize(mut self, graph: DependencyGraph) -> Result<Self, ContainerError> {
        // Step 1: Ensure that every input type has been provided.
        for tag in graph.get_inputs() {
            if !self.objects.contains_key(tag) {
                return Err(ContainerError::InputNotProvided(*tag));
            }
        }

        // Step 2: Order the objects we need to initialize by topologically ordering the dependency
        // graph.
        let sorted = graph.sort().map_err(ContainerError::CycleFound)?;

        for tag in &sorted {
            let vtable = graph.vtables.get(tag).unwrap();
            let object = vtable
                .init(&self)
                .map_err(|e| ContainerError::InitializationFailed(*tag, e))?;
            self.objects.insert(*tag, object);
        }

        for tag in sorted {
            let vtable = graph.vtables.get(&tag).unwrap();
            // We can not hold a mutable reference to self and pass &self to the
            // `post` function. Since `post` is not supposed to get reference to
            // self anyway, we can simply remove it from the map at this step and
            // put it back later.
            //
            // The edge case is that a trait would get `self` as a post dependency,
            // which is dumb enough for us to not care about supporting.
            let mut object = self.objects.remove(&tag).unwrap();
            vtable.post(&mut object, &self);
            self.objects.insert(tag, object);
        }

        // Run the `post_initialization` for the input types.
        for tag in graph.get_inputs() {
            let vtable = graph.vtables.get(tag).unwrap();
            let mut object = self.objects.remove(tag).unwrap();
            vtable.post(&mut object, &self);
            self.objects.insert(*tag, object);
        }

        Ok(self)
    }
}

/// The raw dependency graph of a collection.
pub struct DependencyGraph {
    vtables: HashMap<Tag, VTable>,
    insertion_order: Vec<Tag>,
    dependency_graph: HashMap<Tag, HashSet<Tag>>,
    inputs: HashSet<Tag>,
}

impl DependencyGraph {
    /// Create a new dependency graph from a list of [VTables](VTable). This will collect the
    /// dependencies of each collection member and will construct the raw graph. At this step
    /// cycles are *not* reported.
    pub fn new(collection_vtables: Vec<VTable>) -> Self {
        let len = collection_vtables.len();
        let mut vtables = HashMap::with_capacity(len);
        let insertion_order = Vec::with_capacity(len);
        let dependency_graph = HashMap::with_capacity(len);
        let inputs = HashSet::new();

        let mut visitor = DependencyGraphVisitor {
            current: None,
            insertion_order,
            dependency_graph,
            inputs,
        };

        for table in collection_vtables {
            let tag = table.tag();

            visitor.set_current(tag);
            table.dependencies(&mut visitor);

            vtables.insert(tag, table);
        }

        Self {
            vtables,
            insertion_order: visitor.insertion_order,
            dependency_graph: visitor.dependency_graph,
            inputs: visitor.inputs,
        }
    }

    /// Returns the set containing the types marked as input.
    pub fn get_inputs(&self) -> &HashSet<Tag> {
        &self.inputs
    }

    /// Returns the dependency graph.
    pub fn get_graph(&self) -> &HashMap<Tag, HashSet<Tag>> {
        &self.dependency_graph
    }

    /// Returns `true` if the provided tag is marked as an input.
    pub fn is_input(&self, tag: Tag) -> bool {
        self.inputs.contains(&tag)
    }

    /// Perform topological ordering of this graph. Returns the order at which items
    /// need to be instantiated. Excluding the input items.
    pub fn sort(&self) -> Result<Vec<Tag>, CycleFound> {
        let len = self.dependency_graph.len();
        let mut result = Vec::with_capacity(len);

        // Nodes with degree == 0.
        let mut queue = VecDeque::<Tag>::with_capacity(len);

        // Map each node to its in-degree.
        let mut in_degree = HashMap::<Tag, usize>::with_capacity(len);

        for (v, connections) in &self.dependency_graph {
            in_degree.entry(*v).or_default();

            for tag in connections {
                if self.is_input(*tag) {
                    continue;
                }

                *in_degree.entry(*tag).or_default() += 1;
            }
        }

        for (tag, degree) in self
            .insertion_order
            .iter()
            .filter_map(|t| in_degree.get(t).map(|v| (*t, *v)))
        {
            if degree == 0 {
                queue.push_back(tag);
            }
        }

        while let Some(u) = queue.pop_front() {
            // The degree is zero so it is not depended on any pending things anymore.
            result.push(u);

            // Remove it from the in_degree so that we can end up with only the
            // pending items once the queue is empty. (That would mean there is
            // a cycle)
            in_degree.remove(&u);

            for v in self.dependency_graph.get(&u).unwrap().iter() {
                if let Some(ref_mut) = in_degree.get_mut(v) {
                    *ref_mut -= 1;

                    if *ref_mut == 0 {
                        queue.push_back(u);
                    }
                }
            }
        }

        if !in_degree.is_empty() {
            // There is at least a cycle. We know it only involves the pending nodes.
            // We want to report each cycle separately.
            todo!()
        }

        // Reverse the topological ordering to get the dependency visit ordering.
        result.reverse();

        Ok(result)
    }
}

impl Debug for DependencyGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DependencyGraph")
            .field("input", &self.inputs)
            .field("dependencies", &self.dependency_graph)
            .finish()
    }
}

/// A dependency graph visitor is used by each collection member to report their own
/// dependencies. Additionally, a node can decide to mark itself as an `input` node.
#[derive(Default)]
pub struct DependencyGraphVisitor {
    current: Option<Tag>,
    insertion_order: Vec<Tag>,
    pub(crate) dependency_graph: HashMap<Tag, HashSet<Tag>>,
    pub(crate) inputs: HashSet<Tag>,
}

impl DependencyGraphVisitor {
    /// Set the current node that we are visiting.
    pub(crate) fn set_current(&mut self, tag: Tag) {
        self.current = Some(tag);
        self.dependency_graph.insert(tag, Default::default());
        self.insertion_order.push(tag);
    }

    /// Mark the current node as an input.
    ///
    /// # Panics
    ///
    /// If the current node has already specified dependencies through a prior call to the
    /// [add_dependency](DependencyGraphVisitor::add_dependency) method.
    pub fn mark_input(&mut self) {
        let current = self.current.unwrap();
        if self.inputs.insert(current) {
            assert!(self.dependency_graph.remove(&current).unwrap().is_empty());
        }
    }

    /// Add the provided tag as a dependency to the current node.
    ///
    /// # Panics
    ///
    /// If the current node has already been specified as an input node via a prior call to the
    /// [mark_input](Self::mark_input) method.
    pub fn add_dependency(&mut self, tag: Tag) {
        let current = self.current.unwrap();
        self.dependency_graph
            .get_mut(&current)
            // Should exist unless a call to `mark_input` was made.
            .expect("Can not add dependency to a node after marking it as an input.")
            .insert(tag);
    }
}
