use dedupe2::image_reader::hash_all_bytes;

fn check(len: usize) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.bin");
    let mut bytes = Vec::with_capacity(len);
    let mut x: u64 = 0x9e3779b97f4a7c15;
    for _ in 0..len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        bytes.push((x >> 56) as u8);
    }
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(
        hash_all_bytes(&path).unwrap(),
        seahash::hash(&bytes),
        "streaming hash diverged at len {len}"
    );
}

#[test]
fn streaming_hash_matches_one_shot_hash() {
    for len in [0, 1, 7, 8, 9, 15, 65535, 65536, 65537, 1 << 18, (1 << 20) + 3] {
        check(len);
    }
}
