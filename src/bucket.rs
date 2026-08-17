use crate::placement::Placement;

use std::collections::HashMap;
use std::io;

const EXTENT_BYTES: u64 = 1 << 20;
const REGION_SLOTS: u64 = 4096;

struct Extent {
    start: u64,
    size: u64,
    used: u64,
}

pub struct BucketPlacement {
    bucket: u64,
    extents: HashMap<u64, Vec<Extent>>,
    next_data: u64,
    regions: HashMap<u64, (u64, u64)>,
    next_region: u64,
}

impl BucketPlacement {
    pub fn new(bucket: u64) -> Self {
        BucketPlacement {
            bucket,
            extents: HashMap::new(),
            next_data: 0,
            regions: HashMap::new(),
            next_region: 0,
        }
    }
}

impl Placement for BucketPlacement {
    fn allocate(&mut self, payload_len: u64) -> io::Result<(u64, u64)> {
        let record_len = payload_len + 20;
        let region = self.regions.entry(self.bucket).or_insert_with(|| {
            let start = self.next_region;

            self.next_region += REGION_SLOTS;

            (start, 0)
        });

        if region.1 >= REGION_SLOTS {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "slot region is full",
            ));
        }

        let id = region.0 + region.1;

        region.1 += 1;

        let extents = self.extents.entry(self.bucket).or_default();
        let offset = match extents.last_mut() {
            Some(extent) if extent.used + record_len <= extent.size => {
                let offset = extent.start + extent.used;

                extent.used += record_len;

                offset
            }
            _ => {
                let size = record_len.next_multiple_of(4096).max(EXTENT_BYTES);
                let offset = self.next_data;

                self.next_data += size;
                extents.push(Extent {
                    start: offset,
                    size,
                    used: record_len,
                });

                offset
            }
        };

        Ok((offset, id))
    }

    fn note_written(&mut self, _id: u64, _total_len: u64) {}
}
