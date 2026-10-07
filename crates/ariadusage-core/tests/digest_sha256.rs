use ariadusage_core::digest::sha256_hex;

#[test]
fn test_digest_length_and_lowercase_hex() {
    let digest = sha256_hex("test-namespace", &["part1", "part2"]);
    assert_eq!(digest.len(), 64);
    assert!(
        digest
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    );
}

#[test]
fn test_digest_separation() {
    let d1 = sha256_hex("ns", &["a", "bc"]);
    let d2 = sha256_hex("ns", &["ab", "c"]);
    assert_ne!(
        d1, d2,
        "Parts with different boundaries must produce different digests"
    );

    let d3 = sha256_hex("a", &["bc"]);
    let d4 = sha256_hex("ab", &["c"]);
    assert_ne!(
        d3, d4,
        "Namespace with different boundaries must produce different digests"
    );

    let empty = sha256_hex("ns", &[] as &[&str]);
    let with_empty_part = sha256_hex("ns", &[""]);
    assert_ne!(
        empty, with_empty_part,
        "Empty parts list must differ from list containing empty string"
    );
}

#[test]
fn test_known_empty_parts_digest() {
    // SHA-256 of "ariadusage:test:v1" without delimiter
    // echo -n "ariadusage:test:v1" | sha256sum -> 63d596dd024840ce3673f478ec1aa30953ef8208a0d0d8be70ddadfe20b228f4
    let digest = sha256_hex("ariadusage:test:v1", &[] as &[&str]);
    assert_eq!(
        digest,
        "b7504e0ee5fa6ff38e85f29bf5320fcc634a4cca521bf2df19b50b593275311e"
    );
}

#[test]
fn test_profile_id_determinism() {
    let ns = "ariadusage:codex-credentials-profile:v1";
    let path = "/fakehome/.codex/auth.json";
    let d1 = sha256_hex(ns, &[path]);
    let d2 = sha256_hex(ns, &[path]);
    assert_eq!(d1, d2);
    assert_eq!(d1.len(), 64);
}
