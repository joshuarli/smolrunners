mod content;
mod error;
mod generation;
mod outcome;
mod private;
mod scope;

pub use content::{ContentStore, Digest, read_json, write_atomic, write_json};
pub use error::{CacheError, Result};
pub use generation::{
    Generation, GenerationState, GenerationStore, Manifest, ManifestEntry, Quota,
};
pub use outcome::{
    OUTCOME_EVENT_KIND, CacheOutcome, Fallback, OutcomeRow, read_outcomes, record_outcome,
};
pub use private::PrivateView;
pub use scope::{CacheAuthority, TrustTier};

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use crate::cache::content::ContentStore;
    use crate::cache::generation::{GenerationState, GenerationStore, ManifestEntry, Quota};
    use crate::cache::outcome::{CacheOutcome, read_outcomes, record_outcome};
    use crate::cache::private::PrivateView;
    use crate::cache::scope::{CacheAuthority, TrustTier};
    use crate::store::Store;
    use crate::store::test_support;

    fn scratch(label: &str) -> PathBuf {
        let mut path = test_support::temp_db_path(label);
        path.set_extension("cache");
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create scratch");
        path
    }

    #[test]
    fn cache_outcome_is_written_from_a_private_view_after_quiescence() {
        let root = scratch("pipeline");
        let store = Store::open(root.join("store.sqlite")).expect("open store");
        let generations = GenerationStore::new(root.join("cache"));
        let source = root.join("source");
        fs::create_dir_all(source.join("toolchain")).expect("create source");
        fs::write(source.join("toolchain/gcc"), b"gcc-14").expect("write tool");
        let owner = CacheAuthority::new("acme/widgets", TrustTier::Maintainer);
        let baseline = generations
            .create(&source, &owner, None, "toolchain", "t1")
            .expect("create baseline");
        let manifest = generations.manifest(&baseline).expect("manifest");
        let view = PrivateView::create(
            &root.join("cache").join("views"),
            "job-1",
            &manifest,
            generations.blobs(),
        ).expect("private view");
        view.write("toolchain/gcc", b"gcc-15").expect("private write");
        generations.acquire_writer(&baseline.id).expect("acquire");
        generations.release_writer(&baseline.id).expect("release");
        let prior = generations.publish(&baseline.id, "t2").expect("publish");
        assert_eq!(prior, None);

        let outcome = CacheOutcome {
            schema_version: 1,
            job: "job-1".to_owned(),
            repository: "acme/widgets".to_owned(),
            trust_tier: "maintainer".to_owned(),
            generation: Some(baseline.id.clone()),
            private_view: Some(view.root().to_string_lossy().into_owned()),
            hit: true,
            fallback: None,
            published_generation: Some(baseline.id.clone()),
            recorded_at: "2026-11-21T00:00:00Z".to_owned(),
        };
        record_outcome(&store, &outcome).expect("record outcome");
        let rows = read_outcomes(&store).expect("read outcomes");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].payload["published_generation"],
            serde_json::Value::String(baseline.id.clone())
        );
        assert_eq!(generations.current("acme/widgets").as_deref(), Some(baseline.id.as_str()));
        assert_eq!(generations.get(&baseline.id).expect("reload").state, GenerationState::Published);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cache_quota_and_scope_reasons_are_recorded_as_fallbacks() {
        let root = scratch("fallback_pipeline");
        let store = Store::open(root.join("store.sqlite")).expect("open store");
        let principal = CacheAuthority::new("acme/widgets", TrustTier::Contributor);
        let target = CacheAuthority::new("acme/widgets", TrustTier::Maintainer);
        let denial = principal.require_read(&target).expect_err("scope denied");
        let outcome = CacheOutcome::fallback(
            "job-2",
            "acme/widgets",
            principal.tier,
            denial.to_string(),
            "2026-11-21T00:00:00Z",
        );
        record_outcome(&store, &outcome).expect("record fallback");
        let rows = read_outcomes(&store).expect("read outcomes");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].payload["fallback"]["reason"].as_str().unwrap().contains("broaden"));
        let _ = fs::remove_dir_all(&root);

        let root = scratch("quota_pipeline");
        let generations = GenerationStore::new(root.join("cache"));
        generations.set_quota(&Quota { max_generations: 1, max_bytes: 1000 }).expect("quota");
        let source = root.join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("file"), b"one").expect("file");
        generations.create(&source, &target, None, "one", "t1").expect("first");
        let refusal = generations.create(&source, &target, None, "two", "t2").expect_err("quota refusal");
        let outcome = CacheOutcome::fallback(
            "job-3",
            "acme/widgets",
            target.tier,
            refusal.to_string(),
            "2026-11-21T00:00:00Z",
        );
        let store = Store::open(root.join("store.sqlite")).expect("open store");
        record_outcome(&store, &outcome).expect("record quota fallback");
        let rows = read_outcomes(&store).expect("read outcomes");
        assert!(rows[0].payload["fallback"]["reason"].as_str().unwrap().contains("quota"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cache_manifest_entries_are_content_addressed() {
        let root = scratch("manifest");
        let store = ContentStore::new(root.join("content"));
        let digest = store.put(b"payload").expect("put payload");
        let entry = ManifestEntry {
            path: "dir/file".to_owned(),
            digest: digest.hex(),
            bytes: 7,
        };
        assert_eq!(entry.digest, digest.hex());
        assert_eq!(store.get(&crate::cache::Digest::from_hex(&entry.digest).unwrap()).unwrap(), b"payload");
        let _ = fs::remove_dir_all(&root);
    }
}
