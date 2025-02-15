pub mod file_info;
use std::{
    hash::{Hash, Hasher},
    sync::atomic::AtomicUsize,
};

use common_partitioning::PartitionCacheEntry;
use common_scan_info::PhysicalScanInfo;
use daft_schema::schema::SchemaRef;
pub use file_info::{FileInfo, FileInfos};
use serde::{Deserialize, Serialize};

use crate::partitioning::ClusteringSpecRef;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SourceInfo {
    InMemory(InMemoryInfo),
    Physical(PhysicalScanInfo),
    PlaceHolder(PlaceHolderInfo),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InMemoryInfo {
    pub source_schema: SchemaRef,
    pub cache_key: String,
    pub cache_entry: PartitionCacheEntry,
    pub num_partitions: usize,
    pub size_bytes: usize,
    pub num_rows: usize,
    pub clustering_spec: Option<ClusteringSpecRef>,
}

impl InMemoryInfo {
    pub fn new(
        source_schema: SchemaRef,
        cache_key: String,
        cache_entry: PartitionCacheEntry,
        num_partitions: usize,
        size_bytes: usize,
        num_rows: usize,
        clustering_spec: Option<ClusteringSpecRef>,
    ) -> Self {
        Self {
            source_schema,
            cache_key,
            cache_entry,
            num_partitions,
            size_bytes,
            num_rows,
            clustering_spec,
        }
    }
}

impl PartialEq for InMemoryInfo {
    fn eq(&self, other: &Self) -> bool {
        self.cache_key == other.cache_key
    }
}

impl Eq for InMemoryInfo {}

impl Hash for InMemoryInfo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.cache_key.hash(state);
    }
}

static PLACEHOLDER_ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlaceHolderInfo {
    pub source_schema: SchemaRef,
    pub clustering_spec: ClusteringSpecRef,
    pub source_id: usize,
}

impl PlaceHolderInfo {
    pub fn new(source_schema: SchemaRef, clustering_spec: ClusteringSpecRef) -> Self {
        Self {
            source_schema,
            clustering_spec,
            source_id: PLACEHOLDER_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        }
    }
}
