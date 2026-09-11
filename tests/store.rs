mod common;

use common::*;
use heapstore::append::AppendPlacement;
use heapstore::bucket::BucketPlacement;
use heapstore::error::HeapError;
use heapstore::heap::Heap;
use heapstore::io::BlockIo;
use heapstore::placement::Placement;

fn roundtrip<I: BlockIo, P: Placement>(name: &str, make_io: impl Fn() -> I) {
    let dir = test_dir(name);
    let mut keys: Vec<(u64, u64, usize)> = Vec::new();

    {
        let (mut heap, recovery_report) =
            Heap::<I, P>::open(&dir, make_io(), small_config()).unwrap();

        assert!(!recovery_report.rebuilt);

        for key in 0..48u64 {
            let len = 100 + ((key * 37) % 3000) as usize;
            let id = heap.insert_into(key % 4, &make_payload(key, len)).unwrap();

            keys.push((id, key, len));
        }

        let id0 = heap.insert_into(1, &make_payload(999, 0)).unwrap();

        keys.push((id0, 999, 0));

        for &(id, key, len) in &keys {
            assert_record(&mut heap, id, key, len);
        }

        let ids: Vec<u64> = keys.iter().rev().map(|entry| entry.0).collect();
        let mut out = Vec::new();

        heap.read_batch(&ids, &mut out).unwrap();

        for (entry, got) in keys.iter().rev().zip(&out) {
            assert_eq!(*got, make_payload(entry.1, entry.2));
        }

        for index in [0usize, 5, 10] {
            let (id, key, _) = keys[index];

            heap.update(id, &make_payload(key + 1000, 64)).unwrap();
            heap.update(id, &make_payload(key + 2000, 4500)).unwrap();

            keys[index] = (id, key + 2000, 4500);
        }

        let (deleted_id, ..) = keys.remove(20);

        heap.delete(deleted_id).unwrap();

        assert!(matches!(
            heap.read(deleted_id, &mut Vec::new()),
            Err(HeapError::NotFound(_))
        ));
        assert!(matches!(
            heap.delete(deleted_id),
            Err(HeapError::NotFound(_))
        ));

        heap.flush().unwrap();

        for &(id, key, len) in &keys {
            assert_record(&mut heap, id, key, len);
        }
    }

    let (mut heap, recovery_report) = Heap::<I, P>::open(&dir, make_io(), small_config()).unwrap();

    assert!(!recovery_report.rebuilt);
    assert_eq!(recovery_report.restored_slots, 0);

    for &(id, key, len) in &keys {
        assert_record(&mut heap, id, key, len);
    }

    let id = heap.insert_into(2, &make_payload(5000, 700)).unwrap();

    assert!(
        keys.iter().all(|entry| entry.0 != id),
        "record id was reused"
    );

    assert_record(&mut heap, id, 5000, 700);
}

#[test]
fn roundtrip_sync_append() {
    roundtrip::<_, AppendPlacement>("rt-sync-append", sync_io);
}

#[test]
fn roundtrip_sync_bucket() {
    roundtrip::<_, BucketPlacement>("rt-sync-bucket", sync_io);
}

#[test]
fn roundtrip_uring_append() {
    roundtrip::<_, AppendPlacement>("rt-uring-append", uring_io);
}

#[test]
fn roundtrip_uring_bucket() {
    roundtrip::<_, BucketPlacement>("rt-uring-bucket", uring_io);
}

fn big_records<I: BlockIo, P: Placement>(name: &str, make_io: impl Fn() -> I) {
    let dir = test_dir(name);
    let (mut heap, _) = Heap::<I, P>::open(&dir, make_io(), small_config()).unwrap();
    let big_id = heap.insert_into(0, &make_payload(1, 5 << 20)).unwrap();
    let medium_id = heap.insert_into(1, &make_payload(2, 100_000)).unwrap();
    let small_id = heap.insert_into(0, &make_payload(3, 10)).unwrap();

    heap.flush().unwrap();

    assert_record(&mut heap, big_id, 1, 5 << 20);
    assert_record(&mut heap, medium_id, 2, 100_000);
    assert_record(&mut heap, small_id, 3, 10);

    let mut out = Vec::new();

    heap.read_batch(&[small_id, big_id, medium_id], &mut out)
        .unwrap();

    assert_eq!(out[1].len(), 5 << 20);
}

#[test]
fn big_records_all_paths() {
    big_records::<_, AppendPlacement>("big-sync-append", sync_io);
    big_records::<_, BucketPlacement>("big-sync-bucket", sync_io);
    big_records::<_, AppendPlacement>("big-uring-append", uring_io);
    big_records::<_, BucketPlacement>("big-uring-bucket", uring_io);
}

fn drive<I: BlockIo, P: Placement>(dir: &std::path::Path, io: I) {
    let (mut heap, _) = Heap::<I, P>::open(dir, io, small_config()).unwrap();
    let mut ids = Vec::new();

    for key in 0..40u64 {
        ids.push(
            heap.insert_into(
                key % 3,
                &make_payload(key, 50 + (key as usize * 101) % 9000),
            )
            .unwrap(),
        );
    }

    for (index, &id) in ids.iter().enumerate().filter(|(index, _)| index % 7 == 0) {
        heap.update(id, &make_payload(1000 + index as u64, 333))
            .unwrap();
    }

    heap.delete(ids[13]).unwrap();

    heap.flush().unwrap();
}

fn parity<P: Placement>(name: &str) {
    let (sync_dir, uring_dir) = (
        test_dir(&format!("{name}-s")),
        test_dir(&format!("{name}-u")),
    );

    drive::<_, P>(&sync_dir, sync_io());
    drive::<_, P>(&uring_dir, uring_io());

    for file_name in ["data.hs", "index.hs"] {
        let sync_bytes = std::fs::read(sync_dir.join(file_name)).unwrap();
        let uring_bytes = std::fs::read(uring_dir.join(file_name)).unwrap();

        assert_eq!(
            sync_bytes, uring_bytes,
            "{file_name} differs between sync and io_uring"
        );
    }
}

#[test]
fn sync_uring_parity() {
    parity::<AppendPlacement>("parity-append");
    parity::<BucketPlacement>("parity-bucket");
}

#[test]
fn bucket_grouped_reads_are_contiguous() {
    let bucket_count = 4u64;
    let len = 3000usize;
    let bucket_dir = test_dir("contig-bucket");
    let append_dir = test_dir("contig-append");
    let (mut bucket_heap, _) =
        Heap::<_, BucketPlacement>::open(&bucket_dir, sync_io(), small_config()).unwrap();
    let (mut append_heap, _) =
        Heap::<_, AppendPlacement>::open(&append_dir, sync_io(), small_config()).unwrap();
    let mut bucket_ids = Vec::new();
    let mut append_ids = Vec::new();

    for key in 0..64u64 {
        bucket_ids.push(
            bucket_heap
                .insert_into(key % bucket_count, &make_payload(key, len))
                .unwrap(),
        );
        append_ids.push(
            append_heap
                .insert_into(key % bucket_count, &make_payload(key, len))
                .unwrap(),
        );
    }

    bucket_heap.flush().unwrap();
    append_heap.flush().unwrap();

    let group: Vec<usize> = (0..64)
        .filter(|key| key % bucket_count as usize == 1)
        .collect();
    let bucket_group_ids: Vec<u64> = group.iter().map(|&key| bucket_ids[key]).collect();
    let append_group_ids: Vec<u64> = group.iter().map(|&key| append_ids[key]).collect();
    let mut out = Vec::new();

    bucket_heap.reset_io_counters();

    bucket_heap.read_batch(&bucket_group_ids, &mut out).unwrap();

    let bucket_counters = bucket_heap.io_counters();

    append_heap.reset_io_counters();

    append_heap.read_batch(&append_group_ids, &mut out).unwrap();

    let append_counters = append_heap.io_counters();

    assert!(
        bucket_counters.reads <= 4,
        "bucket reads should merge into a few requests, got {}",
        bucket_counters.reads
    );
    assert!(
        append_counters.reads >= group.len() as u64,
        "append records are scattered, expected at least {} reads, got {}",
        group.len(),
        append_counters.reads
    );
    assert!(append_counters.read_bytes > bucket_counters.read_bytes);
}

#[test]
fn write_amplification_shape() {
    let append_dir = test_dir("wa-append");
    let bucket_dir = test_dir("wa-bucket");
    let (mut append_heap, _) =
        Heap::<_, AppendPlacement>::open(&append_dir, sync_io(), small_config()).unwrap();
    let (mut bucket_heap, _) =
        Heap::<_, BucketPlacement>::open(&bucket_dir, sync_io(), small_config()).unwrap();

    append_heap.flush().unwrap();
    bucket_heap.flush().unwrap();

    append_heap.reset_io_counters();
    bucket_heap.reset_io_counters();

    for key in 0..10u64 {
        append_heap
            .insert_into(key, &make_payload(key, 200))
            .unwrap();
        bucket_heap
            .insert_into(key, &make_payload(key, 200))
            .unwrap();
    }

    append_heap.flush().unwrap();
    bucket_heap.flush().unwrap();

    let (append_counters, bucket_counters) = (append_heap.io_counters(), bucket_heap.io_counters());

    assert!(
        bucket_counters.write_bytes >= append_counters.write_bytes * 3,
        "append {append_counters:?}, bucket {bucket_counters:?}"
    );
}

fn update_survives_reopen_and_insert<P: Placement>(name: &str) {
    let dir = test_dir(name);

    {
        let (mut heap, _) = Heap::<_, P>::open(&dir, sync_io(), small_config()).unwrap();

        heap.insert_into(0, &make_payload(0, 500)).unwrap();
        heap.insert_into(1, &make_payload(1, 500)).unwrap();

        heap.update(0, &make_payload(100, 700)).unwrap();

        heap.flush().unwrap();
    }

    let (mut heap, recovery_report) = Heap::<_, P>::open(&dir, sync_io(), small_config()).unwrap();

    assert_eq!(recovery_report.restored_slots, 0, "{recovery_report:?}");

    let id2 = heap.insert_into(0, &make_payload(2, 500)).unwrap();

    heap.flush().unwrap();

    assert_record(&mut heap, 0, 100, 700);
    assert_record(&mut heap, id2, 2, 500);

    drop(heap);

    let (mut heap, _) = Heap::<_, P>::open(&dir, sync_io(), small_config()).unwrap();

    assert_record(&mut heap, 0, 100, 700);
}

#[test]
fn update_survives_reopen_append() {
    update_survives_reopen_and_insert::<AppendPlacement>("upd-reopen-append");
}

#[test]
fn update_survives_reopen_bucket() {
    update_survives_reopen_and_insert::<BucketPlacement>("upd-reopen-bucket");
}

#[test]
fn interleaved_bucket_writes_batch_in_staging() {
    let dir = test_dir("interleave-bucket");
    let (mut heap, _) = Heap::<_, BucketPlacement>::open(&dir, sync_io(), small_config()).unwrap();
    let buckets = 4u64;
    let per_bucket = 12u64;

    for bucket in 0..buckets {
        heap.insert_into(bucket, &make_payload(bucket, 200))
            .unwrap();
    }

    heap.flush().unwrap();

    heap.reset_io_counters();

    let mut ids = Vec::new();

    for key in 0..buckets * per_bucket {
        ids.push(
            heap.insert_into(key % buckets, &make_payload(100 + key, 200))
                .unwrap(),
        );
    }

    heap.flush().unwrap();

    let counters = heap.io_counters();

    assert!(
        counters.writes <= 16,
        "interleaved writes should remain staged, got {} requests",
        counters.writes
    );

    for (key, &id) in ids.iter().enumerate() {
        assert_record(&mut heap, id, 100 + key as u64, 200);
    }
}
