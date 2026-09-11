mod common;

use common::*;
use heapstore::append::AppendPlacement;
use heapstore::bucket::BucketPlacement;
use heapstore::error::HeapError;
use heapstore::format::{LAYOUT_APPEND, Slot};
use heapstore::heap::{Heap, SyncPolicy};
use heapstore::placement::Placement;
use heapstore::recovery::rebuild;

use std::io::{Read, Seek, SeekFrom, Write};

fn raw_slot(dir: &std::path::Path, id: u64) -> Option<Slot> {
    let mut file = std::fs::File::open(dir.join("index.hs")).unwrap();

    file.seek(SeekFrom::Start(Slot::file_offset(id))).unwrap();

    let mut bytes = [0u8; 16];

    file.read_exact(&mut bytes).unwrap();

    Slot::decode(&bytes)
}

fn zero_slots(dir: &std::path::Path, ids: &[u64]) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.join("index.hs"))
        .unwrap();

    for &id in ids {
        file.seek(SeekFrom::Start(Slot::file_offset(id))).unwrap();
        file.write_all(&[0u8; 16]).unwrap();
    }

    file.sync_all().unwrap();
}

fn flip_byte(dir: &std::path::Path, offset: u64) {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join("data.hs"))
        .unwrap();

    file.seek(SeekFrom::Start(offset)).unwrap();

    let mut bytes = [0u8; 1];

    file.read_exact(&mut bytes).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(&[bytes[0] ^ 0xFF]).unwrap();
    file.sync_all().unwrap();
}

fn tail_restore<P: Placement>(name: &str) {
    let dir = test_dir(name);
    let mut ids = Vec::new();

    {
        let (mut heap, _) = Heap::<_, P>::open(&dir, sync_io(), small_config()).unwrap();

        for key in 0..8u64 {
            ids.push(heap.insert_into(1, &make_payload(key, 1000)).unwrap());
        }

        heap.flush().unwrap();
    }

    zero_slots(&dir, &ids[6..]);

    let (mut heap, recovery_report) = Heap::<_, P>::open(&dir, sync_io(), small_config()).unwrap();

    assert_eq!(recovery_report.restored_slots, 2, "{recovery_report:?}");
    assert!(!recovery_report.rebuilt);

    for (key, &id) in ids.iter().enumerate() {
        assert_record(&mut heap, id, key as u64, 1000);
    }
}

#[test]
fn tail_restore_append() {
    tail_restore::<AppendPlacement>("tail-append");
}

#[test]
fn tail_restore_bucket() {
    tail_restore::<BucketPlacement>("tail-bucket");
}

#[test]
fn corrupt_payload_detected_and_localized() {
    let dir = test_dir("corrupt-payload");
    let record_count = 20u64;

    {
        let (mut heap, _) =
            Heap::<_, AppendPlacement>::open(&dir, sync_io(), small_config()).unwrap();

        for key in 0..record_count {
            heap.insert(&make_payload(key, 2000)).unwrap();
        }

        heap.flush().unwrap();
    }

    let victim = 7u64;
    let slot = raw_slot(&dir, victim).unwrap();

    flip_byte(&dir, slot.offset + 30 + 100);

    {
        let (mut heap, _) =
            Heap::<_, AppendPlacement>::open(&dir, sync_io(), small_config()).unwrap();
        let mut out = Vec::new();

        assert!(matches!(
            heap.read(victim, &mut out),
            Err(HeapError::Corrupt { .. })
        ));

        assert_record(&mut heap, victim + 1, victim + 1, 2000);
    }

    let mut io = sync_io();
    let stats = rebuild(&dir, &mut io, LAYOUT_APPEND, true, &small_config()).unwrap();

    assert_eq!(stats.records, record_count);
    assert_eq!(stats.bad_payload, 1);

    let (mut heap, _) = Heap::<_, AppendPlacement>::open(&dir, sync_io(), small_config()).unwrap();

    assert!(matches!(
        heap.read(victim, &mut Vec::new()),
        Err(HeapError::NotFound(_))
    ));

    for key in (0..record_count).filter(|&key| key != victim) {
        assert_record(&mut heap, key, key, 2000);
    }
}

#[test]
fn corrupt_header_resyncs() {
    let dir = test_dir("corrupt-header");
    let record_count = 30u64;

    {
        let (mut heap, _) =
            Heap::<_, AppendPlacement>::open(&dir, sync_io(), small_config()).unwrap();

        for key in 0..record_count {
            heap.insert(&make_payload(key, 500)).unwrap();
        }

        heap.flush().unwrap();
    }

    let slot = raw_slot(&dir, 10).unwrap();

    flip_byte(&dir, slot.offset);

    let mut io = sync_io();
    let stats = rebuild(&dir, &mut io, LAYOUT_APPEND, true, &small_config()).unwrap();

    assert!(stats.resyncs >= 1, "{stats:?}");
    assert_eq!(stats.records, record_count - 1);

    let (mut heap, _) = Heap::<_, AppendPlacement>::open(&dir, sync_io(), small_config()).unwrap();

    for key in (0..record_count).filter(|&key| key != 10) {
        assert_record(&mut heap, key, key, 500);
    }

    assert!(matches!(
        heap.read(10, &mut Vec::new()),
        Err(HeapError::NotFound(_))
    ));
}

#[test]
fn truncated_tail() {
    let dir = test_dir("truncate");
    let record_count = 12u64;

    {
        let (mut heap, _) =
            Heap::<_, AppendPlacement>::open(&dir, sync_io(), small_config()).unwrap();

        for key in 0..record_count {
            heap.insert(&make_payload(key, 3000)).unwrap();
        }

        heap.flush().unwrap();
    }

    let last_slot = raw_slot(&dir, record_count - 1).unwrap();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.join("data.hs"))
        .unwrap();

    file.set_len(last_slot.offset + last_slot.total_len / 2)
        .unwrap();

    drop(file);

    let mut io = sync_io();
    let stats = rebuild(&dir, &mut io, LAYOUT_APPEND, true, &small_config()).unwrap();

    assert_eq!(stats.records, record_count - 1);

    let (mut heap, _) = Heap::<_, AppendPlacement>::open(&dir, sync_io(), small_config()).unwrap();

    for key in 0..record_count - 1 {
        assert_record(&mut heap, key, key, 3000);
    }
}

fn index_loss<P: Placement>(name: &str) {
    let dir = test_dir(name);
    let mut ids = Vec::new();

    {
        let (mut heap, _) = Heap::<_, P>::open(&dir, sync_io(), small_config()).unwrap();

        for key in 0..25u64 {
            ids.push(heap.insert_into(key % 3, &make_payload(key, 800)).unwrap());
        }

        heap.update(ids[4], &make_payload(9004, 1200)).unwrap();
        heap.update(ids[4], &make_payload(9005, 300)).unwrap();

        heap.delete(ids[9]).unwrap();

        heap.flush().unwrap();
    }

    std::fs::remove_file(dir.join("index.hs")).unwrap();

    if P::WITH_BUCKET {
        std::fs::remove_file(dir.join("registry.hs")).unwrap();
    }

    let (mut heap, recovery_report) = Heap::<_, P>::open(&dir, sync_io(), small_config()).unwrap();

    assert!(recovery_report.rebuilt);

    for (key, &id) in ids.iter().enumerate() {
        match key {
            4 => assert_record(&mut heap, id, 9005, 300),

            9 => assert_record(&mut heap, id, 9, 800),
            _ => assert_record(&mut heap, id, key as u64, 800),
        }
    }

    let id = heap.insert_into(1, &make_payload(7777, 640)).unwrap();

    assert!(!ids.contains(&id));

    assert_record(&mut heap, id, 7777, 640);
}

#[test]
fn index_loss_append() {
    index_loss::<AppendPlacement>("idxloss-append");
}

#[test]
fn index_loss_bucket() {
    index_loss::<BucketPlacement>("idxloss-bucket");
}

#[test]
fn group_sync_loses_at_most_n_minus_1() {
    let dir = test_dir("group-sync");
    let config = heapstore::heap::HeapConfig {
        sync_policy: SyncPolicy::EveryN(4),
        ..small_config()
    };

    {
        let (mut heap, _) =
            Heap::<_, AppendPlacement>::open(&dir, sync_io(), config.clone()).unwrap();

        for key in 0..6u64 {
            heap.insert(&make_payload(key, 900)).unwrap();
        }

        heap.simulate_crash();
    }

    let (mut heap, _) = Heap::<_, AppendPlacement>::open(&dir, sync_io(), config).unwrap();

    for key in 0..4u64 {
        assert_record(&mut heap, key, key, 900);
    }

    for key in 4..6u64 {
        assert!(matches!(
            heap.read(key, &mut Vec::new()),
            Err(HeapError::NotFound(_))
        ));
    }
}
