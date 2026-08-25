pub mod append;
pub mod bucket;
pub mod crc32;
pub mod data;
pub mod format;
pub mod heap;
pub mod index;
pub mod io;
pub mod measure;
pub mod placement;
pub mod rng;

pub use append::AppendPlacement;
pub use bucket::BucketPlacement;
pub use heap::{Heap, HeapConfig, SyncPolicy};
pub use io::{BlockIo, sync::SyncIo, uring::UringIo};
pub use rng::{RandomSource, SplitMix64};

pub type SyncAppendHeap = Heap<SyncIo, AppendPlacement>;
pub type SyncBucketHeap = Heap<SyncIo, BucketPlacement>;
pub type UringAppendHeap = Heap<UringIo, AppendPlacement>;
pub type UringBucketHeap = Heap<UringIo, BucketPlacement>;
