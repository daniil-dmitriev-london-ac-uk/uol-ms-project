//! these tests cover stable low level behavior.

use heapstore::crc32::crc32c;
use heapstore::format::{RecordHeader, RegistryRow, Slot, header_len};
use heapstore::rng::{SplitMix64, payload_for};

#[test]
fn crc32c_reference_vectors() {
    assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    assert_eq!(crc32c(b""), 0);
    assert_eq!(crc32c(&[0u8; 32]), 0x8A91_36AA);
    assert_eq!(crc32c(&[0xFFu8; 32]), 0x62A8_AB43);
}

#[test]
fn splitmix64_reference() {
    let mut rng = SplitMix64::new(0);

    assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);

    let repeated_values: Vec<u64> = (0..8).map(|_| SplitMix64::new(7).next_u64()).collect();

    assert!(repeated_values.windows(2).all(|window| window[0] == window[1]));
    assert_ne!(SplitMix64::new(1).next_u64(), SplitMix64::new(2).next_u64());

    let mut fill_rng = SplitMix64::new(3);
    let mut buffer = [0u8; 13];

    fill_rng.fill(&mut buffer);

    assert_ne!(buffer, [0u8; 13]);
}

#[test]
fn payload_generator_is_stable() {
    let (mut first_payload, mut second_payload) = (Vec::new(), Vec::new());

    payload_for(1, 42, 1000, &mut first_payload);
    payload_for(1, 42, 1000, &mut second_payload);

    assert_eq!(first_payload, second_payload);

    payload_for(1, 43, 1000, &mut second_payload);

    assert_ne!(first_payload, second_payload);
}

#[test]
fn record_header_roundtrip_both_layouts() {
    for with_bucket in [false, true] {
        let header_size = header_len(with_bucket);

        assert_eq!(header_size, if with_bucket { 38 } else { 30 });

        let header = RecordHeader {
            len: 12345,
            bucket: 7,
            id: 99,
            version: 3,
            crc: 0xDEADBEEF,
        };

        let mut buffer = vec![0u8; header_size];

        header.encode(with_bucket, &mut buffer);

        let decoded = RecordHeader::decode(&buffer, with_bucket).expect("decode");

        assert_eq!(decoded.len, 12345);
        assert_eq!(decoded.id, 99);
        assert_eq!(decoded.version, 3);
        assert_eq!(decoded.crc, 0xDEADBEEF);
        assert_eq!(decoded.bucket, if with_bucket { 7 } else { 0 });

        for byte_index in 0..header_size {
            let mut corrupted = buffer.clone();

            corrupted[byte_index] ^= 0x55;

            assert!(
                RecordHeader::decode(&corrupted, with_bucket).is_none(),
                "byte {byte_index} undetected"
            );

        }


    }


}

#[test]
fn slot_zero_means_empty() {
    assert!(Slot::decode(&[0u8; 16]).is_none());

    let slot = Slot {
        offset: 4096,
        total_len: 68,
    };

    let mut buffer = [0u8; 16];

    slot.encode(&mut buffer);

    assert_eq!(Slot::decode(&buffer), Some(slot));
    assert_eq!(Slot::file_offset(10), 4096 + 160);
}


#[test]
fn registry_row_roundtrip_and_corruption() {
    let row = RegistryRow {
        kind: b'D',
        bucket: 5,
        start: 4096,
        size: 1 << 20,
    };

    let mut buffer = [0u8; 64];

    row.encode(&mut buffer);

    assert_eq!(RegistryRow::decode(&buffer), Some(row));

    buffer[17] ^= 1;

    assert_eq!(RegistryRow::decode(&buffer), None);
}

#[test]
fn uring_rejects_zero_chunk() {
    assert!(heapstore::UringIo::new(8, 0).is_err());
}
