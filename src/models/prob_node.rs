use std::{
    ptr,
    sync::{
        atomic::{AtomicPtr, Ordering},
        Arc,
    },
};

use super::{
    prob_lazy_load::{lazy_item::ProbLazyItem, lazy_item_array::ProbLazyItemArray},
    types::{HNSWLevel, MetricResult, NodeProp, VectorId},
};

pub type SharedNode = Arc<ProbLazyItem<ProbNode>>;

pub struct ProbNode {
    pub hnsw_level: HNSWLevel,
    pub prop: Arc<NodeProp>,
    neighbors: Box<[AtomicPtr<(SharedNode, MetricResult)>]>,
    parent: AtomicPtr<SharedNode>,
    child: AtomicPtr<SharedNode>,
    pub versions: ProbLazyItemArray<ProbNode, 4>,
}

unsafe impl Send for ProbNode {}
unsafe impl Sync for ProbNode {}

impl ProbNode {
    pub fn new(
        hnsw_level: HNSWLevel,
        prop: Arc<NodeProp>,
        parent: Option<SharedNode>,
        child: Option<SharedNode>,
        neighbors_count: usize,
    ) -> Self {
        let mut neighbors = Vec::with_capacity(neighbors_count);

        for _ in 0..neighbors_count {
            neighbors.push(AtomicPtr::new(ptr::null_mut()));
        }

        Self {
            hnsw_level,
            prop,
            neighbors: neighbors.into_boxed_slice(),
            parent: AtomicPtr::new(
                parent.map_or_else(|| ptr::null_mut(), |parent| Box::into_raw(Box::new(parent))),
            ),
            child: AtomicPtr::new(
                child.map_or_else(|| ptr::null_mut(), |child| Box::into_raw(Box::new(child))),
            ),
            versions: ProbLazyItemArray::new(),
        }
    }

    pub fn new_with_neighbors(
        hnsw_level: HNSWLevel,
        prop: Arc<NodeProp>,
        neighbors: Box<[AtomicPtr<(SharedNode, MetricResult)>]>,
        parent: Option<SharedNode>,
        child: Option<SharedNode>,
    ) -> Self {
        Self {
            hnsw_level,
            prop,
            neighbors,
            parent: AtomicPtr::new(
                parent.map_or_else(|| ptr::null_mut(), |parent| Box::into_raw(Box::new(parent))),
            ),
            child: AtomicPtr::new(
                child.map_or_else(|| ptr::null_mut(), |child| Box::into_raw(Box::new(child))),
            ),
            versions: ProbLazyItemArray::new(),
        }
    }

    pub fn new_with_neighbors_and_versions(
        hnsw_level: HNSWLevel,
        prop: Arc<NodeProp>,
        neighbors: Box<[AtomicPtr<(SharedNode, MetricResult)>]>,
        parent: Option<SharedNode>,
        child: Option<SharedNode>,
        versions: ProbLazyItemArray<ProbNode, 4>,
    ) -> Self {
        Self {
            hnsw_level,
            prop,
            neighbors,
            parent: AtomicPtr::new(
                parent.map_or_else(|| ptr::null_mut(), |parent| Box::into_raw(Box::new(parent))),
            ),
            child: AtomicPtr::new(
                child.map_or_else(|| ptr::null_mut(), |child| Box::into_raw(Box::new(child))),
            ),
            versions,
        }
    }

    pub fn get_parent(&self) -> Option<SharedNode> {
        unsafe {
            let ptr = self.parent.load(Ordering::SeqCst);
            if ptr.is_null() {
                None
            } else {
                Some((*ptr).clone())
            }
        }
    }

    pub fn set_parent(&self, parent: SharedNode) {
        unsafe {
            let old = self
                .parent
                .swap(Box::into_raw(Box::new(parent)), Ordering::SeqCst);
            if !old.is_null() {
                drop(Box::from_raw(old));
            }
        }
    }

    pub fn get_child(&self) -> Option<SharedNode> {
        unsafe {
            let ptr = self.child.load(Ordering::SeqCst);
            if ptr.is_null() {
                None
            } else {
                Some((*ptr).clone())
            }
        }
    }

    pub fn set_child(&self, child: SharedNode) {
        unsafe {
            let old = self
                .child
                .swap(Box::into_raw(Box::new(child)), Ordering::SeqCst);
            if !old.is_null() {
                drop(Box::from_raw(old));
            }
        }
    }

    pub fn get_id(&self) -> &VectorId {
        &self.prop.id
    }

    pub fn add_neighbor(
        &self,
        neighbor_node: SharedNode,
        neighbor_id: &VectorId,
        dist: MetricResult,
    ) {
        let initial_idx = ((self.get_id().get_hash() ^ neighbor_id.get_hash())
            % self.neighbors.len() as u64) as usize;
        let neighbor = Box::new((neighbor_node, dist));
        let neighbor_ptr = Box::into_raw(neighbor);

        let mut current_idx = initial_idx;
        let mut inserted = false;

        // Try half of indices, starting from initial_idx, with wrap around
        for _ in 0..(self.neighbors.len() / 2) {
            let result = self.neighbors[current_idx].fetch_update(
                Ordering::SeqCst,
                Ordering::SeqCst,
                |current_neighbor| {
                    if current_neighbor.is_null() {
                        Some(neighbor_ptr)
                    } else {
                        unsafe {
                            if dist.get_value() > (*current_neighbor).1.get_value() {
                                Some(neighbor_ptr)
                            } else {
                                None
                            }
                        }
                    }
                },
            );

            match result {
                Ok(prev_neighbor) => {
                    unsafe {
                        if !prev_neighbor.is_null() {
                            drop(Box::from_raw(prev_neighbor));
                        }
                    }
                    inserted = true;
                    break;
                }
                Err(_) => {
                    // Try next index with wraparound
                    current_idx = (current_idx + 1) % self.neighbors.len();
                }
            }
        }

        // If we couldn't insert after trying all positions, clean up
        if !inserted {
            unsafe {
                drop(Box::from_raw(neighbor_ptr));
            }
        }
    }

    pub fn get_neighbors(&self) -> Vec<SharedNode> {
        self.neighbors
            .iter()
            .flat_map(|neighbor| unsafe {
                neighbor
                    .load(Ordering::Relaxed)
                    .as_ref()
                    .map(|neighbor| neighbor.0.clone())
            })
            .collect()
    }

    pub fn clone_neighbors(&self) -> Box<[AtomicPtr<(SharedNode, MetricResult)>]> {
        self.neighbors
            .iter()
            .map(|neighbor| unsafe {
                AtomicPtr::new(neighbor.load(Ordering::SeqCst).as_ref().map_or_else(
                    || ptr::null_mut(),
                    |neighbor| Box::into_raw(Box::new(neighbor.clone())),
                ))
            })
            .collect::<Vec<_>>()
            .into_boxed_slice()
    }

    pub fn get_neighbors_raw(&self) -> &Box<[AtomicPtr<(SharedNode, MetricResult)>]> {
        &self.neighbors
    }
}

impl Drop for ProbNode {
    fn drop(&mut self) {
        for neighbor in &self.neighbors {
            let ptr = neighbor.load(Ordering::SeqCst);

            if !ptr.is_null() {
                unsafe {
                    drop(Box::from_raw(ptr));
                }
            }
        }
    }
}
