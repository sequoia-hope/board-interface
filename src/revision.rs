use sha2::{Digest, Sha256};

use crate::hash::hash;
use crate::model::{Manifest, Record};

/// Prefix for every revision produced by [`revision`], distinct from the geometry hash's
/// `sha256:` so the two can never be mistaken for each other.
pub const REVISION_PREFIX: &str = "rev:sha256:";

/// Derive a manifest's `revision` from its `geometry_hash`, `driver`, `parent` and
/// `last_handover`.
///
/// Every app must use this, so all of them compute the same string. Because the parent is
/// hashed in, a revision differs from its parent even when the geometry did not change (a take
/// over with no edits), and a publish that restores an earlier shape does not repeat an earlier
/// revision. Each revision commits to its whole ancestry, like a git commit.
///
/// Encoding, for implementations in other languages: SHA-256 over the UTF-8 bytes of
/// `board-interface:revision:v1\0`, then each field in this order, where a string is its byte
/// length as a big-endian `u64` followed by its bytes, and an optional value is the byte `0`
/// when absent or the byte `1` followed by the value:
///
/// 1. `geometry_hash` (string)
/// 2. `driver` (string: `waffle`, `composer` or `kicad`)
/// 3. `parent` (optional string)
/// 4. `last_handover` (optional: `by` string, `at` string, `from_revision` string)
///
/// The result is `rev:sha256:` followed by 64 lowercase hex digits.
pub fn revision(manifest: &Manifest) -> String {
    let mut h = Sha256::new();
    h.update(b"board-interface:revision:v1\0");
    put_str(&mut h, &manifest.geometry_hash);
    put_str(&mut h, manifest.driver.as_str());
    match &manifest.parent {
        None => h.update([0u8]),
        Some(p) => {
            h.update([1u8]);
            put_str(&mut h, p);
        }
    }
    match &manifest.last_handover {
        None => h.update([0u8]),
        Some(handover) => {
            h.update([1u8]);
            put_str(&mut h, handover.by.as_str());
            put_str(&mut h, &handover.at);
            put_str(&mut h, &handover.from_revision);
        }
    }
    let digest = h.finalize();
    let mut out = String::with_capacity(REVISION_PREFIX.len() + 64);
    out.push_str(REVISION_PREFIX);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Set `geometry_hash` from the record's geometry, then `revision` from the manifest, in that
/// order. Call this last, after `driver`, `parent` and `last_handover` are set, and before
/// [`crate::write`].
pub fn seal(record: &mut Record) {
    record.manifest.geometry_hash = hash(record);
    record.manifest.revision = revision(&record.manifest);
}

fn put_str(h: &mut Sha256, s: &str) {
    h.update((s.len() as u64).to_be_bytes());
    h.update(s.as_bytes());
}
